//! Wire messages exchanged across the waywallen IPC fabric.
//!
//! Hand-mirrored on the C++ side in `open-wallpaper-engine/host/proto.hpp`.
//! Keep this file small: every field added here must be mirrored there.

use serde::{Deserialize, Serialize};

/// Protocol version. Bumped on any breaking schema change.
pub const PROTOCOL_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// Daemon → renderer-host  (control plane)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlMsg {
    Hello {
        client: String,
        version: u32,
    },
    LoadScene {
        pkg: String,
        assets: String,
        fps: u32,
        width: u32,
        height: u32,
    },
    Play,
    Pause,
    Mouse {
        x: f64,
        y: f64,
    },
    SetFps {
        fps: u32,
    },
    Shutdown,
}

// ---------------------------------------------------------------------------
// Renderer-host → daemon  (events)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventMsg {
    /// Renderer reached the `inited` state and is ready to render.
    Ready,

    /// Sent exactly once after the first successful render, carrying the
    /// full set of swapchain image FDs as ancillary SCM_RIGHTS data.
    /// FDs live for the lifetime of the ExSwapchain.
    BindBuffers {
        /// Number of FDs attached to this message.
        count: u32,
        /// DRM fourcc, e.g. DRM_FORMAT_ABGR8888.
        fourcc: u32,
        width: u32,
        height: u32,
        /// Plane-0 stride in bytes.
        stride: u32,
        /// DRM format modifier, or DRM_FORMAT_MOD_LINEAR / _INVALID.
        modifier: u64,
        /// Plane-0 offset inside each buffer.
        plane_offset: u64,
        /// Per-buffer memory allocation size.
        sizes: Vec<u64>,
    },

    /// A frame was produced. `image_index` indexes into the FD array
    /// delivered by the preceding `BindBuffers`. No FDs are attached here
    /// unless `has_sync_fd` is true.
    FrameReady {
        image_index: u32,
        seq: u64,
        ts_ns: u64,
        /// If true, exactly one FD is attached to this message: a sync_file.
        #[serde(default)]
        has_sync_fd: bool,
    },

    Error {
        msg: String,
    },
}

// ---------------------------------------------------------------------------
// Display client ↔ daemon  (legacy protocol, to be replaced)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ViewerMsg {
    Hello { client: String, version: u32 },
    Subscribe { renderer_id: String },
    Unsubscribe,
}
