//! RendererManager — spawns and supervises `waywallen-renderer` child
//! processes, forwards control messages to them over Unix-domain sockets,
//! and parks their event stream into per-renderer broadcast channels.
//!
//! This module is the Rust daemon's counterpart to the C++ host program
//! in `open-wallpaper-engine/host/`.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::thread;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex as TokioMutex};
use uuid::Uuid;

use crate::ipc::proto::{ControlMsg, EventMsg};
use crate::ipc::uds::{recv_event, send_control};
use crate::plugin::renderer_registry::{RendererDef, RendererRegistry};
use crate::wallpaper_type::WallpaperType;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub type RendererId = String;

#[derive(Debug, Clone, Default)]
pub struct SpawnRequest {
    /// The wallpaper type determines which renderer binary is spawned.
    pub wp_type: WallpaperType,
    /// Type-specific key-value data forwarded as CLI args to the renderer.
    /// For "scene": {"scene": "<pkg>", "assets": "<dir>"}.
    /// For "image": {"path": "<file>"}.
    pub metadata: HashMap<String, String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// When true, pass `--test-pattern` to the renderer host, which
    /// bypasses `SceneWallpaper::loadScene` and drives the offscreen
    /// ExSwapchain ring on a host-owned timer. Used to bring up the
    /// full daemon/display pipeline before a real Wallpaper Engine
    /// assets directory is available (see plan.md I4).
    pub test_pattern: bool,
}

/// Snapshot of the most recent `BindBuffers` event, plus the DMA-BUF FDs
/// the host attached to it. Owned by the manager; display endpoints will
/// `dup(2)` individual fds out of it when a new subscriber connects.
pub struct BindSnapshot {
    /// Monotonically increasing per-renderer pool generation. Starts at
    /// 1 for the first `BindBuffers` and increments on every rebind.
    /// Propagated as `buffer_generation` on the display wire.
    pub generation: u64,
    pub count: u32,
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub modifier: u64,
    pub plane_offset: u64,
    pub sizes: Vec<u64>,
    pub fds: Vec<OwnedFd>,
}

/// Upper bound on the number of per-seq sync_fd entries the reader
/// keeps around before evicting the oldest. Renderers produce ~60 fps,
/// so 16 gives display clients ~250 ms to drain before fences start
/// dropping — plenty for a healthy event loop.
const SYNC_FD_RETENTION: usize = 16;

/// Per-renderer state. Cheap to clone via `Arc`; the inner fields are
/// shared across HTTP handlers and the reader thread.
pub struct RendererHandle {
    pub id: RendererId,
    pub wp_type: WallpaperType,
    pub width: u32,
    pub height: u32,
    pub fps: u32,

    /// Blocking std UnixStream. Guarded by a std Mutex so HTTP handlers
    /// hold the lock only while a `sendmsg` is in flight; they spawn the
    /// actual send onto the blocking pool so the runtime isn't parked.
    sock: Arc<StdMutex<StdUnixStream>>,

    /// Broadcast of every event the host emits (besides the FDs on the
    /// initial BindBuffers — those are stored in `bind_snapshot` so
    /// late subscribers can dup them).
    events: broadcast::Sender<EventMsg>,

    /// Populated when the host sends its first `BindBuffers` event.
    bind_snapshot: Arc<StdMutex<Option<BindSnapshot>>>,

    /// Per-frame acquire fence file descriptors, indexed by `seq`.
    /// The reader thread stashes the `OwnedFd` that arrives with each
    /// `FrameReady { has_sync_fd: true }` event; the display endpoint
    /// consumes it (exactly once per seq) via `take_sync_fd`. Older
    /// entries are evicted once the map exceeds `SYNC_FD_RETENTION`.
    ///
    /// Phase 3b limitation: only one consumer gets the real fd per
    /// (seq); additional display subscribers fall back to the
    /// `dummy_fence` placeholder. Multi-display real-sync fan-out
    /// will require a dup-on-take API.
    sync_fds: Arc<StdMutex<std::collections::VecDeque<(u64, OwnedFd)>>>,

    /// The child process. Kept alive so dropping the manager reaps it.
    child: Arc<TokioMutex<Option<Child>>>,
}

impl RendererHandle {
    pub fn events(&self) -> broadcast::Receiver<EventMsg> {
        self.events.subscribe()
    }

