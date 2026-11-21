use std::ffi::{c_char, c_float, c_int, c_void, CStr, CString};
use std::ptr::NonNull;

use super::window::{AudioPcmWindow, PcmWindowAssembler};

const ERROR_CAPACITY: usize = 512;

#[link(name = "waywallen_pulse_adapter", kind = "static")]
unsafe extern "C" {
    fn ww_pulse_capture_open(
        sink_name: *const c_char,
        error_code: *mut c_int,
        error: *mut c_char,
        error_capacity: usize,
    ) -> *mut c_void;
    fn ww_pulse_capture_close(capture: *mut c_void);
    fn ww_pulse_capture_read(
        capture: *mut c_void,
        samples: *mut c_float,
        frame_capacity: usize,
        generation: *mut u64,
    ) -> usize;
    fn ww_pulse_capture_failed(
        capture: *mut c_void,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn ww_pulse_playback_observer_open(
        error_code: *mut c_int,
        error: *mut c_char,
        error_capacity: usize,
    ) -> *mut c_void;
    fn ww_pulse_playback_observer_close(observer: *mut c_void);
    fn ww_pulse_playback_observer_snapshot(
        observer: *mut c_void,
        streams: *mut PulsePlaybackStream,
        capacity: usize,
    ) -> usize;
    fn ww_pulse_playback_observer_failed(
        observer: *mut c_void,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn ww_pulse_list_sinks(
        sinks: *mut RawPulseSink,
        capacity: usize,
        total: *mut usize,
        error_code: *mut c_int,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn ww_pulse_list_playback_streams(
        streams: *mut PulsePlaybackStream,
        capacity: usize,
        total: *mut usize,
        error_code: *mut c_int,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn ww_pulse_move_playback_streams(
        sink_name: *const c_char,
        indices: *const u32,
        count: usize,
        moved: *mut usize,
        error_code: *mut c_int,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
}

const SINK_NAME_CAPACITY: usize = 256;

#[repr(C)]
struct RawPulseSink {
    name: [c_char; SINK_NAME_CAPACITY],
    description: [c_char; SINK_NAME_CAPACITY],
    is_default: c_int,
}

/// One output device (sink) offered by the sound server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseSink {
    pub name: String,
    pub description: String,
    pub is_default: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PulsePlaybackStream {
    pub index: u32,
    pub process_id: i32,
    pub corked: c_int,
    pub muted: c_int,
    pub has_nonzero_volume: c_int,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureErrorKind {
    LibraryUnavailable,
    MissingSymbol,
    ServerUnavailable,
    MonitorUnavailable,
    StreamFailed,
    OutOfMemory,
    Unknown,
}

#[derive(Debug)]
pub struct CaptureError {
    pub kind: CaptureErrorKind,
    pub message: String,
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for CaptureError {}

pub trait AudioCaptureBackend: Send {
    fn snapshot(&mut self, captured_at_ns: u64) -> Result<Option<AudioPcmWindow>, CaptureError>;
    fn discard(&mut self);
}

pub struct PulseCapture {
    handle: NonNull<c_void>,
    assembler: PcmWindowAssembler,
    scratch: [f32; 2048],
}

// The opaque handle is created, polled, and destroyed by one audio worker.
unsafe impl Send for PulseCapture {}

impl PulseCapture {
    /// Records the monitor of `sink`, or of the default sink when `None`.
    /// A sink that no longer exists falls back to the default one.
    pub fn open(sink: Option<&str>) -> Result<Self, CaptureError> {
        let sink = sink
            .map(CString::new)
            .transpose()
            .map_err(|_| CaptureError {
                kind: CaptureErrorKind::Unknown,
                message: "audio output name contains a NUL byte".to_string(),
            })?;
        let mut code = 0;
        let mut error = [0 as c_char; ERROR_CAPACITY];
        let handle = unsafe {
            ww_pulse_capture_open(
                sink.as_ref().map_or(std::ptr::null(), |name| name.as_ptr()),
                &mut code,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        let Some(handle) = NonNull::new(handle) else {
            return Err(CaptureError {
                kind: error_kind(code),
                message: c_message(&error),
            });
        };
        Ok(Self {
            handle,
            assembler: PcmWindowAssembler::default(),
            scratch: [0.0; 2048],
        })
    }

    fn status(&self) -> Result<(), CaptureError> {
        let mut error = [0 as c_char; ERROR_CAPACITY];
        let code = unsafe {
            ww_pulse_capture_failed(self.handle.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if code == 0 {
            Ok(())
        } else {
            Err(CaptureError {
                kind: error_kind(code),
                message: c_message(&error),
            })
        }
    }
}

impl AudioCaptureBackend for PulseCapture {
    fn snapshot(&mut self, captured_at_ns: u64) -> Result<Option<AudioPcmWindow>, CaptureError> {
        self.status()?;
        let mut generation = 0;
        let frames = unsafe {
            ww_pulse_capture_read(
                self.handle.as_ptr(),
                self.scratch.as_mut_ptr(),
                self.scratch.len() / 2,
                &mut generation,
            )
        };
        self.assembler
            .ingest_interleaved(generation, &self.scratch[..frames * 2]);
        Ok(self.assembler.snapshot(captured_at_ns))
    }

    fn discard(&mut self) {
        loop {
            let mut generation = 0;
            let frames = unsafe {
                ww_pulse_capture_read(
                    self.handle.as_ptr(),
                    self.scratch.as_mut_ptr(),
                    self.scratch.len() / 2,
                    &mut generation,
                )
            };
            self.assembler.ingest_interleaved(generation, &[]);
            if frames == 0 {
                break;
            }
        }
        self.assembler.discard();
    }
}

impl Drop for PulseCapture {
    fn drop(&mut self) {
        unsafe { ww_pulse_capture_close(self.handle.as_ptr()) };
    }
}

pub struct PulsePlaybackObserver {
    handle: NonNull<c_void>,
}

// The opaque handle is polled and destroyed by one audio worker. PulseAudio
// callbacks synchronize snapshots inside the adapter.
unsafe impl Send for PulsePlaybackObserver {}

impl PulsePlaybackObserver {
    pub fn open() -> Result<Self, CaptureError> {
        let mut code = 0;
        let mut error = [0 as c_char; ERROR_CAPACITY];
        let handle =
            unsafe { ww_pulse_playback_observer_open(&mut code, error.as_mut_ptr(), error.len()) };
        let Some(handle) = NonNull::new(handle) else {
            return Err(CaptureError {
                kind: error_kind(code),
                message: c_message(&error),
            });
        };
        Ok(Self { handle })
    }

    pub fn snapshot(&self) -> Result<Vec<PulsePlaybackStream>, CaptureError> {
        self.status()?;
        let mut streams = Vec::new();
        loop {
            let count = unsafe {
                ww_pulse_playback_observer_snapshot(
                    self.handle.as_ptr(),
                    streams.as_mut_ptr(),
                    streams.capacity(),
                )
            };
            if count <= streams.capacity() {
                unsafe { streams.set_len(count) };
                return Ok(streams);
            }
            streams.reserve(count - streams.capacity());
        }
    }

    fn status(&self) -> Result<(), CaptureError> {
        let mut error = [0 as c_char; ERROR_CAPACITY];
        let code = unsafe {
            ww_pulse_playback_observer_failed(self.handle.as_ptr(), error.as_mut_ptr(), error.len())
        };
        if code == 0 {
            Ok(())
        } else {
            Err(CaptureError {
                kind: error_kind(code),
                message: c_message(&error),
            })
        }
    }
}

impl Drop for PulsePlaybackObserver {
    fn drop(&mut self) {
        unsafe { ww_pulse_playback_observer_close(self.handle.as_ptr()) };
    }
}

/// Lists the output devices known to the sound server.
///
/// Opens a short-lived connection, so call it from a blocking context.
pub fn list_sinks() -> Result<Vec<PulseSink>, CaptureError> {
    let mut capacity = 16;
    loop {
        let mut raw: Vec<RawPulseSink> = Vec::with_capacity(capacity);
        let mut total = 0usize;
        let mut code = 0;
        let mut error = [0 as c_char; ERROR_CAPACITY];
        let rc = unsafe {
            ww_pulse_list_sinks(
                raw.as_mut_ptr(),
                capacity,
                &mut total,
                &mut code,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if rc != 0 {
            return Err(CaptureError {
                kind: error_kind(code),
                message: c_message(&error),
            });
        }
        if total > capacity {
            capacity = total;
            continue;
        }
        unsafe { raw.set_len(total) };
        return Ok(raw
            .iter()
            .map(|sink| PulseSink {
                name: c_message(&sink.name),
                description: c_message(&sink.description),
                is_default: sink.is_default != 0,
            })
            .collect());
    }
}

/// Lists every playback stream on the sound server.
///
/// Opens a short-lived connection, so call it from a blocking context.
pub fn list_playback_streams() -> Result<Vec<PulsePlaybackStream>, CaptureError> {
    let mut capacity = 32;
    loop {
        let mut streams: Vec<PulsePlaybackStream> = Vec::with_capacity(capacity);
        let mut total = 0usize;
        let mut code = 0;
        let mut error = [0 as c_char; ERROR_CAPACITY];
        let rc = unsafe {
            ww_pulse_list_playback_streams(
                streams.as_mut_ptr(),
                capacity,
                &mut total,
                &mut code,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if rc != 0 {
            return Err(CaptureError {
                kind: error_kind(code),
                message: c_message(&error),
            });
        }
        if total > capacity {
            capacity = total;
            continue;
        }
        unsafe { streams.set_len(total) };
        return Ok(streams);
    }
}

/// Moves playback streams (by index) to `sink`, or to the current default
/// sink when `None`. Returns how many streams the server moved.
///
/// Opens a short-lived connection, so call it from a blocking context.
pub fn move_playback_streams(sink: Option<&str>, indices: &[u32]) -> Result<usize, CaptureError> {
    if indices.is_empty() {
        return Ok(0);
    }
    let sink = sink
        .map(CString::new)
        .transpose()
        .map_err(|_| CaptureError {
            kind: CaptureErrorKind::Unknown,
            message: "audio output name contains a NUL byte".to_string(),
        })?;
    let mut moved = 0usize;
    let mut code = 0;
    let mut error = [0 as c_char; ERROR_CAPACITY];
    let rc = unsafe {
        ww_pulse_move_playback_streams(
            sink.as_ref().map_or(std::ptr::null(), |name| name.as_ptr()),
            indices.as_ptr(),
            indices.len(),
            &mut moved,
            &mut code,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if rc != 0 {
        return Err(CaptureError {
            kind: error_kind(code),
            message: c_message(&error),
        });
    }
    Ok(moved)
}

fn error_kind(code: i32) -> CaptureErrorKind {
    match code {
        1 => CaptureErrorKind::LibraryUnavailable,
        2 => CaptureErrorKind::MissingSymbol,
        3 => CaptureErrorKind::ServerUnavailable,
        4 => CaptureErrorKind::MonitorUnavailable,
        5 => CaptureErrorKind::StreamFailed,
        6 => CaptureErrorKind::OutOfMemory,
        _ => CaptureErrorKind::Unknown,
    }
}

fn c_message(buffer: &[c_char]) -> String {
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}
