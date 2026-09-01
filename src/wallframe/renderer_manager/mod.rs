use crate::error::{Error, Result, ResultExt};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, RwLock as StdRwLock};
use std::thread;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, watch, Mutex as TokioMutex};
use uuid::Uuid;

use crate::wallframe::ipc::proto::{
    AudioWindow, BufferDirective, BufferFormat, BufferMemorySource, BufferPath, ControlMsg,
    EventMsg, EventSubscriptionResult, EventSubscriptionStatus, LogLevel, MediaPlaybackState,
    PointerAxis, PointerButton, PointerMotion, RendererInit, RendererState, WireMprisSnapshot,
    RENDERER_STATE_FIELD_CLEAR_COLOR, RENDERER_STATE_FIELD_RUNTIME_TAGS,
    RENDERER_STATE_KNOWN_FIELDS,
};
use crate::wallframe::ipc::uds::{recv_event, send_control};

/// Renderer IPC compatibility version the daemon currently emits. Bump
/// this when the daemon/renderer wire contract changes.
pub const SPAWN_VERSION: u32 = 11;
use crate::catalog::entry::WallpaperType;
use crate::plugin::renderer_registry::{RendererActivityMode, RendererDef, RendererRegistry};
use crate::settings::SettingsStore;

mod handshake;
mod reader;
mod reported_state;
mod subscriptions;
mod writer;

use handshake::validate_renderer_spawn_version;
pub(crate) use handshake::{build_init_msg, run_init_handshake};
use reader::run_reader;
use reported_state::apply_renderer_state_patch;
#[cfg(test)]
use reported_state::validate_runtime_tags;
use subscriptions::RendererSubscriptionRegistry;
pub use subscriptions::{
    RendererEventKind, RendererProcessOwnershipSnapshot, RendererSubscription,
    RendererSubscriptionSnapshot,
};
use writer::*;

// ---------------------------------------------------------------------------
// Public types

pub type RendererId = String;
pub type RendererProcessGeneration = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RendererLogSnapshot {
    level: LogLevel,
    revision: u64,
}

fn log_level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Off => "off",
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
        LogLevel::Trace => "trace",
    }
}

fn renderer_log_level(level: log::LevelFilter) -> LogLevel {
    match level {
        log::LevelFilter::Off => LogLevel::Off,
        log::LevelFilter::Error => LogLevel::Error,
        log::LevelFilter::Warn => LogLevel::Warn,
        log::LevelFilter::Info => LogLevel::Info,
        log::LevelFilter::Debug => LogLevel::Debug,
        log::LevelFilter::Trace => LogLevel::Trace,
    }
}

fn initial_renderer_log_level() -> (LogLevel, bool) {
    (
        crate::logging::ww_log_level()
            .map(renderer_log_level)
            .unwrap_or(LogLevel::Info),
        crate::logging::ww_log_active(),
    )
}