    /// Borrow the cached bind snapshot. Returns `None` until the host's
    /// first frame has been rendered and the fds arrived.
    pub fn bind_snapshot(&self) -> Arc<StdMutex<Option<BindSnapshot>>> {
        Arc::clone(&self.bind_snapshot)
    }

    /// Obtain a dup'd copy of the acquire sync_fd that arrived with
    /// `FrameReady` seq. Each caller gets an independent kernel
    /// reference to the same underlying `dma_fence` sync_file, so
    /// multiple display subscribers can each wait on (or import) the
    /// fence without interfering with one another.
    ///
    /// The original fd stays in the deque and is evicted only when
    /// the retention limit is hit (new frames push out old ones) or
    /// on a rebind.
    ///
    /// Returns `None` if the fd was never recorded (producer didn't
    /// export one) or has already been evicted (>SYNC_FD_RETENTION
    /// newer frames have arrived).
    pub fn clone_sync_fd(&self, seq: u64) -> Option<OwnedFd> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let guard = self.sync_fds.lock().ok()?;
        let (_, fd) = guard.iter().find(|(s, _)| *s == seq)?;
        let dup_raw = nix::unistd::dup(fd.as_raw_fd()).ok()?;
        // SAFETY: nix::unistd::dup returned a fresh fd we now own.
        Some(unsafe { OwnedFd::from_raw_fd(dup_raw) })
    }
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

pub struct RendererManager {
    inner: TokioMutex<Inner>,
    /// Plugin registry mapping wallpaper types to renderer binaries.
    registry: RendererRegistry,
}

struct Inner {
    renderers: HashMap<RendererId, Arc<RendererHandle>>,
}

impl RendererManager {
    pub fn new(registry: RendererRegistry) -> Self {
        Self {
            inner: TokioMutex::new(Inner {
                renderers: HashMap::new(),
            }),
            registry,
        }
    }

    /// Test-only convenience: construct a manager whose registry has a
    /// single "scene" renderer pointed at `$WAYWALLEN_RENDERER_BIN`. If
    /// that env var is unset the registry is empty and any spawn call
    /// will fail with "no renderer registered for type 'scene'".
    pub fn new_default() -> Self {
        let mut registry = RendererRegistry::new();
        if let Some(bin) = std::env::var_os("WAYWALLEN_RENDERER_BIN") {
            registry.register(RendererDef {
                name: "test-scene".to_string(),
                bin: PathBuf::from(bin),
                types: vec!["scene".to_string()],
                extra_args: vec![],
                priority: 100,
            });
        }
        Self::new(registry)
    }

    /// Access the renderer registry (for HTTP introspection endpoints).
    pub fn registry(&self) -> &RendererRegistry {
        &self.registry
    }

