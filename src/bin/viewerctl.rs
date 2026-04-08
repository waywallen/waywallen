//! viewerctl — minimal viewer client for the waywallen viewer endpoint.
//!
//! Subcommands:
//!   dump  --renderer ID --frames N --out PATTERN [--socket PATH]
//!         Subscribes to the given renderer, mmaps each DMA-BUF FD it
//!         receives, and copies the bytes of the first N FrameReady
//!         events out to numbered files. PATTERN must contain a `%03d`
//!         placeholder which is replaced with the frame index.
//!
//! Designed for the iteration-4 architecture milestone: combine with
//! ffmpeg to turn the resulting raw frames into a video.

use anyhow::{anyhow, bail, Context, Result};
use kwallpaper_backend::ipc::proto::{EventMsg, ViewerMsg, PROTOCOL_VERSION};
use kwallpaper_backend::ipc::uds::{recv_msg, send_msg};

use std::io::Write;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

fn usage() -> ! {
    eprintln!(
        "usage: viewerctl dump --renderer ID --frames N --out PATH [--socket SOCK]\n\
         \n\
         PATH should contain `%03d` for the frame number (e.g. /tmp/f%03d.raw).\n\
         SOCK defaults to $XDG_RUNTIME_DIR/waywallen/viewer.sock."
    );
    std::process::exit(2);
}

#[derive(Debug)]
struct DumpArgs {
    renderer: String,
    frames: usize,
    out: String,
    socket: PathBuf,
}

fn parse_dump(mut args: impl Iterator<Item = String>) -> Result<DumpArgs> {
    let mut renderer = None;
    let mut frames = None;
    let mut out = None;
    let mut socket = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--renderer" => {
                renderer = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("missing value for --renderer"))?,
                );
            }
            "--frames" => {
                let s = args
                    .next()
                    .ok_or_else(|| anyhow!("missing value for --frames"))?;
                frames = Some(s.parse::<usize>().context("parse --frames")?);
            }
            "--out" => {
                out = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("missing value for --out"))?,
                );
            }
            "--socket" => {
                socket = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("missing value for --socket"))?,
                ));
            }
            _ => bail!("unknown arg: {a}"),
        }
    }
    Ok(DumpArgs {
        renderer: renderer.ok_or_else(|| anyhow!("--renderer required"))?,
        frames: frames.ok_or_else(|| anyhow!("--frames required"))?,
        out: out.ok_or_else(|| anyhow!("--out required"))?,
        socket: socket.unwrap_or_else(default_socket_path),
    })
}

fn default_socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    runtime.join("waywallen").join("viewer.sock")
}

fn dump(args: DumpArgs) -> Result<()> {
    if !args.out.contains("%03d") && !args.out.contains("%04d") {
        bail!("--out PATTERN must contain a %03d or %04d placeholder");
    }

    let stream =
        UnixStream::connect(&args.socket).with_context(|| format!("connect {}", args.socket.display()))?;

    // Hello + Subscribe.
    send_msg(
        &stream,
        &ViewerMsg::Hello {
            client: "viewerctl".to_string(),
            version: PROTOCOL_VERSION,
        },
        &[],
    )?;
    send_msg(
        &stream,
        &ViewerMsg::Subscribe {
            renderer_id: args.renderer.clone(),
        },
        &[],
    )?;

    // First message must be BindBuffers.
    let (bind, fds): (EventMsg, Vec<OwnedFd>) = recv_msg(&stream)?;
    let (count, width, height, stride, sizes) = match bind {
        EventMsg::BindBuffers {
            count,
            width,
            height,
            stride,
            ref sizes,
            ..
        } => (count, width, height, stride, sizes.clone()),
        other => bail!("expected BindBuffers, got {other:?}"),
    };
    if fds.len() as u32 != count {
        bail!(
            "BindBuffers said count={count} but got {} fds",
            fds.len()
        );
    }
    eprintln!(
        "[viewerctl] subscribed: {count} buffers, {width}x{height}, stride {stride}"
    );

    // mmap each fd.
    let mappings: Vec<MmapBuf> = fds
        .iter()
        .zip(sizes.iter())
        .map(|(fd, size)| MmapBuf::new(fd.as_raw_fd(), *size as usize))
        .collect::<Result<_>>()?;

    // Drain N FrameReady events, dump bytes per frame.
    for n in 0..args.frames {
        let (event, _fds): (EventMsg, _) = recv_msg(&stream)?;
        let image_index = match event {
            EventMsg::FrameReady { image_index, .. } => image_index,
            other => bail!("expected FrameReady, got {other:?}"),
        };
        let buf = mappings
            .get(image_index as usize)
            .ok_or_else(|| anyhow!("frame referenced unknown image_index {image_index}"))?;
        let path_str = format_pattern(&args.out, n);
        let mut f = std::fs::File::create(&path_str)
            .with_context(|| format!("create {path_str}"))?;
        f.write_all(buf.as_bytes())
            .with_context(|| format!("write {path_str}"))?;
        eprintln!("[viewerctl] frame {n:03} → {path_str} ({} bytes)", buf.len);
    }

    Ok(())
}

fn format_pattern(pattern: &str, n: usize) -> String {
    if pattern.contains("%03d") {
        pattern.replacen("%03d", &format!("{n:03}"), 1)
    } else {
        pattern.replacen("%04d", &format!("{n:04}"), 1)
    }
}

// ---------------------------------------------------------------------------
// Tiny mmap helper. Avoids pulling in a memmap2 dependency.
// ---------------------------------------------------------------------------

struct MmapBuf {
    ptr: *mut libc::c_void,
    len: usize,
}

impl MmapBuf {
    fn new(fd: i32, len: usize) -> Result<Self> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error()).context("mmap");
        }
        Ok(MmapBuf { ptr, len })
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }
}

impl Drop for MmapBuf {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

// SAFETY: MmapBuf is logically a read-only borrow of a kernel-managed
// mapping; passing it across thread boundaries is safe.
unsafe impl Send for MmapBuf {}
unsafe impl Sync for MmapBuf {}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| usage());
    let result = match cmd.as_str() {
        "dump" => parse_dump(args).and_then(dump),
        "-h" | "--help" => {
            usage();
        }
        other => {
            eprintln!("unknown subcommand: {other}");
            usage();
        }
    };
    if let Err(e) = result {
        eprintln!("viewerctl: {e:#}");
        std::process::exit(1);
    }
}

fn _path_unused(_p: &Path) {} // keep `Path` import alive