fn renderer_process_label(name: &str, pid: Option<u32>, id: &str) -> String {
    let name = if name.is_empty() { "renderer" } else { name };
    let identity = pid.map_or_else(
        || id.chars().take(8).collect::<String>(),
        |pid| pid.to_string(),
    );
    if identity.is_empty() {
        name.to_owned()
    } else {
        format!("{name}-{identity}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererProcessExitKind {
    Stopped,
    Killed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererProcessExit {
    pub renderer_id: RendererId,
    pub process_generation: RendererProcessGeneration,
    pub kind: RendererProcessExitKind,
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererRuntimeTag {
    pub key: String,
    pub value: String,
}

const MAX_EVENT_SUBSCRIPTIONS: usize = 16;
const MAX_EVENT_KIND_BYTES: usize = 64;
const MAX_EVENT_KIND_TOTAL_BYTES: usize = 512;
const MAX_RUNTIME_TAGS: usize = 8;
const MAX_RUNTIME_TAG_KEY_BYTES: usize = 32;
const MAX_RUNTIME_TAG_VALUE_BYTES: usize = 64;
const WRITER_QUEUE_CAPACITY: usize = 64;
const RENDERER_FAILED_EXIT_GRACE: Duration = Duration::from_millis(250);
const RENDERER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RENDERER_INIT_TIMEOUT: Duration = Duration::from_secs(10);
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
const STDERR_REASON_BYTES: usize = 512;

fn append_stderr_reason(
    mut reason: String,
    snapshot: crate::wallframe::process_stdio::StderrSnapshot,
) -> String {
    let Some(line) = snapshot.last_line_limited(STDERR_REASON_BYTES) else {
        return reason;
    };
    reason.push_str("; stderr: ");
    reason.push_str(&line);
    reason
}

async fn renderer_failure_reason(
    reason: String,
    stderr: Option<&crate::wallframe::process_stdio::ChildStderrCapture>,
) -> String {
    match stderr {
        Some(stderr) => append_stderr_reason(reason, stderr.drain(STDERR_DRAIN_TIMEOUT).await),
        None => reason,
    }
}

fn renderer_exit_status(status: &ExitStatus) -> String {
    let hint = match status.code() {
        Some(127) => Some(
            "a runtime dependency or launcher command may be missing; check renderer stderr in the daemon logs",
        ),
        Some(126) => Some(
            "the renderer or a launcher command could not be executed; check renderer stderr in the daemon logs",
        ),
        _ => None,
    };

    let mut description = status.to_string();
    if status.core_dumped() {
        description.push_str("; core dumped");
    }
    if let Some(hint) = hint {
        description.push_str("; ");
        description.push_str(hint);
    }
    description
}

fn renderer_was_force_killed(status: Option<&ExitStatus>, force_requested: bool) -> bool {
    force_requested
        && status.is_none_or(|status| {
            status
                .signal()
                .is_some_and(|signal| signal == libc::SIGKILL)
        })
}

fn renderer_exit_failed(status: Option<&ExitStatus>) -> bool {
    status.is_some_and(|status| {
        status.signal().is_some() || status.code().is_some_and(|code| code != 0)
    })
}

async fn failed_renderer_process_status(child: &mut Child) -> String {
    match tokio::time::timeout(RENDERER_FAILED_EXIT_GRACE, child.wait()).await {
        Ok(Ok(status)) => format!("process_status={}", renderer_exit_status(&status)),
        Ok(Err(error)) => {
            let kill = child.start_kill().map_or_else(
                |kill_error| format!("kill failed: {kill_error}"),
                |()| "kill requested".to_string(),
            );
            format!("process_status=wait failed: {error}; {kill}")
        }
        Err(_) => {
            let kill = child.start_kill().map_or_else(
                |error| format!("kill failed: {error}"),
                |()| "killed after grace timeout".to_string(),
            );
            let status = match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                Ok(Ok(status)) => status.to_string(),
                Ok(Err(error)) => format!("wait failed: {error}"),
                Err(_) => "wait timed out after 2s".to_string(),
            };
            format!("process_status={status}; {kill}")
        }
    }
}

fn renderer_spawn_error_reason(error: Error) -> String {
    match error {
        Error::RendererSpawnFailed(reason) => reason,
        other => other.to_string(),
    }
}

async fn run_init_handshake_with_timeout(
    sock: &StdUnixStream,
    init: ControlMsg,
    timeout: Duration,
) -> Result<DrmNode> {
    let handshake_stream = sock.try_clone().context("try_clone for Init handshake")?;
    let mut handshake =
        tokio::task::spawn_blocking(move || run_init_handshake(&handshake_stream, &init));
    match tokio::time::timeout(timeout, &mut handshake).await {
        Ok(result) => result.context("init handshake join")?,
        Err(_) => {
            let _ = sock.shutdown(Shutdown::Both);
            if let Err(error) = handshake.await {
                log::warn!("renderer Init handshake worker failed after timeout: {error}");
            }
            Err(Error::RendererSpawnFailed(format!(
                "timed out waiting for renderer Ready after {timeout:?}"
            )))
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MprisSnapshot {
    pub state: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub art_url: String,
    pub previous_art_url: String,
}

impl Default for MprisSnapshot {
    fn default() -> Self {
        Self {
            state: 0,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            album_artist: String::new(),
            art_url: String::new(),
            previous_art_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SpawnRequest {
    /// The wallpaper type determines which renderer binary is spawned.
    pub wp_type: WallpaperType,
    /// CLI argv dictionary the daemon turns into `--<key> <value>`
    /// pairs after `--ipc <socket>`.
    pub extras: HashMap<String, String>,
    /// Plugin settings kv that flows directly into `Init.settings`.
    /// Callers usually source this from the reconciled settings store.
    pub settings: HashMap<String, String>,
    /// When true, pass `--test-pattern` to the renderer host, which
    /// lets test renderers bypass normal content loading.
    pub test_pattern: bool,
    /// Optional explicit renderer plugin name. `None` (default) lets
    /// `spawn` pick by type priority.
    pub renderer_name: Option<String>,
    /// Persisted renderer-owned property overrides. Daemon-owned layout
    /// keys are filtered out before spawn.
    pub user_property_overrides: HashMap<String, String>,
    /// Source-authored defaults for renderer-owned user properties.
    /// Persisted overrides take precedence when the Init payload is built.
    pub default_user_properties: HashMap<String, String>,
    /// Real (width, height) of this spawn's single target display, if known.
    pub display_size: Option<(u32, u32)>,
}

/// Immutable renderer publication created from one `BindBuffers` event.
/// References held by display events keep its DMA-BUF FDs alive even after
/// the renderer publishes a newer pool.
pub struct PublishedPool {
    /// Monotonically increasing per-renderer pool generation. Sourced
    /// from the renderer's `bind_buffers.generation` field.
    pub generation: u64,
    /// Placement flag set the renderer used when allocating this pool.
    /// Bit 0 = host_visible (GTT). See `BUF_HOST_VISIBLE`.
    pub flags: u32,
    pub count: u32,
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
    pub modifier: u64,
    pub planes_per_buffer: u32,
    /// `count * planes_per_buffer` entries, flattened (buffer, plane).
    pub stride: Vec<u32>,
    /// `count * planes_per_buffer` entries, flattened (buffer, plane).
    pub plane_offset: Vec<u32>,
    /// `count * planes_per_buffer` entries, flattened (buffer, plane).
    /// Per-plane memory span in bytes.
    pub size: Vec<u64>,
    /// `count * planes_per_buffer` entries, flattened (buffer, plane).
    /// Multi-plane modifiers may repeat the same underlying dma-buf fd.
    pub fds: Vec<OwnedFd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub buffer_generation: u64,
    pub buffer_index: u32,
    pub seq: u64,
    pub release_point: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct RendererProgressSnapshot {
    pub registered_at: Instant,
    pub bind_at: Option<Instant>,
    pub buffer_generation: Option<u64>,
    pub first_frame_at: Option<Instant>,
    pub last_frame_at: Option<Instant>,
}

struct RendererProgress {
    registered_at: Instant,
    bind_at: Option<Instant>,
    buffer_generation: Option<u64>,
    first_frame_at: Option<Instant>,
    last_frame_at: Option<Instant>,
}

impl RendererProgress {
    fn new() -> Self {
        Self {
            registered_at: Instant::now(),
            bind_at: None,
            buffer_generation: None,
            first_frame_at: None,
            last_frame_at: None,
        }
    }

    fn snapshot(&self) -> RendererProgressSnapshot {
        RendererProgressSnapshot {
            registered_at: self.registered_at,
            bind_at: self.bind_at,
            buffer_generation: self.buffer_generation,
            first_frame_at: self.first_frame_at,
            last_frame_at: self.last_frame_at,
        }
    }
}

/// Renderer event plus the pool generation that owned it when it was received.
/// `frame_ready` carries this relationship implicitly through event order, so
/// consumers must not reconstruct it from the latest published pool later.
#[derive(Clone, Debug)]
pub struct RendererEvent {
    pub message: EventMsg,
    pub pool_generation: Option<u64>,
    pub state_changed_fields: u32,
}

#[derive(Debug, Clone, PartialEq)]
struct RendererReportedState {
    clear_rgba: [f32; 4],
    runtime_tags: Vec<RendererRuntimeTag>,
}

impl Default for RendererReportedState {
    fn default() -> Self {
        Self {
            clear_rgba: [0.0, 0.0, 0.0, 1.0],
            runtime_tags: Vec::new(),
        }
    }
}

/// Bit 0 of `PublishedPool::flags` / `ControlMsg::ConfigureBuffers.flags`:
/// the renderer must back the dmabuf with HOST_VISIBLE memory.
pub const BUF_HOST_VISIBLE: u32 = 1 << 0;

/// DRM render-node identity reported by a renderer in its `Ready` event.
/// `(0, 0)` is the sentinel for an unknown render node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DrmNode {
    pub major: u32,
    pub minor: u32,
}

impl DrmNode {
    pub const UNKNOWN: Self = Self { major: 0, minor: 0 };
    pub fn is_known(&self) -> bool {
        self.major != 0 || self.minor != 0
    }
}

/// Upper bound on the number of per-seq sync_fd entries the reader
/// keeps around before evicting the oldest.
const SYNC_FD_RETENTION: usize = 16;

/// Per-renderer state. Cheap to clone via `Arc`; the inner fields are
/// shared across HTTP handlers and the reader thread.
pub struct RendererHandle {
    pub id: RendererId,
    pub process_generation: RendererProcessGeneration,
    pub wp_type: WallpaperType,
    /// The `SpawnRequest.extras` this renderer was started with —
    /// canonical resource path plus manifest-allowlisted keys.
    pub extras: HashMap<String, String>,
    /// Renderer plugin name from the resolved `RendererDef` (e.g.
    /// `"wescene"`). Surfaced to the UI as the renderer name.
    pub name: String,
    /// Domain id of the installable plugin that supplied this renderer.
    pub plugin_id: String,
    pub activity_mode: RendererActivityMode,
    /// OS pid of the renderer child captured right after `spawn()`.
    /// `None` only if Tokio could not return a child pid.
    pub pid: Option<u32>,
    /// Process group created for this renderer and inherited by normal
    /// helper processes. Audio ownership uses this boundary rather than
    /// renderer-controlled application names.
    pub process_group: Option<i32>,
    /// DRM render-node id of the GPU the renderer's Vulkan instance
    /// picked. Reported in Ready and used by DMA-BUF negotiation.
    pub gpu: DrmNode,
    spawn_request: SpawnRequest,

    /// Sole owner-facing path for daemon-to-renderer writes. A dedicated
    /// thread serializes reliable control traffic and latest-only audio.
    writer: RendererWriter,

    /// Broadcast of every event the host emits (besides the FDs on the
    /// initial BindBuffers, whose fds are stored in `published_pool`).
    events: broadcast::Sender<RendererEvent>,
    _release_events_tx: tokio::sync::mpsc::UnboundedSender<crate::wallframe::sync::ReleaseEvent>,
    release_events_rx: StdMutex<
        Option<tokio::sync::mpsc::UnboundedReceiver<crate::wallframe::sync::ReleaseEvent>>,
    >,
    progress: Arc<StdMutex<RendererProgress>>,

    /// Latest immutable pool published by the renderer.
    published_pool: Arc<StdMutex<Option<Arc<PublishedPool>>>>,

    /// In-flight `ConfigureBuffers` request. `Some(flags)` while the
    /// router has asked for a re-export not yet answered by BindBuffers.
    pending_configure: Arc<StdMutex<Option<u32>>>,

    /// Per-frame acquire fence file descriptors, indexed by `seq`.
    /// The reader thread stashes the fd attached to each FrameReady event.
    sync_fds: Arc<StdMutex<std::collections::VecDeque<(u64, OwnedFd)>>>,

    /// Most recent frame metadata, tied to the active bind generation.
    latest_frame: Arc<StdMutex<Option<FrameSnapshot>>>,

    /// Producer-exported timeline drm_syncobj used as the release
    /// fence target. Populated by a ReleaseSyncobj event.
    release_syncobj: Arc<StdMutex<Option<OwnedFd>>>,

    /// Modifier-negotiation capabilities the producer declared in
    /// its FormatCaps event.
    format_caps: Arc<StdMutex<Option<crate::wallframe::dma::negotiate::PeerCaps>>>,

    /// Last `NegotiatedScheme` the daemon dispatched via
    /// NegotiateBuffers to this renderer, used for idempotence.
    last_dispatched_scheme:
        Arc<StdMutex<Option<crate::wallframe::dma::negotiate::NegotiatedScheme>>>,

    /// Sink for frame registration and member completion events.
    frame_record_tx:
        Option<tokio::sync::mpsc::UnboundedSender<crate::wallframe::sync::FrameRecord>>,

    /// The child process. Kept alive so dropping the manager reaps it.
    child: Arc<TokioMutex<Option<Child>>>,
    stderr: Option<crate::wallframe::process_stdio::ChildStderrCapture>,

    /// Renderer-published state. Clear color and runtime tags are committed
    /// together so readers never observe a partially applied ReportState.
    reported_state: Arc<StdMutex<RendererReportedState>>,
}

impl RendererHandle {
    pub fn events(&self) -> broadcast::Receiver<RendererEvent> {
        self.events.subscribe()
    }

    pub fn take_release_events(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<crate::wallframe::sync::ReleaseEvent>> {
        self.release_events_rx
            .lock()
            .ok()
            .and_then(|mut receiver| receiver.take())
    }

    pub fn progress(&self) -> RendererProgressSnapshot {
        self.progress
            .lock()
            .map(|progress| progress.snapshot())
            .unwrap_or(RendererProgressSnapshot {
                registered_at: Instant::now(),
                bind_at: None,
                buffer_generation: None,
                first_frame_at: None,
                last_frame_at: None,
            })
    }

    pub fn spawn_request(&self) -> SpawnRequest {
        self.spawn_request.clone()
    }

    pub fn default_user_property(&self, key: &str) -> Option<String> {
        self.spawn_request
            .default_user_properties
            .get(crate::catalog::properties::canonical_user_property_key(key))
            .cloned()
    }

    pub fn frame_ready_seen(&self) -> bool {
        self.sync_fds.lock().map(|g| !g.is_empty()).unwrap_or(false)
    }

    pub fn latest_frame(&self) -> Option<FrameSnapshot> {
        let frame = self.latest_frame.lock().ok().and_then(|g| *g)?;
        let sync_fds = self.sync_fds.lock().ok()?;
        sync_fds
            .iter()
            .any(|(seq, _)| *seq == frame.seq)
            .then_some(frame)
    }

    /// Return the current immutable publication. Callers retain the exact
    /// pool and its FDs independently of later renderer publications.
    pub fn published_pool(&self) -> Option<Arc<PublishedPool>> {
        self.published_pool.lock().ok().and_then(|g| g.clone())
    }

    /// Actual texture dimensions reported by the renderer's most recent
    /// `BindBuffers`. Returns `(0, 0)` before the first BindBuffers.
    pub fn texture_size(&self) -> (u32, u32) {
        self.published_pool()
            .map(|pool| (pool.width, pool.height))
            .unwrap_or((0, 0))
    }

    /// Current placement flags from the latest `BindBuffers`, or 0 if
    /// no snapshot has arrived yet.
    pub fn current_flags(&self) -> u32 {
        self.published_pool().map(|pool| pool.flags).unwrap_or(0)
    }

    /// Whether a `ConfigureBuffers` request is currently in flight (sent
    /// to the renderer but not yet answered by BindBuffers).
    pub fn pending_configure(&self) -> Option<u32> {
        self.pending_configure.lock().ok().and_then(|g| *g)
    }

    /// Obtain a dup'd copy of the acquire sync_fd that arrived with
    /// `FrameReady` seq. Each caller gets an independent fd.
    pub fn clone_sync_fd(&self, seq: u64) -> Option<OwnedFd> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let guard = self.sync_fds.lock().ok()?;
        let (_, fd) = guard.iter().find(|(s, _)| *s == seq)?;
        let dup_raw = nix::unistd::dup(fd.as_raw_fd()).ok()?;
        // SAFETY: nix::unistd::dup returned a fresh fd we now own.
        Some(unsafe { OwnedFd::from_raw_fd(dup_raw) })
    }

    /// Borrow a dup'd handle to the producer's release timeline
    /// syncobj fd. Returns `None` until ReleaseSyncobj arrives.
    pub fn clone_release_syncobj_fd(&self) -> Option<OwnedFd> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let guard = self.release_syncobj.lock().ok()?;
        let fd = guard.as_ref()?;
        let dup_raw = nix::unistd::dup(fd.as_raw_fd()).ok()?;
        Some(unsafe { OwnedFd::from_raw_fd(dup_raw) })
    }

    /// Borrow a clone of the producer's declared modifier-negotiation
    /// capabilities. Returns `None` until FormatCaps arrives.
    pub fn format_caps(&self) -> Option<crate::wallframe::dma::negotiate::PeerCaps> {
        self.format_caps.lock().ok().and_then(|g| g.clone())
    }

    /// Mutate the producer's blacklist with `(fourcc, modifier)`. The
    /// blacklist lives inside the producer's cached PeerCaps.
    pub fn blacklist_format(&self, fourcc: u32, modifier: u64) -> bool {
        let Ok(mut guard) = self.format_caps.lock() else {
            return false;
        };
        let Some(caps) = guard.as_mut() else {
            return false;
        };
        caps.blacklist.insert((fourcc, modifier))
    }

    /// Most recently dispatched [`crate::wallframe::dma::negotiate::NegotiatedScheme`]
    /// for this renderer. `None` until a negotiation succeeds.
    pub fn current_scheme(&self) -> Option<crate::wallframe::dma::negotiate::NegotiatedScheme> {
        self.last_dispatched_scheme.lock().ok().and_then(|g| *g)
    }

    /// True iff `pool` matches the most recently dispatched scheme.
    pub fn scheme_satisfied_by(&self, pool: &PublishedPool) -> bool {
        let Some(scheme) = self.current_scheme() else {
            return false;
        };
        pool.fourcc == scheme.fourcc && pool.modifier == scheme.modifier
    }

    #[cfg(test)]
    pub fn test_publish_pool(&self, pool: PublishedPool) {
        *self.published_pool.lock().unwrap() = Some(Arc::new(pool));
    }

    pub fn register_frame_consumers(
        &self,
        identity: crate::wallframe::sync::FrameIdentity,
        consumers: Vec<crate::wallframe::sync::FrameConsumerIdentity>,
    ) -> std::result::Result<Vec<crate::wallframe::sync::FrameConsumerMember>, &'static str> {
        let Some(tx) = self.frame_record_tx.as_ref() else {
            return Err("no reaper wired (test stub or unconfigured renderer)");
        };
        crate::wallframe::sync::register_frame(tx, identity, consumers)
    }

    /// Renderer-published clear color (RGBA, 0..=1). Defaults to
    /// opaque black until the renderer reports state.
    pub fn clear_rgba(&self) -> [f32; 4] {
        self.reported_state
            .lock()
            .map(|state| state.clear_rgba)
            .unwrap_or([0.0, 0.0, 0.0, 1.0])
    }

    pub fn runtime_tags(&self) -> Vec<RendererRuntimeTag> {
        self.reported_state
            .lock()
            .map(|state| state.runtime_tags.clone())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Manager

pub struct RendererManager {
    inner: TokioMutex<Inner>,
    /// Plugin registry mapping wallpaper types to renderer binaries.
    registry: StdRwLock<RendererRegistry>,
    /// Cached system snapshot from startup. Used at spawn time to resolve
    /// GPU selections into render-node paths.
    system_info: OnceLock<Arc<crate::system::SystemInfo>>,
    settings: OnceLock<Arc<SettingsStore>>,
    /// Dead-renderer signals queue here (from reader-thread exit or
    /// a send_control hitting EPIPE). One background task drains it.
    reap_tx: tokio::sync::mpsc::UnboundedSender<(RendererId, RendererProcessGeneration)>,
    reap_rx: StdMutex<
        Option<tokio::sync::mpsc::UnboundedReceiver<(RendererId, RendererProcessGeneration)>>,
    >,
    process_exit_tx: tokio::sync::mpsc::UnboundedSender<RendererProcessExit>,
    process_exit_rx: StdMutex<Option<tokio::sync::mpsc::UnboundedReceiver<RendererProcessExit>>>,
    reaper_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    subscriptions: Arc<RendererSubscriptionRegistry>,
    process_ownership: watch::Sender<RendererProcessOwnershipSnapshot>,
    process_ownership_state: StdMutex<RendererProcessOwnershipState>,
    next_process_generation: std::sync::atomic::AtomicU64,
    renderer_log: StdMutex<RendererLogSnapshot>,
    renderer_log_env_active: bool,
    renderer_log_update: TokioMutex<()>,
}

struct Inner {
    renderers: HashMap<RendererId, Arc<RendererHandle>>,
}

#[derive(Default)]
struct RendererProcessOwnershipState {
    generation: u64,
    process_groups: BTreeSet<i32>,
}

struct RendererProcessGroupRegistration<'a> {
    manager: &'a RendererManager,
    process_group: Option<i32>,
}

impl RendererProcessGroupRegistration<'_> {
    fn commit(mut self) {
        self.process_group = None;
    }
}

impl Drop for RendererProcessGroupRegistration<'_> {
    fn drop(&mut self) {
        self.manager.unregister_process_group(self.process_group);
    }
}

impl RendererManager {
    pub fn new(registry: RendererRegistry) -> Self {
        let (reap_tx, reap_rx) = tokio::sync::mpsc::unbounded_channel();
        let (process_exit_tx, process_exit_rx) = tokio::sync::mpsc::unbounded_channel();
        let (process_ownership, _) = watch::channel(RendererProcessOwnershipSnapshot::default());
        let (renderer_log_level, renderer_log_env_active) = initial_renderer_log_level();
        Self {
            inner: TokioMutex::new(Inner {
                renderers: HashMap::new(),
            }),
            registry: StdRwLock::new(registry),
            system_info: OnceLock::new(),
            settings: OnceLock::new(),
            reap_tx,
            reap_rx: StdMutex::new(Some(reap_rx)),
            process_exit_tx,
            process_exit_rx: StdMutex::new(Some(process_exit_rx)),
            reaper_task: StdMutex::new(None),
            subscriptions: Arc::new(RendererSubscriptionRegistry::new()),
            process_ownership,
            process_ownership_state: StdMutex::new(RendererProcessOwnershipState::default()),
            next_process_generation: std::sync::atomic::AtomicU64::new(0),
            renderer_log: StdMutex::new(RendererLogSnapshot {
                level: renderer_log_level,
                revision: 1,
            }),
            renderer_log_env_active,
            renderer_log_update: TokioMutex::new(()),
        }
    }

    /// Hand the manager the startup system snapshot used for renderer policy.
    pub fn attach_system_info(&self, system_info: Arc<crate::system::SystemInfo>) {
        let _ = self.system_info.set(system_info);
    }

    /// Wire the live settings store used by outbound renderer policy gates.
    pub fn attach_settings(&self, settings: Arc<SettingsStore>) {
        let _ = self.settings.set(settings);
    }

    fn renderer_log_snapshot(&self) -> RendererLogSnapshot {
        *self
            .renderer_log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub async fn initialize_renderer_log_level(&self, debug_enabled: bool) {
        if self.renderer_log_env_active {
            log::info!(
                "renderer log level initialized from WW_LOG={}",
                log_level_name(self.renderer_log_snapshot().level)
            );
            return;
        }
        self.set_renderer_log_level(if debug_enabled {
            LogLevel::Debug
        } else {
            LogLevel::Info
        })
        .await;
    }

    pub async fn set_renderer_debug_logging(&self, enabled: bool) {
        self.set_renderer_log_level(if enabled {
            LogLevel::Debug
        } else {
            LogLevel::Info
        })
        .await;
    }

    async fn set_renderer_log_level(&self, level: LogLevel) {
        let _update = self.renderer_log_update.lock().await;
        {
            let mut state = self
                .renderer_log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.level == level {
                return;
            }
            state.level = level;
            state.revision = state.revision.wrapping_add(1).max(1);
        }

        let handles = {
            let inner = self.inner.lock().await;
            inner.renderers.values().cloned().collect::<Vec<_>>()
        };
        for handle in handles {
            if let Err(error) = self
                .send_control_to_handle(&handle.id, &handle, ControlMsg::SetLogLevel { level })
                .await
            {
                log::warn!(
                    "renderer {}: failed to set log level to {}: {error}",
                    handle.id,
                    log_level_name(level)
                );
            }
        }
        log::info!("renderer log level set to {}", log_level_name(level));
    }

    pub fn take_process_exits(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<RendererProcessExit>> {
        self.process_exit_rx
            .lock()
            .ok()
            .and_then(|mut receiver| receiver.take())
    }

    /// Start the background reaper task that drains `mark_dead`
    /// signals and runs async eviction.
    pub fn start_reaper(self: &Arc<Self>) {
        let rx = match self.reap_rx.lock() {
            Ok(mut g) => g.take(),
            Err(_) => return,
        };
        let Some(mut rx) = rx else { return };
        let this = Arc::clone(self);
        let task = tokio::spawn(async move {
            while let Some((id, generation)) = rx.recv().await {
                this.evict(&id, generation).await;
            }
        });
        if let Ok(mut slot) = self.reaper_task.lock() {
            *slot = Some(task);
        }
    }

    pub async fn shutdown(self: &Arc<Self>) {
        let reaper_task = self
            .reaper_task
            .lock()
            .ok()
            .and_then(|mut task| task.take());
        if let Some(task) = reaper_task {
            task.abort();
            let _ = task.await;
        }

        let renderer_ids = self.list().await;
        for id in renderer_ids {
            match self.stop(&id).await {
                Ok(exit) => {
                    let _ = self.process_exit_tx.send(exit);
                }
                Err(error) => {
                    log::warn!("renderer {id}: shutdown failed: {error}");
                }
            }
        }
    }

    /// Test-only convenience: construct a manager whose registry has a
    /// single scene renderer when `$WAYWALLEN_RENDERER_BIN` is set.
    pub fn new_default() -> Self {
        let mut registry = RendererRegistry::new();
        if let Some(bin) = std::env::var_os("WAYWALLEN_RENDERER_BIN") {
            registry.register(RendererDef {
                name: "test-scene".to_string(),
                plugin_id: "test.plugin".to_string(),
                plugin_version: "0.0.0".to_string(),
                plugin_system: false,
                bin: PathBuf::from(bin),
                types: vec!["scene".to_string()],
                priority: 100,
                activity: RendererActivityMode::Continuous,
                spawn_version: None,
                extras: Vec::new(),
                settings: Default::default(),
                legacy_events: None,
            });
        }
        Self::new(registry)
    }

    pub fn with_registry<T>(&self, f: impl FnOnce(&RendererRegistry) -> T) -> T {
        let registry = self.registry.read().expect("renderer registry poisoned");
        f(&registry)
    }

    pub fn registry_snapshot(&self) -> RendererRegistry {
        self.with_registry(Clone::clone)
    }

    pub fn replace_registry(&self, registry: RendererRegistry) {
        *self.registry.write().expect("renderer registry poisoned") = registry;
    }

    /// Spawn a fresh renderer-host subprocess, wait for its `Ready`
    /// event, and return its id. Cleans up the child on failure.
    pub async fn spawn(&self, req: SpawnRequest) -> Result<RendererId> {
        let id: RendererId = Uuid::new_v4().to_string();
        let process_generation = self.reserve_process_generation();
        self.spawn_for_generation(id.clone(), process_generation, req)
            .await?;
        Ok(id)
    }

    pub fn reserve_process_generation(&self) -> RendererProcessGeneration {
        self.next_process_generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    pub async fn spawn_for_generation(
        &self,
        id: RendererId,
        process_generation: RendererProcessGeneration,
        mut req: SpawnRequest,
    ) -> Result<()> {
        if self.get(&id).await.is_some() {
            return Err(Error::InvalidArgument(format!(
                "renderer '{id}' already has a live process"
            )));
        }
        req.default_user_properties =
            crate::catalog::properties::normalize_renderer_user_properties(
                req.default_user_properties,
            );

        let renderer_def = match req.renderer_name.as_deref() {
            Some(name) => self
                .with_registry(|registry| registry.resolve_by_name(name).cloned())
                .ok_or_else(|| Error::RendererNotFound(name.to_string()))?,
            None => self
                .with_registry(|registry| registry.resolve(&req.wp_type).cloned())
                .ok_or_else(|| Error::NoRendererForType(req.wp_type.clone()))?,
        };
        validate_renderer_spawn_version(&renderer_def)?;

        // Create a listening UDS at a temp path; the child connects to
        // it shortly after exec().
        let sock_path = temp_sock_path(&id);
        let _ = std::fs::remove_file(&sock_path);
        let listener = tokio::net::UnixListener::bind(&sock_path)
            .with_context(|| format!("bind {}", sock_path.display()))?;

        // Best-effort cleanup of the socket file at the end of spawn —
        // the connection survives unlink(2).
        let _cleanup = TempUnlink(sock_path.clone());

        // Translate the user's GPU choice into a render-node path before
        // settings reach the subprocess.
        if let Some(raw) = req.settings.remove(crate::system::GPU_DRM_DEV_KEY) {
            let resolved = self
                .system_info
                .get()
                .and_then(|system| system.render_node_for_drm_dev(&raw))
                .and_then(|path| path.to_str().map(str::to_owned));
            if let Some(path) = resolved {
                req.settings
                    .insert(crate::system::RENDER_NODE_KEY.to_string(), path);
            } else {
                log::warn!(
                    "spawn: gpu_drm_dev={raw} is invalid or unavailable; \
                     dropping selection and letting renderer pick default"
                );
            }
        }

        // Build the Init message before spawning the child.
        let init_msg = build_init_msg(&req, &renderer_def);
        let renderer_log = self.renderer_log_snapshot();

        let mut cmd = Command::new(&renderer_def.bin);
        cmd.process_group(0);
        cmd.arg("--ipc").arg(&sock_path);
        cmd.env("WW_LOG", log_level_name(renderer_log.level));
        // SPAWN_VERSION 3: extras (canonical `path` + plugin-specific
        // keys like `assets`/`external_id`) ride as `--<key> <value>`
        let mut extra_keys: Vec<&String> = req.extras.keys().collect();
        extra_keys.sort();
        for k in extra_keys {
            if k != "path" && !renderer_def.extras.iter().any(|w| w == k) {
                continue;
            }
            cmd.arg(format!("--{k}")).arg(&req.extras[k]);
        }
        cmd.kill_on_drop(true)
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {}", renderer_def.bin.display()))?;
        let child_pid = child.id();
        let stderr = child.stderr.take().map(|stderr| {
            crate::wallframe::process_stdio::ChildStderrCapture::spawn(
                stderr,
                renderer_process_label(&renderer_def.name, child_pid, &id),
            )
        });
        let process_group = child_pid.and_then(|pid| i32::try_from(pid).ok());
        let process_group_registration = self.register_process_group(process_group);

        // A dynamic-loader or early initialization failure happens after
        // exec succeeds, so observe the child while waiting for IPC.
        let connect_timeout = tokio::time::sleep(RENDERER_CONNECT_TIMEOUT);
        tokio::pin!(connect_timeout);
        let (tokio_stream, _addr) = tokio::select! {
            accepted = listener.accept() => accepted.context("accept")?,
            status = child.wait() => {
                let status = status.context("wait for renderer before IPC connect")?;
                let reason = renderer_failure_reason(format!(
                    "renderer exited before IPC connect: {}",
                    renderer_exit_status(&status),
                ), stderr.as_ref()).await;
                return Err(Error::RendererSpawnFailed(reason));
            }
            () = &mut connect_timeout => {
                let _ = child.start_kill();
                let process_status = failed_renderer_process_status(&mut child).await;
                let reason = renderer_failure_reason(format!(
                    "timed out waiting for waywallen-renderer to connect back; {process_status}"
                ), stderr.as_ref()).await;
                return Err(Error::RendererSpawnFailed(reason));
            }
        };

        // Convert to a blocking std UnixStream for the rest of the
        // lifecycle because ipc::uds uses blocking sendmsg/recvmsg.
        let std_stream = tokio_stream.into_std().context("UnixStream::into_std")?;
        std_stream
            .set_nonblocking(false)
            .context("clear O_NONBLOCK on accepted stream")?;

        // Emit typed Init right after accept; CLI extras only identify
        // launch resources now.
        let handshake =
            run_init_handshake_with_timeout(&std_stream, init_msg, RENDERER_INIT_TIMEOUT).await;
        let gpu = match handshake {
            Ok(gpu) => gpu,
            Err(error) => {
                let reason = renderer_spawn_error_reason(error);
                drop(std_stream);
                let process_status = failed_renderer_process_status(&mut child).await;
                let reason = renderer_failure_reason(
                    format!(
                        "{reason}; renderer={} pid={}; {process_status}",
                        renderer_def.name,
                        child_pid.map_or_else(|| "unknown".to_string(), |pid| pid.to_string()),
                    ),
                    stderr.as_ref(),
                )
                .await;
                return Err(Error::RendererSpawnFailed(reason));
            }
        };
        log::info!(
            "renderer {id}: Ready (drm_render={}:{})",
            gpu.major,
            gpu.minor
        );

        // Now wire up the permanent reader thread and store the handle.
        let (events_tx, _events_rx) = broadcast::channel::<RendererEvent>(256);
        let (release_events_tx, release_events_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::wallframe::sync::ReleaseEvent>();
        let published_pool: Arc<StdMutex<Option<Arc<PublishedPool>>>> =
            Arc::new(StdMutex::new(None));
        let sync_fds: Arc<StdMutex<std::collections::VecDeque<(u64, OwnedFd)>>> =
            Arc::new(StdMutex::new(std::collections::VecDeque::new()));
        let latest_frame: Arc<StdMutex<Option<FrameSnapshot>>> = Arc::new(StdMutex::new(None));
        let release_syncobj: Arc<StdMutex<Option<OwnedFd>>> = Arc::new(StdMutex::new(None));
        let format_caps: Arc<StdMutex<Option<crate::wallframe::dma::negotiate::PeerCaps>>> =
            Arc::new(StdMutex::new(None));
        let pending_configure: Arc<StdMutex<Option<u32>>> = Arc::new(StdMutex::new(None));
        let reported_state = Arc::new(StdMutex::new(RendererReportedState::default()));
        let progress = Arc::new(StdMutex::new(RendererProgress::new()));

        let reader_stream = std_stream
            .try_clone()
            .context("try_clone for renderer reader")?;
        self.subscriptions.register(id.clone());
        let writer = RendererWriter::spawn(
            id.clone(),
            process_generation,
            std_stream,
            Arc::clone(&self.subscriptions),
            self.reap_tx.clone(),
        );
        let reader_writer = writer.clone();
        let reader_subscriptions = Arc::clone(&self.subscriptions);
        let reader_events = events_tx.clone();
        let reader_pool = published_pool.clone();
        let reader_sync_fds = sync_fds.clone();
        let reader_latest_frame = latest_frame.clone();
        let reader_release_syncobj = release_syncobj.clone();
        let reader_format_caps = format_caps.clone();
        let reader_pending = pending_configure.clone();
        let reader_reported_state = Arc::clone(&reported_state);
        let reader_progress = Arc::clone(&progress);
        let reader_id = id.clone();
        let reader_reap_tx = self.reap_tx.clone();

        // Per-renderer reaper drains FrameRecords and transfers consumer
        // release fences onto the producer timeline.
        let (frame_tx, frame_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::wallframe::sync::FrameRecord>();
        let frame_record_tx = match crate::wallframe::sync::drm_device() {
            Ok(_) => Some(frame_tx),
            Err(e) => {
                log::warn!(
                    "renderer {id}: no DRM render node ({e}); release-syncobj reaper disabled"
                );
                None
            }
        };

        let handle = Arc::new(RendererHandle {
            id: id.clone(),
            process_generation,
            wp_type: req.wp_type.clone(),
            extras: req.extras.clone(),
            name: renderer_def.name.clone(),
            plugin_id: renderer_def.plugin_id.clone(),
            activity_mode: renderer_def.activity,
            pid: child_pid,
            process_group,
            gpu,
            spawn_request: req.clone(),
            writer,
            events: events_tx,
            _release_events_tx: release_events_tx.clone(),
            release_events_rx: StdMutex::new(Some(release_events_rx)),
            progress,
            published_pool,
            sync_fds,
            latest_frame,
            release_syncobj,
            format_caps,
            last_dispatched_scheme: Arc::new(StdMutex::new(None)),
            frame_record_tx,
            pending_configure,
            child: Arc::new(TokioMutex::new(Some(child))),
            stderr,
            reported_state,
        });

        if handle.frame_record_tx.is_some() {
            // SAFETY: drm_device() returned Ok above and is idempotent.
            let drm = crate::wallframe::sync::drm_device().expect("checked above");
            // Pass only the renderer id and release_syncobj; the reaper
            // must not keep the whole RendererHandle alive.
            crate::wallframe::sync::spawn_reaper(
                drm,
                id.clone(),
                Arc::clone(&handle.release_syncobj),
                frame_rx,
                release_events_tx,
            );
        }

        {
            let mut inner = self.inner.lock().await;
            inner.renderers.insert(id.clone(), Arc::clone(&handle));
        }
        {
            let _update = self.renderer_log_update.lock().await;
            let current = self.renderer_log_snapshot();
            if current.revision != renderer_log.revision {
                if let Err(error) = self
                    .send_control_to_handle(
                        &id,
                        &handle,
                        ControlMsg::SetLogLevel {
                            level: current.level,
                        },
                    )
                    .await
                {
                    log::warn!(
                        "renderer {id}: failed to reconcile log level to {}: {error}",
                        log_level_name(current.level)
                    );
                }
            }
        }
        process_group_registration.commit();
        thread::spawn(move || {
            run_reader(
                reader_id,
                process_generation,
                reader_stream,
                reader_writer,
                reader_subscriptions,
                reader_events,
                reader_pool,
                reader_sync_fds,
                reader_latest_frame,
                reader_release_syncobj,
                reader_format_caps,
                reader_pending,
                reader_reported_state,
                reader_progress,
                reader_reap_tx,
            );
        });
        log::info!("spawned renderer {id} ({})", req.wp_type);
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Option<Arc<RendererHandle>> {
        let inner = self.inner.lock().await;
        inner.renderers.get(id).cloned()
    }

    pub async fn list(&self) -> Vec<RendererId> {
        let inner = self.inner.lock().await;
        inner.renderers.keys().cloned().collect()
    }

    pub async fn live_renderer_ids_by_plugin_id(&self, plugin_id: &str) -> Vec<RendererId> {
        let inner = self.inner.lock().await;
        inner
            .renderers
            .iter()
            .filter_map(|(id, handle)| (handle.plugin_id == plugin_id).then(|| id.clone()))
            .collect()
    }

    pub(super) async fn wait_for_first_frame_generation(
        &self,
        id: &str,
        process_generation: RendererProcessGeneration,
        timeout: Duration,
    ) -> Result<()> {
        let handle = self
            .get(id)
            .await
            .filter(|handle| handle.process_generation == process_generation)
            .ok_or_else(|| Error::RendererNotFound(id.to_string()))?;
        if handle.frame_ready_seen() {
            return Ok(());
        }

        let mut events = handle.events();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let mut liveness = tokio::time::interval(Duration::from_millis(100));

        loop {
            if handle.frame_ready_seen() {
                return Ok(());
            }

            tokio::select! {
                _ = &mut deadline => {
                    return Err(Error::RendererFrameFailed(format!(
                        "timed out after {}s waiting for renderer '{id}' to send its first frame",
                        timeout.as_secs()
                    )));
                }
                _ = liveness.tick() => {
                    if !self.get(id).await.is_some_and(|current| {
                        current.process_generation == process_generation
                    }) {
                        return Err(Error::RendererFrameFailed(format!(
                            "renderer '{id}' generation {process_generation} exited before its first frame"
                        )));
                    }

                    let mut child_guard = handle.child.lock().await;
                    if let Some(child) = child_guard.as_mut() {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                self.mark_dead_generation(id, handle.process_generation);
                                let reason = format!(
                                    "renderer '{id}' exited before its first frame: {status}"
                                );
                                let reason = handle.stderr.as_ref().map_or(reason.clone(), |stderr| {
                                    append_stderr_reason(reason, stderr.snapshot())
                                });
                                return Err(Error::RendererFrameFailed(reason));
                            }
                            Ok(None) => {}
                            Err(e) => {
                                return Err(Error::RendererFrameFailed(format!(
                                    "failed to check renderer '{id}' liveness: {e}"
                                )));
                            }
                        }
                    }
                }
                recv = events.recv() => {
                    match recv {
                        Ok(event) if matches!(event.message, EventMsg::FrameReady { .. }) => {
                            return Ok(())
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            if handle.frame_ready_seen() {
                                return Ok(());
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return Err(Error::RendererFrameFailed(format!(
                                "renderer '{id}' event stream closed before its first frame"
                            )));
                        }
                    }
                }
            }
        }
    }

    /// Fire-and-forget control send. Returns an error if the renderer
    /// is unknown or the underlying socket write fails.
    pub async fn send_control(&self, id: &str, msg: ControlMsg) -> Result<()> {
        let handle = self
            .get(id)
            .await
            .ok_or_else(|| Error::RendererNotFound(id.to_string()))?;
        self.send_control_to_handle(id, &handle, msg).await
    }

    async fn send_control_to_handle(
        &self,
        id: &str,
        handle: &RendererHandle,
        msg: ControlMsg,
    ) -> Result<()> {
        let writer = handle.writer.clone();
        let write = tokio::task::spawn_blocking(move || writer.send_blocking(msg, None))
            .await
            .context("send_control join")?;
        write.map_err(|error| {
            self.mark_dead_generation(id, handle.process_generation);
            Error::RendererControlFailed(format!("send_control: {error}"))
        })
    }

    /// Ask a renderer to publish its current content once. The renderer
    /// may satisfy this with its next normal frame or republish the latest
    /// released slot without advancing content time.
    pub async fn request_frame(&self, id: &str) -> Result<()> {
        log::debug!("renderer {id}: request current frame");
        self.send_control(id, ControlMsg::RequestFrame).await
    }

    /// Dispatch the complete buffer allocation decision.
    pub async fn send_negotiate_buffers(
        &self,
        id: &str,
        scheme: crate::wallframe::dma::negotiate::NegotiatedScheme,
    ) -> Result<()> {
        let handle = self
            .get(id)
            .await
            .ok_or_else(|| Error::RendererNotFound(id.to_string()))?;
        // Idempotence: skip if we've already dispatched this exact scheme.
        if let Ok(guard) = handle.last_dispatched_scheme.lock() {
            if guard.as_ref() == Some(&scheme) {
                return Ok(());
            }
        }
        log::info!(
            "renderer {id}: NegotiateBuffers fourcc=0x{:08x} modifier=0x{:x} \
             plane_count={} sync=0x{:x} color=0x{:x} mem_hint=0x{:x} \
             count={} path={:?} mem_source={:?}",
            scheme.fourcc,
            scheme.modifier,
            scheme.plane_count,
            scheme.sync_mode,
            scheme.color,
            scheme.mem_hint,
            scheme.count,
            scheme.path,
            scheme.mem_source,
        );
        let msg = ControlMsg::NegotiateBuffers {
            directive: BufferDirective {
                format: BufferFormat {
                    fourcc: scheme.fourcc,
                    modifier: scheme.modifier,
                    plane_count: scheme.plane_count,
                },
                sync_mode: scheme.sync_mode,
                color: scheme.color,
                mem_hint: scheme.mem_hint,
                count: scheme.count,
                path: match scheme.path {
                    crate::wallframe::dma::negotiate::PathCategory::OptimizedSameDevice => {
                        BufferPath::OptimizedSameDevice
                    }
                    crate::wallframe::dma::negotiate::PathCategory::OptimizedSameVendor => {
                        BufferPath::OptimizedSameVendor
                    }
                    crate::wallframe::dma::negotiate::PathCategory::CompatLinear => {
                        BufferPath::CompatLinear
                    }
                    crate::wallframe::dma::negotiate::PathCategory::CompatCpuReadback => {
                        BufferPath::CompatCpuReadback
                    }
                },
                memory_source: match scheme.mem_source {
                    crate::wallframe::dma::negotiate::MemSource::GpuNative => {
                        BufferMemorySource::GpuNative
                    }
                    crate::wallframe::dma::negotiate::MemSource::GpuLinear => {
                        BufferMemorySource::GpuLinear
                    }
                    crate::wallframe::dma::negotiate::MemSource::DmabufHeap => {
                        BufferMemorySource::DmabufHeap
                    }
                },
            },
        };
        self.send_control(id, msg).await?;
        if let Ok(mut guard) = handle.last_dispatched_scheme.lock() {
            *guard = Some(scheme);
        }
        Ok(())
    }

    /// Push a `setting_changed` event to a live renderer. `settings` is
    /// the caller-filtered runtime delta.
    pub async fn send_setting_changed(
        &self,
        id: &str,
        settings: Vec<(String, String)>,
        fps: Option<u32>,
    ) -> Result<()> {
        let handle = self
            .get(id)
            .await
            .ok_or_else(|| Error::RendererNotFound(id.to_string()))?;
        // setting_changed is a pure kv list. fps is just one of the kv
        // keys (when the manifest declares it), not a typed scalar.
        let mut settings = settings;
        if let Some(f) = fps {
            if f != 0 {
                settings.retain(|(k, _)| k != "fps");
                settings.push(("fps".to_string(), f.to_string()));
            }
        }
        let msg = ControlMsg::SettingChanged {
            settings: settings.clone(),
        };
        log::info!(
            "renderer {id}: setting_changed keys={:?}",
            settings.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        );
        self.send_control(id, msg).await?;
        let _ = handle;
        Ok(())
    }

    /// Forward a pointer-motion event to a live renderer. Silently
    /// drops when the renderer did not subscribe to pointer events.
    pub async fn send_pointer_motion(&self, id: &str, event: PointerMotion) -> Result<()> {
        if !self.pointer_forwarding_enabled() {
            return Ok(());
        }
        if !self.subscribed_to(id, RendererEventKind::Pointer) {
            return Ok(());
        }
        self.send_control(id, ControlMsg::PointerMotion { event })
            .await
    }

    /// Forward a pointer-button event. Same gating as
    /// [`Self::send_pointer_motion`].
    pub async fn send_pointer_button(&self, id: &str, event: PointerButton) -> Result<()> {
        if !self.pointer_forwarding_enabled() {
            return Ok(());
        }
        if !self.subscribed_to(id, RendererEventKind::Pointer) {
            return Ok(());
        }
        self.send_control(id, ControlMsg::PointerButton { event })
            .await
    }

    /// Forward a pointer-axis (scroll) event. Same gating as
    /// [`Self::send_pointer_motion`].
    pub async fn send_pointer_axis(&self, id: &str, event: PointerAxis) -> Result<()> {
        if !self.pointer_forwarding_enabled() {
            return Ok(());
        }
        if !self.subscribed_to(id, RendererEventKind::Pointer) {
            return Ok(());
        }
        self.send_control(id, ControlMsg::PointerAxis { event })
            .await
    }

    fn pointer_forwarding_enabled(&self) -> bool {
        self.settings
            .get()
            .is_none_or(|settings| settings.pointer_forwarding_enabled())
    }

    /// Forward an MPRIS media snapshot to a live renderer. Silently
    /// drops when the renderer did not subscribe to MPRIS events.
    pub async fn send_mpris(&self, id: &str, snapshot: MprisSnapshot) -> Result<()> {
        let Some(handle) = self.get(id).await else {
            return Ok(());
        };
        if !self.subscribed_to(id, RendererEventKind::Mpris) {
            return Ok(());
        }
        self.send_control_to_handle(
            id,
            &handle,
            ControlMsg::Mpris {
                snapshot: WireMprisSnapshot {
                    state: match snapshot.state {
                        1 => MediaPlaybackState::Playing,
                        2 => MediaPlaybackState::Paused,
                        _ => MediaPlaybackState::Stopped,
                    },
                    title: snapshot.title,
                    artist: snapshot.artist,
                    album: snapshot.album,
                    album_artist: snapshot.album_artist,
                    art_url: snapshot.art_url,
                    previous_art_url: snapshot.previous_art_url,
                },
            },
        )
        .await
    }

    pub fn subscription_snapshot(&self) -> RendererSubscriptionSnapshot {
        self.subscriptions.snapshot()
    }

    pub fn subscribe_subscriptions(&self) -> watch::Receiver<RendererSubscriptionSnapshot> {
        self.subscriptions.subscribe()
    }

    pub fn subscribe_process_ownership(&self) -> watch::Receiver<RendererProcessOwnershipSnapshot> {
        self.process_ownership.subscribe()
    }

    fn register_process_group(
        &self,
        process_group: Option<i32>,
    ) -> RendererProcessGroupRegistration<'_> {
        if let Some(process_group) = process_group {
            self.update_process_group(process_group, true);
        }
        RendererProcessGroupRegistration {
            manager: self,
            process_group,
        }
    }

    fn unregister_process_group(&self, process_group: Option<i32>) {
        if let Some(process_group) = process_group {
            self.update_process_group(process_group, false);
        }
    }

    fn update_process_group(&self, process_group: i32, present: bool) {
        let mut state = self
            .process_ownership_state
            .lock()
            .expect("renderer process ownership poisoned");
        let changed = if present {
            state.process_groups.insert(process_group)
        } else {
            state.process_groups.remove(&process_group)
        };
        if !changed {
            return;
        }
        state.generation = state.generation.wrapping_add(1);
        self.process_ownership
            .send_replace(RendererProcessOwnershipSnapshot {
                generation: state.generation,
                process_groups: Arc::new(state.process_groups.clone()),
            });
    }

    fn subscribed_to(&self, id: &str, kind: RendererEventKind) -> bool {
        self.subscription_snapshot()
            .revision_for(id, kind)
            .is_some()
    }

    pub async fn send_audio_window_latest(&self, id: &str, window: AudioWindow) -> Result<()> {
        let handle = self
            .get(id)
            .await
            .ok_or_else(|| Error::RendererNotFound(id.to_string()))?;
        if self
            .subscription_snapshot()
            .revision_for(id, RendererEventKind::Audio)
            != Some(window.subscription_revision)
        {
            return Ok(());
        }
        handle
            .writer
            .replace_audio(
                window.subscription_revision,
                ControlMsg::AudioWindow { window },
            )
            .map_err(|error| Error::RendererControlFailed(format!("send audio: {error}")))
    }

    /// Enqueue a renderer for eviction. Synchronous (cheap channel
    /// send); cleanup happens on the reaper task.
    fn mark_dead_generation(&self, id: &str, process_generation: RendererProcessGeneration) {
        if self
            .reap_tx
            .send((id.to_string(), process_generation))
            .is_err()
        {
            log::warn!("renderer {id}: mark_dead dropped (reaper channel closed)");
        }
    }

    /// Remove the matching live generation, reap its child, and notify
    /// the lifecycle owner. Called only by the reaper task and is idempotent.
    async fn evict(self: &Arc<Self>, id: &str, process_generation: RendererProcessGeneration) {
        let handle = {
            let mut inner = self.inner.lock().await;
            match inner.renderers.get(id) {
                Some(handle) if handle.process_generation == process_generation => {
                    inner.renderers.remove(id)
                }
                _ => None,
            }
        };
        let Some(handle) = handle else { return };
        self.unregister_process_group(handle.process_group);
        self.subscriptions.remove(id);
        log::warn!("renderer {id}: evicting");

        let mut child_guard = handle.child.lock().await;
        let mut exit = None;
        let mut force_kill_requested = false;
        let mut wait_error = None;
        if let Some(mut child) = child_guard.take() {
            exit = child.try_wait().ok().flatten();
            if exit.is_none() {
                match tokio::time::timeout(RENDERER_FAILED_EXIT_GRACE, child.wait()).await {
                    Ok(Ok(status)) => exit = Some(status),
                    Ok(Err(error)) => wait_error = Some(error.to_string()),
                    Err(_) => {
                        force_kill_requested = child.start_kill().is_ok();
                        exit = tokio::time::timeout(Duration::from_secs(2), child.wait())
                            .await
                            .ok()
                            .and_then(std::result::Result::ok);
                    }
                }
            }
        }
        let force_killed = renderer_was_force_killed(exit.as_ref(), force_kill_requested);
        let reason = exit.as_ref().map(renderer_exit_status).unwrap_or_else(|| {
            if force_killed {
                "renderer IPC closed; process was killed".to_string()
            } else if let Some(error) = wait_error {
                format!("renderer IPC closed; wait failed: {error}")
            } else {
                "renderer IPC closed".to_string()
            }
        });
        let reason = renderer_failure_reason(reason, handle.stderr.as_ref()).await;
        let event = RendererProcessExit {
            renderer_id: id.to_string(),
            process_generation,
            kind: if force_killed {
                RendererProcessExitKind::Killed
            } else {
                RendererProcessExitKind::Failed
            },
            code: exit.as_ref().and_then(ExitStatus::code),
            signal: exit.as_ref().and_then(ExitStatusExt::signal),
            reason,
        };
        if self.process_exit_tx.send(event).is_err() {
            log::warn!("renderer {id}: process exit dropped (owner channel closed)");
        }
    }

    /// Send Shutdown, wait for the child to exit gracefully, escalate
    /// to SIGKILL only if it doesn't. Removes from the map.
    pub async fn kill(&self, id: &str) -> Result<()> {
        self.stop(id).await.map(|_| ())
    }

    pub async fn stop(&self, id: &str) -> Result<RendererProcessExit> {
        let handle = {
            let mut inner = self.inner.lock().await;
            inner.renderers.remove(id)
        }
        .ok_or_else(|| Error::RendererNotFound(id.to_string()))?;
        self.stop_handle(id, handle).await
    }

    pub async fn stop_generation(
        &self,
        id: &str,
        process_generation: RendererProcessGeneration,
    ) -> Result<Option<RendererProcessExit>> {
        let handle = {
            let mut inner = self.inner.lock().await;
            match inner.renderers.get(id) {
                Some(handle) if handle.process_generation == process_generation => {
                    inner.renderers.remove(id)
                }
                Some(_) => return Ok(None),
                None => return Err(Error::RendererNotFound(id.to_string())),
            }
        }
        .expect("matched renderer disappeared while locked");
        self.stop_handle(id, handle).await.map(Some)
    }

    async fn stop_handle(
        &self,
        id: &str,
        handle: Arc<RendererHandle>,
    ) -> Result<RendererProcessExit> {
        self.unregister_process_group(handle.process_group);
        self.subscriptions.remove(id);

        let writer = handle.writer.clone();
        let _ =
            tokio::task::spawn_blocking(move || writer.send_blocking(ControlMsg::Shutdown, None))
                .await;

        let mut child_guard = handle.child.lock().await;
        let mut exit = None;
        let mut forced = false;
        let mut wait_error = None;
        if let Some(mut child) = child_guard.take() {
            // 5 s: comfortably above any plausible vkDeviceWaitIdle
            // under load; image is usually microseconds, mpv/wescene slower.
            match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                Ok(Ok(status)) => {
                    log::info!("renderer {id}: shutdown complete");
                    exit = Some(status);
                }
                Ok(Err(error)) => {
                    wait_error = Some(error.to_string());
                    forced = child.start_kill().is_ok();
                    exit = tokio::time::timeout(Duration::from_secs(1), child.wait())
                        .await
                        .ok()
                        .and_then(std::result::Result::ok);
                }
                Err(_) => {
                    log::warn!("renderer {id}: Shutdown timeout (5s), escalating to SIGKILL");
                    forced = child.start_kill().is_ok();
                    exit = tokio::time::timeout(Duration::from_secs(1), child.wait())
                        .await
                        .ok()
                        .and_then(std::result::Result::ok);
                }
            }
        }
        let force_killed = renderer_was_force_killed(exit.as_ref(), forced);
        Ok(RendererProcessExit {
            renderer_id: id.to_string(),
            process_generation: handle.process_generation,
            kind: if force_killed {
                RendererProcessExitKind::Killed
            } else if renderer_exit_failed(exit.as_ref()) || wait_error.is_some() {
                RendererProcessExitKind::Failed
            } else {
                RendererProcessExitKind::Stopped
            },
            code: exit.as_ref().and_then(ExitStatus::code),
            signal: exit.as_ref().and_then(ExitStatusExt::signal),
            reason: if force_killed {
                wait_error.map_or_else(
                    || "renderer did not exit after Shutdown and was killed".to_string(),
                    |error| format!("wait after Shutdown failed: {error}; renderer was killed"),
                )
            } else {
                exit.as_ref().map(renderer_exit_status).unwrap_or_else(|| {
                    wait_error.map_or_else(
                        || "renderer stopped".to_string(),
                        |error| format!("wait after Shutdown failed: {error}"),
                    )
                })
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Reader thread

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

// ---------------------------------------------------------------------------
// Test stubs

#[cfg(test)]
impl RendererHandle {
    /// Test-only: inject a `PeerCaps` so router-level negotiation
    /// tests can pretend the renderer shipped a `FormatCaps` event.
    pub fn test_set_format_caps(&self, caps: crate::wallframe::dma::negotiate::PeerCaps) {
        if let Ok(mut g) = self.format_caps.lock() {
            *g = Some(caps);
        }
    }

    /// Test-only: read the producer's blacklist length. Lets a
    /// router-side test assert bind-failure blacklist mutation.
    pub fn test_blacklist_len(&self) -> usize {
        self.format_caps
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|c| c.blacklist.len()))
            .unwrap_or(0)
    }

    pub fn test_set_latest_frame(&self, frame: FrameSnapshot) {
        use nix::sys::memfd::{memfd_create, MemFdCreateFlag};
        use std::ffi::CString;

        let name = CString::new("waywallen-frame-test").unwrap();
        let fd = memfd_create(&name, MemFdCreateFlag::MFD_CLOEXEC).unwrap();
        if let Ok(mut guard) = self.sync_fds.lock() {
            guard.push_back((frame.seq, fd));
        }
        if let Ok(mut guard) = self.latest_frame.lock() {
            *guard = Some(frame);
        }
    }

    pub(crate) fn test_set_clear_rgba(&self, clear_rgba: [f32; 4]) {
        if let Ok(mut state) = self.reported_state.lock() {
            state.clear_rgba = clear_rgba;
        }
    }

    pub(crate) fn test_set_runtime_tags(&self, runtime_tags: Vec<RendererRuntimeTag>) {
        if let Ok(mut state) = self.reported_state.lock() {
            state.runtime_tags = runtime_tags;
        }
    }
}

impl RendererHandle {
    fn test_writer(id: &str, stream: StdUnixStream) -> RendererWriter {
        let subscriptions = Arc::new(RendererSubscriptionRegistry::new());
        subscriptions.register(id.to_string());
        let (reap_tx, _reap_rx) = tokio::sync::mpsc::unbounded_channel();
        RendererWriter::spawn(id.to_string(), 1, stream, subscriptions, reap_tx)
    }

    /// Construct a `RendererHandle` with no running child process.
    /// Used by routing-table unit tests.
    pub fn test_stub(id: &str, wp_type: &str) -> Arc<Self> {
        let (handle, _peer) = Self::test_stub_with_peer_inner(id, wp_type, None);
        handle
    }

    #[cfg(test)]
    pub fn test_stub_with_peer(id: &str, wp_type: &str) -> (Arc<Self>, StdUnixStream) {
        Self::test_stub_with_peer_inner(id, wp_type, None)
    }

    #[cfg(test)]
    pub(crate) fn test_stub_with_frame_records(
        id: &str,
        wp_type: &str,
    ) -> (
        Arc<Self>,
        tokio::sync::mpsc::UnboundedReceiver<crate::wallframe::sync::FrameRecord>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (handle, _peer) = Self::test_stub_with_peer_inner(id, wp_type, Some(tx));
        (handle, rx)
    }

    fn test_stub_with_peer_inner(
        id: &str,
        wp_type: &str,
        frame_record_tx: Option<
            tokio::sync::mpsc::UnboundedSender<crate::wallframe::sync::FrameRecord>,
        >,
    ) -> (Arc<Self>, StdUnixStream) {
        let (a, b) = StdUnixStream::pair().expect("UnixStream pair");
        let (events_tx, _) = broadcast::channel::<RendererEvent>(8);
        let (release_events_tx, release_events_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::wallframe::sync::ReleaseEvent>();
        let handle = Arc::new(Self {
            id: id.into(),
            process_generation: 1,
            wp_type: wp_type.into(),
            extras: HashMap::new(),
            name: "test-stub".into(),
            plugin_id: "test.plugin".into(),
            activity_mode: RendererActivityMode::OnDemand,
            pid: None,
            process_group: None,
            gpu: DrmNode::UNKNOWN,
            spawn_request: SpawnRequest {
                wp_type: wp_type.into(),
                renderer_name: Some("test-stub".into()),
                ..Default::default()
            },
            writer: Self::test_writer(id, a),
            events: events_tx,
            _release_events_tx: release_events_tx,
            release_events_rx: StdMutex::new(Some(release_events_rx)),
            progress: Arc::new(StdMutex::new(RendererProgress::new())),
            published_pool: Arc::new(StdMutex::new(None)),
            sync_fds: Arc::new(StdMutex::new(std::collections::VecDeque::new())),
            latest_frame: Arc::new(StdMutex::new(None)),
            release_syncobj: Arc::new(StdMutex::new(None)),
            format_caps: Arc::new(StdMutex::new(None)),
            last_dispatched_scheme: Arc::new(StdMutex::new(None)),
            frame_record_tx,
            pending_configure: Arc::new(StdMutex::new(None)),
            child: Arc::new(TokioMutex::new(None)),
            stderr: None,
            reported_state: Arc::new(StdMutex::new(RendererReportedState::default())),
        });
        (handle, b)
    }
}

impl RendererManager {
    /// Insert a pre-built handle into the manager's map without
    /// spawning a child process. Used by routing-table unit tests.
    pub async fn register_test_handle(&self, handle: Arc<RendererHandle>) {
        self.subscriptions.register(handle.id.clone());
        let process_group = handle.process_group;
        let mut inner = self.inner.lock().await;
        inner.renderers.insert(handle.id.clone(), handle);
        drop(inner);
        if let Some(process_group) = process_group {
            self.update_process_group(process_group, true);
        }
    }
}

#[cfg(test)]
mod subscription_tests {
    use super::*;
    use crate::wallframe::ipc::proto::{AudioStreamFormat, RgbaColor};
    use crate::wallframe::ipc::uds::{recv_control, CodecError};

    #[test]
    fn renderer_process_label_matches_ui_identity() {
        assert_eq!(
            renderer_process_label("wescene-renderer", Some(4321), "ignored"),
            "wescene-renderer-4321"
        );
        assert_eq!(
            renderer_process_label("", None, "d6ce5a5f-eab7-4bd6-a898-f5c84d027c5e"),
            "renderer-d6ce5a5f"
        );
    }

    #[tokio::test]
    async fn renderer_log_level_update_is_sent_to_live_renderer() {
        let manager = RendererManager::new_default();
        let (handle, peer) = RendererHandle::test_stub_with_peer("renderer", "scene");
        manager.register_test_handle(handle).await;
        let level = if manager.renderer_log_snapshot().level == LogLevel::Debug {
            LogLevel::Info
        } else {
            LogLevel::Debug
        };
        let reader = std::thread::spawn(move || recv_control(&peer).unwrap());

        manager.set_renderer_log_level(level).await;

        let (message, fds) = reader.join().unwrap();
        assert_eq!(message, ControlMsg::SetLogLevel { level });
        assert!(fds.is_empty());
    }

    #[test]
    fn renderer_exit_status_diagnoses_runtime_dependency_failure() {
        let status = ExitStatus::from_raw(127 << 8);
        let message = renderer_exit_status(&status);
        assert!(message.contains("exit status: 127"), "{message}");
        assert!(message.contains("runtime dependency"), "{message}");
        assert!(message.contains("launcher command"), "{message}");
    }

    #[test]
    fn renderer_exit_status_keeps_signal_diagnostic_factual() {
        let status = ExitStatus::from_raw(libc::SIGSEGV);
        let message = renderer_exit_status(&status);
        assert!(message.contains("signal: 11"), "{message}");
        assert!(!message.contains("possible"), "{message}");
    }

    #[test]
    fn renderer_signal_exit_is_not_force_killed() {
        let segfault = ExitStatus::from_raw(libc::SIGSEGV);
        let external_sigkill = ExitStatus::from_raw(libc::SIGKILL);

        assert!(!renderer_was_force_killed(Some(&segfault), true));
        assert!(!renderer_was_force_killed(Some(&external_sigkill), false));
        assert!(renderer_exit_failed(Some(&segfault)));
        assert!(renderer_exit_failed(Some(&external_sigkill)));
    }

    #[test]
    fn daemon_sigkill_is_force_killed() {
        let status = ExitStatus::from_raw(libc::SIGKILL);
        assert!(renderer_was_force_killed(Some(&status), true));
    }

    #[tokio::test]
    async fn failed_renderer_process_status_includes_exit_code() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 23")
            .spawn()
            .unwrap();

        let status = failed_renderer_process_status(&mut child).await;
        assert_eq!(status, "process_status=exit status: 23");
    }

    fn state_patch(fields: u32, tags: Vec<(String, String)>) -> RendererState {
        RendererState {
            fields,
            clear_color: RgbaColor {
                r: 0.25,
                g: 0.5,
                b: 0.75,
                a: 1.0,
            },
            runtime_tags: tags,
        }
    }

    #[test]
    fn renderer_state_patches_preserve_unselected_fields() {
        let mut current = RendererReportedState::default();
        let tags = vec![("hwdec".to_string(), "vulkan".to_string())];
        let changed = apply_renderer_state_patch(
            &mut current,
            &state_patch(RENDERER_STATE_FIELD_RUNTIME_TAGS, tags),
        )
        .unwrap();
        assert_eq!(changed, RENDERER_STATE_FIELD_RUNTIME_TAGS);
        assert_eq!(current.clear_rgba, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            current.runtime_tags,
            vec![RendererRuntimeTag {
                key: "hwdec".to_string(),
                value: "vulkan".to_string(),
            }]
        );

        let changed = apply_renderer_state_patch(
            &mut current,
            &state_patch(
                RENDERER_STATE_FIELD_RUNTIME_TAGS,
                vec![("hwdec".to_string(), "vulkan".to_string())],
            ),
        )
        .unwrap();
        assert_eq!(changed, 0);

        let changed = apply_renderer_state_patch(
            &mut current,
            &state_patch(RENDERER_STATE_FIELD_CLEAR_COLOR, Vec::new()),
        )
        .unwrap();
        assert_eq!(changed, RENDERER_STATE_FIELD_CLEAR_COLOR);
        assert_eq!(current.clear_rgba, [0.25, 0.5, 0.75, 1.0]);
        assert_eq!(current.runtime_tags[0].value, "vulkan");

        let changed = apply_renderer_state_patch(
            &mut current,
            &state_patch(RENDERER_STATE_FIELD_RUNTIME_TAGS, Vec::new()),
        )
        .unwrap();
        assert_eq!(changed, RENDERER_STATE_FIELD_RUNTIME_TAGS);
        assert!(current.runtime_tags.is_empty());
    }

    #[test]
    fn invalid_renderer_state_patch_is_atomic() {
        let mut current = RendererReportedState {
            clear_rgba: [0.0, 0.0, 0.0, 1.0],
            runtime_tags: vec![RendererRuntimeTag {
                key: "hwdec".to_string(),
                value: "vaapi".to_string(),
            }],
        };
        let before = current.clone();
        let invalid = state_patch(
            RENDERER_STATE_FIELD_CLEAR_COLOR | RENDERER_STATE_FIELD_RUNTIME_TAGS,
            vec![
                ("hwdec".to_string(), "vulkan".to_string()),
                ("hwdec".to_string(), "sw".to_string()),
            ],
        );
        assert!(apply_renderer_state_patch(&mut current, &invalid).is_err());
        assert_eq!(current, before);

        let unknown = state_patch(1 << 31, Vec::new());
        assert!(apply_renderer_state_patch(&mut current, &unknown).is_err());
        assert_eq!(current, before);
    }

    #[test]
    fn runtime_tag_constraints_reject_invalid_lists() {
        let too_many = (0..=MAX_RUNTIME_TAGS)
            .map(|index| (format!("tag{index}"), "value".to_string()))
            .collect::<Vec<_>>();
        assert!(validate_runtime_tags(&too_many).is_err());
        assert!(validate_runtime_tags(&[("Hwdec".to_string(), "vulkan".to_string())]).is_err());
        assert!(validate_runtime_tags(&[("hwdec".to_string(), "bad\nvalue".to_string())]).is_err());
        assert!(validate_runtime_tags(&[(
            "hwdec".to_string(),
            "x".repeat(MAX_RUNTIME_TAG_VALUE_BYTES + 1),
        )])
        .is_err());
    }

    #[tokio::test]
    async fn pointer_forwarding_setting_gates_renderer_control() {
        let temp = tempfile::tempdir().unwrap();
        let settings = SettingsStore::load_or_default(temp.path().join("settings.toml")).await;
        let manager = RendererManager::new_default();
        manager.attach_settings(settings.clone());

        let (handle, peer) = RendererHandle::test_stub_with_peer("renderer", "scene");
        manager.register_test_handle(handle).await;
        let applied = manager
            .subscriptions
            .prepare("renderer", 1, &["pointer".to_string()]);
        manager
            .subscriptions
            .commit("renderer".to_string(), applied.commit.unwrap());

        settings.update(|state| state.global.pointer_forwarding_enabled = false);
        peer.set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        manager
            .send_pointer_motion(
                "renderer",
                PointerMotion {
                    x: 1.0,
                    y: 2.0,
                    timestamp_us: 3,
                    modifiers: 0,
                },
            )
            .await
            .unwrap();
        let error = recv_control(&peer).unwrap_err();
        match error {
            CodecError::Nix(nix::errno::Errno::EAGAIN) => {}
            CodecError::Io(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            other => panic!("expected a read timeout, got {other:?}"),
        }

        settings.update(|state| state.global.pointer_forwarding_enabled = true);
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        manager
            .send_pointer_motion(
                "renderer",
                PointerMotion {
                    x: 4.0,
                    y: 5.0,
                    timestamp_us: 6,
                    modifiers: 0,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            recv_control(&peer).unwrap().0,
            ControlMsg::PointerMotion {
                event: PointerMotion {
                    x: 4.0,
                    y: 5.0,
                    timestamp_us: 6,
                    modifiers: 0,
                },
            }
        ));
    }

    #[test]
    fn process_group_registration_publishes_generation_and_rolls_back() {
        let manager = RendererManager::new_default();
        let ownership = manager.subscribe_process_ownership();

        {
            let _registration = manager.register_process_group(Some(42));
            let snapshot = ownership.borrow().clone();
            assert_eq!(snapshot.generation, 1);
            assert!(snapshot.owns_process_group(42));
        }

        let snapshot = ownership.borrow().clone();
        assert_eq!(snapshot.generation, 2);
        assert!(!snapshot.owns_process_group(42));
    }

    #[test]
    fn subscription_registry_applies_complete_sets_atomically() {
        let registry = RendererSubscriptionRegistry::new();
        registry.register("renderer".to_string());
        assert!(registry
            .snapshot()
            .subscribers(RendererEventKind::Audio)
            .is_empty());

        let applied = registry.prepare(
            "renderer",
            1,
            &[
                "audio".to_string(),
                "pointer".to_string(),
                "audio".to_string(),
            ],
        );
        assert_eq!(applied.status, EventSubscriptionStatus::Applied);
        assert_eq!(applied.kinds, vec!["pointer", "audio"]);
        assert!(registry
            .snapshot()
            .subscribers(RendererEventKind::Audio)
            .is_empty());
        registry.commit("renderer".to_string(), applied.commit.unwrap());
        assert_eq!(
            registry
                .snapshot()
                .revision_for("renderer", RendererEventKind::Audio),
            Some(1)
        );

        let replay = registry.prepare("renderer", 1, &["pointer".to_string(), "audio".to_string()]);
        assert_eq!(replay.status, EventSubscriptionStatus::Applied);
        assert!(replay.commit.is_none());

        let conflict = registry.prepare("renderer", 1, &["mpris".to_string()]);
        assert_eq!(conflict.status, EventSubscriptionStatus::RevisionConflict);
        let next = registry.prepare("renderer", 2, &["audio".to_string()]);
        registry.commit("renderer".to_string(), next.commit.unwrap());
        let stale = registry.prepare("renderer", 1, &[]);
        assert_eq!(stale.status, EventSubscriptionStatus::StaleRevision);
        assert_eq!(
            registry
                .snapshot()
                .revision_for("renderer", RendererEventKind::Audio),
            Some(2)
        );
    }

    #[test]
    fn subscription_registry_rejects_unknown_and_oversized_sets() {
        let registry = RendererSubscriptionRegistry::new();
        registry.register("renderer".to_string());

        let unknown = registry.prepare("renderer", 1, &["video".to_string()]);
        assert_eq!(unknown.status, EventSubscriptionStatus::Invalid);
        let oversized = registry.prepare("renderer", 1, &["x".repeat(MAX_EVENT_KIND_BYTES + 1)]);
        assert_eq!(oversized.status, EventSubscriptionStatus::LimitExceeded);
        let too_many = registry.prepare(
            "renderer",
            1,
            &vec!["audio".to_string(); MAX_EVENT_SUBSCRIPTIONS + 1],
        );
        assert_eq!(too_many.status, EventSubscriptionStatus::LimitExceeded);
        assert!(registry
            .snapshot()
            .subscribers(RendererEventKind::Audio)
            .is_empty());
    }

    #[test]
    fn subscription_snapshot_filters_mpris_subscribers() {
        let registry = RendererSubscriptionRegistry::new();
        registry.register("audio".to_string());
        registry.register("mpris".to_string());

        let audio = registry.prepare("audio", 1, &["audio".to_string()]);
        registry.commit("audio".to_string(), audio.commit.unwrap());
        let mpris = registry.prepare("mpris", 4, &["pointer".to_string(), "mpris".to_string()]);
        registry.commit("mpris".to_string(), mpris.commit.unwrap());

        assert_eq!(
            registry.snapshot().subscribers(RendererEventKind::Mpris),
            vec![("mpris".to_string(), 4)]
        );
    }

    #[tokio::test]
    async fn send_mpris_requires_a_live_committed_subscription() {
        let manager = RendererManager::new_default();
        let (handle, peer) = RendererHandle::test_stub_with_peer("renderer", "scene");
        peer.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        manager.register_test_handle(handle).await;
        let snapshot = MprisSnapshot {
            state: 1,
            title: "Track".to_string(),
            artist: "Artist".to_string(),
            ..MprisSnapshot::default()
        };
        let assert_no_control = || match recv_control(&peer).unwrap_err() {
            CodecError::Nix(nix::errno::Errno::EAGAIN) => {}
            CodecError::Io(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            error => panic!("expected a read timeout, got {error:?}"),
        };

        manager
            .send_mpris("renderer", snapshot.clone())
            .await
            .unwrap();
        assert_no_control();

        let applied = manager
            .subscriptions
            .prepare("renderer", 1, &["mpris".to_string()]);
        manager
            .subscriptions
            .commit("renderer".to_string(), applied.commit.unwrap());
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        manager.send_mpris("renderer", snapshot).await.unwrap();
        match recv_control(&peer).unwrap().0 {
            ControlMsg::Mpris { snapshot } => {
                assert_eq!(snapshot.state, MediaPlaybackState::Playing);
                assert_eq!(snapshot.title, "Track");
                assert_eq!(snapshot.artist, "Artist");
            }
            message => panic!("expected MPRIS snapshot, got {message:?}"),
        }

        let removed = manager.subscriptions.prepare("renderer", 2, &[]);
        manager
            .subscriptions
            .commit("renderer".to_string(), removed.commit.unwrap());
        peer.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        manager
            .send_mpris("renderer", MprisSnapshot::default())
            .await
            .unwrap();
        assert_no_control();

        manager
            .send_mpris("missing", MprisSnapshot::default())
            .await
            .unwrap();
    }

    #[test]
    fn writer_sends_subscription_ack_before_audio_for_its_revision() {
        let (daemon, renderer) = StdUnixStream::pair().unwrap();
        let subscriptions = Arc::new(RendererSubscriptionRegistry::new());
        subscriptions.register("renderer".to_string());
        let (reap_tx, _reap_rx) = tokio::sync::mpsc::unbounded_channel();
        let writer = RendererWriter::spawn(
            "renderer".to_string(),
            1,
            daemon,
            Arc::clone(&subscriptions),
            reap_tx,
        );
        let applied = subscriptions.prepare("renderer", 1, &["audio".to_string()]);
        writer
            .send_blocking(
                ControlMsg::EventSubscriptionsApplied {
                    result: EventSubscriptionResult {
                        revision: 1,
                        status: applied.status,
                        kinds: applied.kinds,
                        reason: applied.reason,
                    },
                },
                applied.commit,
            )
            .unwrap();
        writer
            .replace_audio(
                1,
                ControlMsg::AudioWindow {
                    window: AudioWindow {
                        subscription_revision: 1,
                        generation: 2,
                        sequence: 3,
                        captured_at_ns: 4,
                        end_sample_frame: 4096,
                        format: AudioStreamFormat {
                            sample_rate_hz: 48_000,
                            channels: 2,
                        },
                        frames: 4096,
                        flags: 0,
                        samples: vec![0.0; 8192],
                    },
                },
            )
            .unwrap();

        let (first, _) = recv_control(&renderer).unwrap();
        let (second, _) = recv_control(&renderer).unwrap();
        assert!(matches!(
            first,
            ControlMsg::EventSubscriptionsApplied {
                result: EventSubscriptionResult { revision: 1, .. }
            }
        ));
        assert!(matches!(
            second,
            ControlMsg::AudioWindow {
                window: AudioWindow {
                    subscription_revision: 1,
                    generation: 2,
                    sequence: 3,
                    ..
                }
            }
        ));
    }
}

#[cfg(test)]
mod init_handshake_tests {
    use super::*;
    use crate::plugin::renderer_registry::{SettingDef, SettingType};
    use crate::wallframe::ipc::proto::{InitRejection, PROTOCOL_VERSION};
    use crate::wallframe::ipc::uds::send_event;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::thread;

    fn def_legacy(name: &str) -> RendererDef {
        // Legacy (no-schema) manifest: build_init_msg falls back to
        // the hard-coded primary-key priority list.
        RendererDef {
            name: name.to_string(),
            plugin_id: "test.plugin".to_string(),
            plugin_version: "0.0.0".to_string(),
            plugin_system: false,
            bin: PathBuf::from("/dev/null"),
            types: vec!["scene".to_string()],
            priority: 100,
            activity: RendererActivityMode::OnDemand,
            spawn_version: None,
            extras: Vec::new(),
            settings: Default::default(),
            legacy_events: None,
        }
    }

    fn def_mpv_schema() -> RendererDef {
        let mut ps = HashMap::new();
        ps.insert(
            "loop_file".to_string(),
            SettingDef::new(
                SettingType::String,
                toml::Value::String("inf".into()),
                false,
            ),
        );
        RendererDef {
            name: "waywallen-mpv".into(),
            plugin_id: "test.plugin".to_string(),
            plugin_version: "0.0.0".to_string(),
            plugin_system: false,
            bin: PathBuf::from("/dev/null"),
            types: vec!["video".into()],
            priority: 100,
            activity: RendererActivityMode::Continuous,
            spawn_version: Some(SPAWN_VERSION),
            extras: Vec::new(),
            settings: ps,
            legacy_events: None,
        }
    }

    // Legacy Init-shape tests were removed after Init became plain settings
    // plus user_properties.

    #[test]
    fn slim_init_carries_extent_and_settings_kv() {
        // Init carries settings kv verbatim; callers own sourcing them from
        // the settings store.
        let mut settings_in = HashMap::new();
        settings_in.insert("loop_file".to_string(), "inf".to_string());
        let req = SpawnRequest {
            extras: HashMap::new(),
            wp_type: "video".into(),
            settings: settings_in,
            test_pattern: false,
            renderer_name: None,
            user_property_overrides: HashMap::new(),
            default_user_properties: HashMap::new(),
            display_size: None,
        };
        let msg = build_init_msg(&req, &def_mpv_schema());
        match msg {
            ControlMsg::Init { config } => {
                assert_eq!(
                    config.protocol_version,
                    crate::wallframe::ipc::proto::PROTOCOL_VERSION
                );
                assert_eq!(config.spawn_version, SPAWN_VERSION);
                assert_eq!(
                    config.settings,
                    vec![("loop_file".to_string(), "inf".to_string())]
                );
                assert_eq!(config.user_properties, "");
            }
            other => panic!("expected ControlMsg::Init, got {other:?}"),
        }
    }

    #[test]
    fn init_carries_display_size_when_set() {
        let req = SpawnRequest {
            extras: HashMap::new(),
            wp_type: "web".into(),
            settings: HashMap::new(),
            test_pattern: false,
            renderer_name: None,
            user_property_overrides: HashMap::new(),
            default_user_properties: HashMap::new(),
            display_size: Some((3440, 1440)),
        };
        let msg = build_init_msg(&req, &def_mpv_schema());
        match msg {
            ControlMsg::Init { config } => {
                assert_eq!(config.display_width, 3440);
                assert_eq!(config.display_height, 1440);
            }
            other => panic!("expected ControlMsg::Init, got {other:?}"),
        }
    }

    #[test]
    fn init_defaults_display_size_to_zero() {
        let req = SpawnRequest {
            extras: HashMap::new(),
            wp_type: "web".into(),
            settings: HashMap::new(),
            test_pattern: false,
            renderer_name: None,
            user_property_overrides: HashMap::new(),
            default_user_properties: HashMap::new(),
            ..Default::default()
        };
        let msg = build_init_msg(&req, &def_mpv_schema());
        match msg {
            ControlMsg::Init { config } => {
                assert_eq!(config.display_width, 0);
                assert_eq!(config.display_height, 0);
            }
            other => panic!("expected ControlMsg::Init, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_rejects_incompatible_manifest_before_exec() {
        let mut def = def_legacy("old-renderer");
        def.bin = PathBuf::from("/path/that/must/not/be/executed");
        def.spawn_version = Some(SPAWN_VERSION - 1);
        let mut registry = RendererRegistry::new();
        registry.register(def);
        let manager = RendererManager::new(registry);
        let error = manager
            .spawn(SpawnRequest {
                wp_type: "scene".to_string(),
                renderer_name: Some("old-renderer".to_string()),
                ..Default::default()
            })
            .await
            .expect_err("old spawn version must be rejected");
        let message = error.to_string();
        assert!(
            message.contains("renderer spawn version mismatch"),
            "{message}"
        );
        assert!(
            message.contains(&format!("declares {}", SPAWN_VERSION - 1)),
            "{message}"
        );
        assert!(
            message.contains(&format!("daemon requires {SPAWN_VERSION}")),
            "{message}"
        );
        assert!(message.contains("update the plugin"), "{message}");
    }

    #[tokio::test]
    async fn spawn_reports_early_renderer_exit_without_waiting_for_connect_timeout() {
        let directory = tempfile::tempdir().expect("create renderer directory");
        let renderer = directory.path().join("renderer-exits-127");
        std::fs::write(&renderer, "#!/bin/sh\nexit 127\n").expect("write renderer script");
        let mut permissions = std::fs::metadata(&renderer)
            .expect("read renderer metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&renderer, permissions).expect("make renderer executable");

        let mut def = def_legacy("early-exit-renderer");
        def.bin = renderer;
        let mut registry = RendererRegistry::new();
        registry.register(def);
        let manager = RendererManager::new(registry);

        let result = tokio::time::timeout(
            Duration::from_secs(3),
            manager.spawn(SpawnRequest {
                wp_type: "scene".to_string(),
                renderer_name: Some("early-exit-renderer".to_string()),
                ..Default::default()
            }),
        )
        .await
        .expect("early exit must beat the 10-second connect timeout");
        let message = result
            .expect_err("early renderer exit must fail")
            .to_string();
        assert!(
            message.contains("renderer exited before IPC connect"),
            "{message}"
        );
        assert!(message.contains("exit status: 127"), "{message}");
        assert!(message.contains("runtime dependency"), "{message}");
    }

    #[test]
    fn init_merges_authored_defaults_with_user_overrides() {
        let req = SpawnRequest {
            wp_type: "video".into(),
            default_user_properties: HashMap::from([
                (
                    "waywallen.scheme_color".to_string(),
                    "0.1 0.2 0.3".to_string(),
                ),
                ("speed".to_string(), "100".to_string()),
            ]),
            user_property_overrides: HashMap::from([(
                "waywallen.scheme_color".to_string(),
                "0.8 0.7 0.6".to_string(),
            )]),
            ..Default::default()
        };
        let ControlMsg::Init { config } = build_init_msg(&req, &def_mpv_schema()) else {
            panic!("expected Init");
        };
        let properties: HashMap<String, String> =
            serde_json::from_str(&config.user_properties).unwrap();
        assert_eq!(
            properties.get("waywallen.scheme_color").map(String::as_str),
            Some("0.8 0.7 0.6")
        );
        assert_eq!(properties.get("speed").map(String::as_str), Some("100"));
    }

    #[tokio::test]
    async fn init_handshake_times_out_and_unblocks_receiver() {
        let (daemon, _renderer) = StdUnixStream::pair().expect("UnixStream::pair");
        let init = build_init_msg(&SpawnRequest::default(), &def_legacy("timeout-renderer"));

        let error = run_init_handshake_with_timeout(&daemon, init, Duration::from_millis(20))
            .await
            .expect_err("missing Ready must time out");

        assert!(
            error
                .to_string()
                .contains("timed out waiting for renderer Ready after 20ms"),
            "{error}"
        );
    }

    #[test]
    fn spawn_handshake_init_nack_aborts() {
        // Drive the daemon side over a socketpair while the peer replies
        // with InitNack.
        let (daemon, renderer) = StdUnixStream::pair().expect("UnixStream::pair");
        daemon
            .set_nonblocking(false)
            .expect("set_nonblocking(false) on daemon side");
        renderer
            .set_nonblocking(false)
            .expect("set_nonblocking(false) on renderer side");

        let peer = thread::spawn(move || {
            // Receive the Init then immediately reply with InitNack.
            let (got, _fds) =
                crate::wallframe::ipc::uds::recv_control(&renderer).expect("renderer recv Init");
            assert!(matches!(got, ControlMsg::Init { .. }));
            send_event(
                &renderer,
                &EventMsg::InitNack {
                    rejection: InitRejection {
                        received_protocol_version: PROTOCOL_VERSION,
                        supported_protocol_version: PROTOCOL_VERSION,
                        received_spawn_version: 999,
                        supported_spawn_version: SPAWN_VERSION,
                        reason: "unsupported spawn_version".into(),
                    },
                },
                &[],
            )
            .expect("renderer send InitNack");
        });

        let mut settings = HashMap::new();
        settings.insert("scene".to_string(), "/tmp/scene.pkg".to_string());
        let req = SpawnRequest {
            extras: HashMap::new(),
            wp_type: "scene".into(),
            settings,
            test_pattern: false,
            renderer_name: None,
            user_property_overrides: HashMap::new(),
            default_user_properties: HashMap::new(),
            display_size: None,
        };
        let init = build_init_msg(&req, &def_legacy("wescene-renderer"));
        let err =
            run_init_handshake(&daemon, &init).expect_err("InitNack must abort the handshake");
        let s = err.to_string();
        assert!(
            s.contains("renderer rejected Init"),
            "unexpected error: {s}"
        );
        assert!(
            s.contains("unsupported spawn_version"),
            "unexpected error: {s}"
        );

        peer.join().expect("peer thread");
    }
}

#[cfg(test)]
mod reuse_tests {
    use super::*;
    use crate::plugin::renderer_registry::{
        RendererDef, RendererRegistry, SettingDef, SettingType,
    };
    use std::path::PathBuf;

    fn def_mpv() -> RendererDef {
        let mut ps = HashMap::new();
        ps.insert(
            "loop_file".to_string(),
            SettingDef::new(
                SettingType::String,
                toml::Value::String("inf".into()),
                false,
            ),
        );
        ps.insert(
            "hwdec".to_string(),
            SettingDef::new(
                SettingType::String,
                toml::Value::String("auto".into()),
                false,
            ),
        );
        RendererDef {
            name: "waywallen-mpv".into(),
            plugin_id: "test.plugin".to_string(),
            plugin_version: "0.0.0".to_string(),
            plugin_system: false,
            bin: PathBuf::from("/dev/null"),
            types: vec!["video".into()],
            priority: 100,
            activity: RendererActivityMode::Continuous,
            spawn_version: Some(SPAWN_VERSION),
            extras: Vec::new(),
            settings: ps,
            legacy_events: None,
        }
    }

    #[tokio::test]
    async fn stop_generation_does_not_remove_a_newer_process() {
        let mgr = RendererManager::new(RendererRegistry::new());
        let handle = RendererHandle::test_stub("h1", "image");
        let generation = handle.process_generation;
        mgr.register_test_handle(handle).await;

        assert!(mgr
            .stop_generation("h1", generation.wrapping_add(1))
            .await
            .unwrap()
            .is_none());
        assert_eq!(mgr.get("h1").await.unwrap().process_generation, generation);

        let exit = mgr
            .stop_generation("h1", generation)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(exit.process_generation, generation);
        assert!(mgr.get("h1").await.is_none());
    }

    #[tokio::test]
    async fn request_frame_writes_typed_control() {
        let mut registry = RendererRegistry::new();
        registry.register(def_mpv());
        let mgr = RendererManager::new(registry);
        let (handle, peer) = RendererHandle::test_stub_with_peer("h1", "image");
        mgr.register_test_handle(handle).await;

        let reader = std::thread::spawn(move || {
            crate::wallframe::ipc::uds::recv_control(&peer).expect("recv request_frame")
        });
        mgr.request_frame("h1").await.expect("request_frame send");
        let (message, fds) = reader.join().expect("peer joined");
        assert_eq!(message, ControlMsg::RequestFrame);
        assert!(fds.is_empty());
    }

    #[tokio::test]
    async fn send_setting_changed_writes_wire_and_updates_cache() {
        // Direct end-to-end: wire a socketpair into a RendererHandle and
        // drain the setting_changed control message from the peer side.
        let mut registry = RendererRegistry::new();
        registry.register(def_mpv());
        let mgr = RendererManager::new(registry);

        let (daemon_side, renderer_side) = std::os::unix::net::UnixStream::pair().unwrap();
        daemon_side.set_nonblocking(false).unwrap();
        renderer_side.set_nonblocking(false).unwrap();

        let (events_tx, _) = tokio::sync::broadcast::channel::<RendererEvent>(8);
        let (release_events_tx, release_events_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::wallframe::sync::ReleaseEvent>();
        let h = Arc::new(RendererHandle {
            id: "h1".into(),
            process_generation: 1,
            wp_type: "video".into(),
            extras: HashMap::new(),
            name: "waywallen-mpv".into(),
            plugin_id: "test.plugin".into(),
            activity_mode: RendererActivityMode::Continuous,
            pid: None,
            process_group: None,
            gpu: DrmNode::UNKNOWN,
            spawn_request: SpawnRequest {
                wp_type: "video".into(),
                renderer_name: Some("waywallen-mpv".into()),
                ..Default::default()
            },
            writer: RendererHandle::test_writer("h1", daemon_side),
            events: events_tx,
            _release_events_tx: release_events_tx,
            release_events_rx: StdMutex::new(Some(release_events_rx)),
            progress: Arc::new(StdMutex::new(RendererProgress::new())),
            published_pool: Arc::new(StdMutex::new(None)),
            sync_fds: Arc::new(StdMutex::new(std::collections::VecDeque::new())),
            latest_frame: Arc::new(StdMutex::new(None)),
            release_syncobj: Arc::new(StdMutex::new(None)),
            format_caps: Arc::new(StdMutex::new(None)),
            last_dispatched_scheme: Arc::new(StdMutex::new(None)),
            frame_record_tx: None,
            pending_configure: Arc::new(StdMutex::new(None)),
            child: Arc::new(TokioMutex::new(None)),
            stderr: None,
            reported_state: Arc::new(StdMutex::new(RendererReportedState::default())),
        });
        mgr.register_test_handle(Arc::clone(&h)).await;

        // Renderer-side reader running in a thread to drain the wire.
        let peer = std::thread::spawn(move || {
            let (req, _fds) =
                crate::wallframe::ipc::uds::recv_control(&renderer_side).expect("recv");
            req
        });

        mgr.send_setting_changed("h1", vec![("loop_file".into(), "no".into())], None)
            .await
            .expect("send_setting_changed ok");

        let got = peer.join().expect("peer joined");
        match got {
            ControlMsg::SettingChanged { settings } => {
                assert_eq!(settings, vec![("loop_file".into(), "no".into())]);
            }
            other => panic!("expected ApplySettings, got {other:?}"),
        }
    }
}