    /// Spawn a fresh renderer-host subprocess, wait for its `Ready`
    /// event, and return its id. Fails (and cleans up the child) if the
    /// host doesn't come online within `timeout`.
    pub async fn spawn(&self, req: SpawnRequest) -> Result<RendererId> {
        let id: RendererId = Uuid::new_v4().to_string();

        // Create a listening UDS at a temp path; the child connects to
        // it shortly after exec().
        let sock_path = temp_sock_path(&id);
        let _ = std::fs::remove_file(&sock_path);
        let listener = tokio::net::UnixListener::bind(&sock_path)
            .with_context(|| format!("bind {}", sock_path.display()))?;

        // Best-effort cleanup of the socket file at the end of spawn —
        // the connection survives unlink(2).
        let _cleanup = TempUnlink(sock_path.clone());

        let renderer_def = self
            .registry
            .resolve(&req.wp_type)
            .ok_or_else(|| anyhow!("no renderer registered for type '{}'", req.wp_type))?
            .clone();

        let mut cmd = Command::new(&renderer_def.bin);
        cmd.arg("--ipc")
            .arg(&sock_path)
            .arg("--width")
            .arg(req.width.to_string())
            .arg("--height")
            .arg(req.height.to_string())
            .arg("--fps")
            .arg(req.fps.to_string());
        // Forward type-specific metadata as --key value CLI args.
        for (key, value) in &req.metadata {
            cmd.arg(format!("--{key}")).arg(value);
        }
        // Append extra_args from the renderer manifest.
        for arg in &renderer_def.extra_args {
            cmd.arg(arg);
        }
        if req.test_pattern {
            cmd.arg("--test-pattern");
        }
        cmd.kill_on_drop(true)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {}", renderer_def.bin.display()))?;

        // Accept, with a bound to avoid hanging forever on a broken host.
        let accept = listener.accept();
        let (tokio_stream, _addr) = tokio::time::timeout(Duration::from_secs(10), accept)
            .await
            .map_err(|_| {
                let _ = child.start_kill();
                anyhow!("timed out waiting for waywallen-renderer to connect back")
            })?
            .context("accept")?;

        // Convert to a blocking std UnixStream for the rest of the
        // lifecycle: the ipc::uds helpers use nix sendmsg/recvmsg which
        // need a real blocking fd.
        let std_stream = tokio_stream
            .into_std()
            .context("UnixStream::into_std")?;
        std_stream
            .set_nonblocking(false)
            .context("clear O_NONBLOCK on accepted stream")?;

        // Read the host's initial `Ready` event synchronously so we
        // can fail spawn() with a clear error if initVulkan blew up.
        let ready_stream = std_stream
            .try_clone()
            .context("try_clone for Ready poll")?;
        let ready: (EventMsg, Vec<OwnedFd>) = tokio::task::spawn_blocking(move || {
            recv_event(&ready_stream).map_err(|e| anyhow!("recv Ready: {e}"))
        })
        .await
        .context("ready poll join")??;
        if !matches!(ready.0, EventMsg::Ready) {
            let _ = child.start_kill();
            return Err(anyhow!(
                "host emitted {:?} before Ready; aborting spawn",
                ready.0
            ));
        }
        if !ready.1.is_empty() {
            log::warn!("Ready unexpectedly carried {} fds; dropping", ready.1.len());
        }

        // Now wire up the permanent reader thread and store the handle.
        let (events_tx, _events_rx) = broadcast::channel::<EventMsg>(256);
        let bind_snapshot: Arc<StdMutex<Option<BindSnapshot>>> =
            Arc::new(StdMutex::new(None));
        let sync_fds: Arc<StdMutex<std::collections::VecDeque<(u64, OwnedFd)>>> =
            Arc::new(StdMutex::new(std::collections::VecDeque::new()));

        let sock = Arc::new(StdMutex::new(std_stream));
        let reader_sock = sock.clone();
        let reader_events = events_tx.clone();
        let reader_snapshot = bind_snapshot.clone();
        let reader_sync_fds = sync_fds.clone();
        let reader_id = id.clone();
        thread::spawn(move || {
            run_reader(reader_id, reader_sock, reader_events, reader_snapshot, reader_sync_fds);
        });

        let handle = Arc::new(RendererHandle {
            id: id.clone(),
            wp_type: req.wp_type.clone(),
            width: req.width,
            height: req.height,
            fps: req.fps,
            sock,
            events: events_tx,
            bind_snapshot,
            sync_fds,
            child: Arc::new(TokioMutex::new(Some(child))),
        });

        {
            let mut inner = self.inner.lock().await;
            inner.renderers.insert(id.clone(), handle);
        }
        log::info!("spawned renderer {id} ({}x{} @ {} fps)", req.width, req.height, req.fps);
        Ok(id)
    }

    pub async fn get(&self, id: &str) -> Option<Arc<RendererHandle>> {
        let inner = self.inner.lock().await;
        inner.renderers.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<RendererId> {
        let inner = self.inner.lock().await;
        inner.renderers.keys().cloned().collect()
    }

    /// Fire-and-forget control send. Returns an error only if the
    /// renderer is unknown or the underlying socket write fails.
    pub async fn send_control(&self, id: &str, msg: ControlMsg) -> Result<()> {
        let handle = self
            .get(id)
            .await
            .ok_or_else(|| anyhow!("unknown renderer: {id}"))?;
        let sock = handle.sock.clone();
        tokio::task::spawn_blocking(move || {
            let guard = sock
                .lock()
                .map_err(|e| anyhow!("sock mutex poisoned: {e}"))?;
            send_control(&*guard, &msg, &[]).map_err(|e| anyhow!("send_control: {e}"))
        })
        .await
        .context("send_control join")?
    }

    /// Send Shutdown, then kill + reap the child. Removes from the map.
    pub async fn kill(&self, id: &str) -> Result<()> {
        let handle = {
            let mut inner = self.inner.lock().await;
            inner.renderers.remove(id)
        }
        .ok_or_else(|| anyhow!("unknown renderer: {id}"))?;

        // Try a polite shutdown first. Ignore the result — we're going
        // to SIGKILL it anyway.
        let sock = handle.sock.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(guard) = sock.lock() {
                let _ = send_control(&*guard, &ControlMsg::Shutdown, &[]);
            }
        })
        .await;

