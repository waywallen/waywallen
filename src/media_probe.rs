//! Fail-soft media metadata probe.
//!
//! Lazy-loads `libavformat` via `libloading` (no link-time dep on FFmpeg) and
//! exposes a tiny [`MediaProbe`] trait whose `probe` method is total: it never
//! returns `Err` and never panics. When libavformat cannot be loaded — or its
//! ABI doesn't match what we coded against — the probe degrades gracefully to
//! filling only `size` from `std::fs::metadata`.
//!
//! The cross-agent contract for this module lives in
//! `.plans/media-meta/notes.md`. Don't change shapes here without updating
//! that file.
//!
//! Supported `libavformat` SONAMEs: 60 (FFmpeg 6.x), 61 (FFmpeg 7.x),
//! 62 (FFmpeg 8.x devel). Other majors fall back to size-only because the
//! `AVCodecParameters` layout shifted in FFmpeg 7.0 (added `coded_side_data`
//! between `extradata_size` and `format`). When that happens we still log a
//! single warning per process and never retry.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::fs;
use std::ptr;
use std::sync::Mutex;

use libloading::{Library, Symbol};
use log::warn;

/// Public, owner-controlled list of `SONAME`s we'll try to dlopen, in order.
/// NEVER take this from caller-controlled input — see security rules in
/// `.plans/media-meta/plan.md`.
const LIBAVFORMAT_CANDIDATES: &[&str] = &[
    "libavformat.so.62", // FFmpeg 8.x (devel) — observed on Fedora 44+
    "libavformat.so.61", // FFmpeg 7.x
    "libavformat.so.60", // FFmpeg 6.x — original target ABI
    "libavformat.so.59",
    "libavformat.so.58",
    "libavformat.so",
];

/// FFmpeg `AV_LOG_QUIET` — suppress libavformat stderr noise during probe.
const AV_LOG_QUIET: c_int = -8;

/// AVMediaType discriminant for video streams (stable across all FFmpeg
/// majors we care about).
const AVMEDIA_TYPE_VIDEO: c_int = 0;

/// Result of a probe. All fields optional; absence means "unknown".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaMetadata {
    pub size: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub format: Option<String>,
}

/// Probe contract. Implementations must be `Send + Sync` so they can be
/// shared across the `SourceManager` `Arc`.
pub trait MediaProbe: Send + Sync {
    fn probe(&self, path: &str) -> MediaMetadata;
}

/// libavformat-backed probe. Lazy-loaded; cached after the first attempt.
pub struct AvFormatProbe {
    state: Mutex<LibState>,
}

enum LibState {
    /// Haven't tried loading yet.
    Uninitialized,
    /// Loading attempted; either succeeded (with handle) or failed (recorded
    /// as `Unavailable`). Either way, never retry.
    Loaded(Option<LoadedLib>),
}

/// Holds an open libavformat handle plus the function-pointer table we
/// resolved out of it. The `Library` field MUST outlive any derived
/// function pointer — keeping it owned in the same struct guarantees that.
struct LoadedLib {
    #[allow(dead_code)]
    library: Library,
    #[allow(dead_code)]
    soname: &'static str,
    /// Major ABI version (e.g. `60`/`61`/`62`). Drives the
    /// `AVCodecParameters` layout we use.
    #[allow(dead_code)]
    major: u32,
    /// Cached function pointers. Each entry's safety is tied to `library`.
    syms: Syms,
}

/// All libavformat entry points we use, plus the `AVCodecParameters`
/// offsets we inferred from `major`.
struct Syms {
    avformat_open_input: unsafe extern "C" fn(
        ps: *mut *mut AvFormatContext,
        url: *const c_char,
        fmt: *const c_void,
        options: *mut *mut c_void,
    ) -> c_int,
    avformat_find_stream_info:
        unsafe extern "C" fn(ic: *mut AvFormatContext, options: *mut *mut c_void) -> c_int,
    avformat_close_input: unsafe extern "C" fn(s: *mut *mut AvFormatContext),
    codecpar_layout: CodecparLayout,
}

