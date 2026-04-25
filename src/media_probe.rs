//! Fail-soft media metadata probe.
//!
//! Lazy-loads `libavformat` via `libloading` (no link-time dep on FFmpeg) and
//! exposes a tiny `MediaProbe` trait whose `probe` method is total: it never
//! returns `Err` and never panics. When libavformat cannot be loaded — or its
//! ABI doesn't match what we coded against — the probe degrades gracefully to
//! filling only `size` from `std::fs::metadata`.
//!
//! The cross-agent contract for this module lives in
//! `.plans/media-meta/notes.md`. Don't change shapes here without updating
//! that file.

use std::ffi::c_int;
use std::fs;
use std::sync::Mutex;

use libloading::Library;
use log::warn;

/// Public, owner-controlled list of `SONAME`s we'll try to dlopen, in order.
/// NEVER take this from caller-controlled input — see security rules in
/// `.plans/media-meta/plan.md`.
const LIBAVFORMAT_CANDIDATES: &[&str] = &[
    "libavformat.so.60", // FFmpeg 6.x — primary ABI we target
    "libavformat.so.59",
    "libavformat.so.58",
    "libavformat.so",
];

/// FFmpeg `AV_LOG_QUIET` — suppress libavformat stderr noise during probe.
const AV_LOG_QUIET: c_int = -8;

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

/// Holds an open libavformat handle. The `Library` field MUST outlive any
/// derived function pointer; keeping it owned here ensures that.
#[allow(dead_code)] // we deliberately retain the library handle for ABI safety
struct LoadedLib {
    library: Library,
    soname: &'static str,
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

        // PUNT (documented in notes.md / plan T-101): we deliberately skip
        // dereferencing libavformat's AVFormatContext / AVStream / AVCodec-
        // Parameters to extract width/height/format. Those struct layouts are
        // not stable across FFmpeg majors, and a wrong offset is UB. The
        // contract explicitly tolerates this partial implementation: dev-
        // bridge will see `width / height / format == None` and treat them
        // as "unknown" rather than "absent". A future task can flesh out the
        // FFI layouts (scoped to a private `mod ffi` with FFmpeg 6.x header
        // citations) once we're ready to invest in version-pinned testing.
        //
        // We still attempt to load the library so that:
        //   - failure mode is the same regardless of host (test parity), and
        //   - load+silence happens once, eagerly, so production logs stay
        //     quiet later.
        if self.ensure_loaded() {
            // Loaded successfully — but we don't introspect the file.
            // (No warn! here — successful load is the happy path.)
        } else {
            // Single-line warn, as required by the implementation guide.
            // Use `target` so consumers can mute it independently.
            warn!(
                target: "waywallen::media_probe",
                "libavformat unavailable; size-only probe for {:?}",
                path
            );
        }

        meta
    }
}

/// Try each candidate SONAME in order. On the first successful `dlopen`,
/// silence libavformat's internal logging (best-effort) and return the
/// handle. Returns `None` if none of them load.
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

        return Some(LoadedLib {
            library,
            soname,
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
        // width/height/format are punted; just assert they don't panic.
        let _ = (meta.width, meta.height, meta.format);
    }

    /// Trait-object usage check: ensures `AvFormatProbe` can stand in
    /// behind an `Arc<dyn MediaProbe>` (the way SourceManager will hold it).
    #[test]
    fn dyn_dispatch_compiles_and_runs() {
        let probe: std::sync::Arc<dyn MediaProbe> = std::sync::Arc::new(AvFormatProbe::new());
        let _ = probe.probe("/nope");
    }
}