        let mut child_guard = handle.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            let _ = child.start_kill();
            // Give it a moment to exit cleanly before we move on.
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
        log::info!("killed renderer {id}");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reader thread
// ---------------------------------------------------------------------------

fn run_reader(
    id: RendererId,
    sock: Arc<StdMutex<StdUnixStream>>,
    events: broadcast::Sender<EventMsg>,
    bind_snapshot: Arc<StdMutex<Option<BindSnapshot>>>,
    sync_fds: Arc<StdMutex<std::collections::VecDeque<(u64, OwnedFd)>>>,
) {
    // Hold the stream by dup'ing the raw fd so the blocking recv is not
    // contending with sends on the same mutex. recvmsg on an AF_UNIX
    // stream socket is safe to call from a different fd referencing the
    // same open file description.
    let read_stream = {
        let guard = match sock.lock() {
            Ok(g) => g,
            Err(_) => {
                log::error!("renderer {id}: sock mutex poisoned, reader exiting");
                return;
            }
        };
        match guard.try_clone() {
            Ok(s) => s,
            Err(e) => {
                log::error!("renderer {id}: try_clone failed: {e}");
                return;
            }
        }
    };

    loop {
        let received = match recv_event(&read_stream) {
            Ok(ok) => ok,
            Err(e) => {
                log::info!("renderer {id}: reader exit: {e}");
                return;
            }
        };
        let (msg, fds) = received;

        // If this is the first BindBuffers, cache it with its fds.
        if let EventMsg::BindBuffers {
            count,
            fourcc,
            width,
            height,
            stride,
            modifier,
            plane_offset,
            ref sizes,
        } = msg
        {
            if fds.is_empty() {
                log::warn!("renderer {id}: BindBuffers arrived without fds");
            } else {
                let prev_gen = bind_snapshot
                    .lock()
                    .ok()
                    .and_then(|g| g.as_ref().map(|s| s.generation));
                let generation = prev_gen.map_or(1, |g| g + 1);
                let snap = BindSnapshot {
                    generation,
                    count,
                    fourcc,
                    width,
                    height,
                    stride,
                    modifier,
                    plane_offset,
                    sizes: sizes.clone(),
                    fds,
                };
                if let Ok(mut guard) = bind_snapshot.lock() {
                    *guard = Some(snap);
                    log::info!("renderer {id}: BindBuffers cached (gen={generation})");
                }
                // A rebind retires any pending acquire fences — they
                // belong to the previous buffer_generation and cannot
                // be waited on against the new textures.
                if let Ok(mut guard) = sync_fds.lock() {
                    guard.clear();
                }
            }
        } else if let EventMsg::FrameReady { seq, .. } = msg {
            // frame_ready always carries exactly one sync_fd: the codec
            // enforced expected_fds() == 1 before handing us `fds`.
            let mut taken = fds;
            let fd = taken.remove(0);
            if let Ok(mut guard) = sync_fds.lock() {
                while guard.len() >= SYNC_FD_RETENTION {
                    guard.pop_front();
                }
                guard.push_back((seq, fd));
            }
        } else if !fds.is_empty() {
            log::warn!(
                "renderer {id}: unexpected fds on event {msg:?}, dropping"
            );
        }

        // Broadcast to any subscribers. No subscribers means no error:
        // SendError is only returned when receivers drop, which is fine.
        let _ = events.send(msg);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_sock_path(id: &str) -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = runtime_dir.join("waywallen");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("renderer-{id}.sock"))
}

struct TempUnlink(PathBuf);
impl Drop for TempUnlink {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[allow(dead_code)]
fn _assert_path_ok<P: AsRef<std::path::Path>>(_p: P) {} // compile-time shim