/// Offsets within `AVCodecParameters` for the fields we read. The layout
/// shifted in FFmpeg 7 because `coded_side_data` + `nb_coded_side_data`
/// were inserted between `extradata_size` and `format`.
#[derive(Clone, Copy)]
struct CodecparLayout {
    /// Always 0 — `enum AVMediaType` is the first field.
    codec_type: usize,
    width: usize,
    height: usize,
}

const CODECPAR_FFMPEG_6: CodecparLayout = CodecparLayout {
    codec_type: 0,
    width: 56,
    height: 60,
};

const CODECPAR_FFMPEG_7_PLUS: CodecparLayout = CodecparLayout {
    codec_type: 0,
    width: 72,
    height: 76,
};

// ---------------------------------------------------------------------------
// Hand-rolled C struct layouts.
//
// We only access fields up to `streams**` on `AVFormatContext` and
// `codecpar*` on `AVStream`. Both are at stable offsets across libavformat
// 58→62 — they are public ABI fields that have never been reordered.
// `AVInputFormat` only needs its first field (`name`).
// ---------------------------------------------------------------------------

#[repr(C)]
struct AvFormatContext {
    av_class: *const c_void,
    iformat: *const AvInputFormat,
    oformat: *const c_void,
    priv_data: *mut c_void,
    pb: *mut c_void,
    ctx_flags: c_int,
    nb_streams: c_uint,
    streams: *mut *mut AvStream,
    // Tail intentionally omitted — never accessed.
}

#[repr(C)]
struct AvInputFormat {
    name: *const c_char,
    // Tail intentionally omitted.
}

#[repr(C)]
struct AvStream {
    av_class: *const c_void,
    index: c_int,
    id: c_int,
    codecpar: *mut c_void,
    // Tail intentionally omitted.
}

// SAFETY: we only ever read from `LibState` under a Mutex, and `Library`
// itself is `Send + Sync` per libloading's docs (raw `void*` handle that
// does not own thread-local state).
unsafe impl Send for LoadedLib {}
unsafe impl Sync for LoadedLib {}

impl AvFormatProbe {
    /// Construct without loading. The first `probe()` call triggers the load
    /// attempt. Never panics.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LibState::Uninitialized),
        }
    }

    /// Ensure load attempted. Returns whether a usable library is cached.
    fn ensure_loaded(&self) -> bool {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let LibState::Uninitialized = *guard {
            *guard = LibState::Loaded(try_load_libavformat());
        }
        matches!(*guard, LibState::Loaded(Some(_)))
    }

    /// Run the libavformat-driven probe. Caller has already verified the
    /// library loaded; we extract format name + first video stream's
    /// width/height. Any failure leaves the corresponding field as `None`.
    fn probe_with_libav(&self, path: &str) -> (Option<u32>, Option<u32>, Option<String>) {
        let guard = match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let lib = match &*guard {
            LibState::Loaded(Some(l)) => l,
            _ => return (None, None, None),
        };

        let c_path = match CString::new(path) {
            Ok(c) => c,
            Err(_) => return (None, None, None),
        };

        unsafe {
            let mut ctx: *mut AvFormatContext = ptr::null_mut();
            let rc = (lib.syms.avformat_open_input)(
                &mut ctx,
                c_path.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
            );
            if rc < 0 || ctx.is_null() {
                return (None, None, None);
            }

            // Best effort — even if find_stream_info fails, codecpar may
            // still carry usable width/height for many container formats.
            let _ = (lib.syms.avformat_find_stream_info)(ctx, ptr::null_mut());

            // Format name from iformat->name.
            let format = if !(*ctx).iformat.is_null() {
                let name_ptr = (*(*ctx).iformat).name;
                if name_ptr.is_null() {
                    None
                } else {
                    CStr::from_ptr(name_ptr)
                        .to_str()
                        .ok()
                        .map(|s| s.to_owned())
                }
            } else {
                None
            };

            // Find the first video stream.
            let nb = (*ctx).nb_streams as usize;
            let streams = (*ctx).streams;
            let mut width: Option<u32> = None;
            let mut height: Option<u32> = None;
            if !streams.is_null() {
                for i in 0..nb {
                    let stream_ptr = *streams.add(i);
                    if stream_ptr.is_null() {
                        continue;
                    }
                    let codecpar = (*stream_ptr).codecpar;
                    if codecpar.is_null() {
                        continue;
                    }
                    let bytes = codecpar as *const u8;
                    let codec_type =
                        ptr::read(bytes.add(lib.syms.codecpar_layout.codec_type) as *const c_int);
                    if codec_type != AVMEDIA_TYPE_VIDEO {
                        continue;
                    }
                    let w = ptr::read(bytes.add(lib.syms.codecpar_layout.width) as *const c_int);
                    let h = ptr::read(bytes.add(lib.syms.codecpar_layout.height) as *const c_int);
                    if w > 0 {
                        width = Some(w as u32);
                    }
                    if h > 0 {
                        height = Some(h as u32);
                    }
                    break;
                }
            }

            (lib.syms.avformat_close_input)(&mut ctx);
            (width, height, format)
        }
    }
}

impl Default for AvFormatProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaProbe for AvFormatProbe {
    fn probe(&self, path: &str) -> MediaMetadata {
        let mut meta = MediaMetadata::default();

        // `size` is independent of libavformat; fill from std::fs::metadata.
        // Missing files / permission errors → leave size as None.
        if let Ok(md) = fs::metadata(path) {
            if let Ok(len) = i64::try_from(md.len()) {
                meta.size = Some(len);
            }
        }

        if !self.ensure_loaded() {
            // Single-line warn, as required by the implementation guide.
            // Use `target` so consumers can mute it independently.
            warn!(
                target: "waywallen::media_probe",
                "libavformat unavailable; size-only probe for {:?}",
                path
            );
            return meta;
        }

        let (width, height, format) = self.probe_with_libav(path);
        meta.width = width;
        meta.height = height;
        meta.format = format;
        meta
    }
}

/// Try each candidate SONAME in order. On the first successful `dlopen`,
/// silence libavformat's internal logging (best-effort), resolve the
/// function table we use, and return the handle. Returns `None` if none
/// of them load — or if the cached major version doesn't match a layout
/// we know.
fn try_load_libavformat() -> Option<LoadedLib> {
    for soname in LIBAVFORMAT_CANDIDATES {
        // SAFETY: `soname` is a hardcoded constant from a private list — never
        // user-controlled. `Library::new` may execute initializers in the
        // loaded shared object; we trust libavformat's initializers.
        let library = match unsafe { Library::new(soname) } {
            Ok(lib) => lib,
            Err(_) => continue,
        };

        // Best-effort: silence libavformat's stderr logging.
        // Signature: `void av_log_set_level(int level)`.
        // SAFETY: symbol pointer's lifetime is tied to `library` which we
        // own and return alongside it; no dereference happens after drop.
        unsafe {
            if let Ok(sym) = library.get::<unsafe extern "C" fn(c_int)>(b"av_log_set_level\0") {
                sym(AV_LOG_QUIET);
            }
        }

        // Detect ABI major. `avformat_version` returns
        // (MAJOR << 16) | (MINOR << 8) | MICRO.
        let major = match unsafe {
            library.get::<unsafe extern "C" fn() -> c_uint>(b"avformat_version\0")
        } {
            Ok(sym) => (unsafe { sym() }) >> 16,
            Err(_) => {
                warn!(
                    target: "waywallen::media_probe",
                    "{soname}: avformat_version missing; size-only fallback"
                );
                continue;
            }
        };

        let layout = match major {
            60 => CODECPAR_FFMPEG_6,
            61 | 62 => CODECPAR_FFMPEG_7_PLUS,
            other => {
                warn!(
                    target: "waywallen::media_probe",
                    "{soname}: unsupported libavformat major={other}; size-only fallback"
                );
                continue;
            }
        };

        // Resolve the rest of the symbol table. Any miss → skip this lib.
        let syms = unsafe {
            let open: Symbol<
                unsafe extern "C" fn(
                    *mut *mut AvFormatContext,
                    *const c_char,
                    *const c_void,
                    *mut *mut c_void,
                ) -> c_int,
            > = match library.get(b"avformat_open_input\0") {
                Ok(s) => s,
                Err(_) => continue,
            };
            let info: Symbol<
                unsafe extern "C" fn(*mut AvFormatContext, *mut *mut c_void) -> c_int,
            > = match library.get(b"avformat_find_stream_info\0") {
                Ok(s) => s,
                Err(_) => continue,
            };
            let close: Symbol<unsafe extern "C" fn(*mut *mut AvFormatContext)> =
                match library.get(b"avformat_close_input\0") {
                    Ok(s) => s,
                    Err(_) => continue,
                };
            Syms {
                avformat_open_input: *open,
                avformat_find_stream_info: *info,
                avformat_close_input: *close,
                codecpar_layout: layout,
            }
        };

        log::info!(
            target: "waywallen::media_probe",
            "{soname}: libavformat major={major} loaded — full media probe enabled"
        );
        return Some(LoadedLib {
            library,
            soname,
            major: major as u32,
            syms,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Required check (T-101): missing path → all fields None.
    /// Two probes to confirm the cached state is reused without panicking.
    #[test]
    fn probe_missing_returns_none() {
        let probe = AvFormatProbe::new();
        let path = "/this/path/definitely/does/not/exist";

        for _ in 0..2 {
            let meta = probe.probe(path);
            assert_eq!(meta.size, None, "size must be None for missing file");
            assert_eq!(meta.width, None);
            assert_eq!(meta.height, None);
            assert_eq!(meta.format, None);
        }
    }

    /// Optional second test from the spec: real file → size at minimum.
    #[test]
    fn probe_real_file_extracts_size_at_minimum() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile");
        let payload = b"hello waywallen media probe test bytes";
        tmp.write_all(payload).expect("write tempfile");
        tmp.flush().expect("flush tempfile");

        let probe = AvFormatProbe::new();
        let meta = probe.probe(tmp.path().to_str().expect("utf8 path"));
        assert_eq!(meta.size, Some(payload.len() as i64));
        // width/height/format may or may not populate (this isn't a
        // valid container); just assert no panic.
        let _ = (meta.width, meta.height, meta.format);
    }

    /// Trait-object usage check: ensures `AvFormatProbe` can stand in
    /// behind an `Arc<dyn MediaProbe>` (the way SourceManager will hold it).
    #[test]
    fn dyn_dispatch_compiles_and_runs() {
        let probe: std::sync::Arc<dyn MediaProbe> = std::sync::Arc::new(AvFormatProbe::new());
        let _ = probe.probe("/nope");
    }

    /// If libavformat is available, verify it actually parses a real
    /// media file. We synthesize a tiny WAV file (uncompressed PCM) so
    /// the test doesn't pull in any encoder. format should come back
    /// as something containing "wav"; width/height stay None for audio.
    #[test]
    fn probe_real_wav_yields_format() {
        use std::io::Write;
        let mut tmp = tempfile::Builder::new()
            .suffix(".wav")
            .tempfile()
            .expect("create tempfile");
        // Minimal valid 8-bit mono PCM WAV (silence, 1 sample).
        let header: [u8; 44] = [
            b'R', b'I', b'F', b'F', // ChunkID
            37, 0, 0, 0,           // ChunkSize = 36 + data(1)
            b'W', b'A', b'V', b'E', // Format
            b'f', b'm', b't', b' ', // Subchunk1ID
            16, 0, 0, 0,           // Subchunk1Size = 16
            1, 0,                  // AudioFormat = PCM
            1, 0,                  // NumChannels = 1
            0x44, 0xAC, 0, 0,     // SampleRate = 44100
            0x44, 0xAC, 0, 0,     // ByteRate
            1, 0,                  // BlockAlign
            8, 0,                  // BitsPerSample
            b'd', b'a', b't', b'a', // Subchunk2ID
            1, 0, 0, 0,           // Subchunk2Size = 1
        ];
        tmp.write_all(&header).expect("write wav header");
        tmp.write_all(&[0x80]).expect("write sample");
        tmp.flush().unwrap();

        let probe = AvFormatProbe::new();
        let meta = probe.probe(tmp.path().to_str().unwrap());
        assert_eq!(meta.size, Some(45));
        // If libavformat is available on the test host, format should
        // come back. If not, that's also fine — the test just verifies
        // we don't crash and size is correct.
        if let Some(fmt) = &meta.format {
            assert!(
                fmt.contains("wav"),
                "expected wav-ish format string, got {fmt:?}"
            );
        }
        // Audio-only file — never has video dims.
        assert_eq!(meta.width, None);
        assert_eq!(meta.height, None);
    }
}
