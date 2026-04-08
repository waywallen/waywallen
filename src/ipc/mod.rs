//! IPC layer shared by the waywallen daemon, the renderer host subprocess,
//! and external viewer clients.
//!
//! The wire format is a length-prefixed JSON frame:
//!   [u32 BE length] [JSON body]
//! File descriptors (e.g. DMA-BUF FDs in `BindBuffers`) are passed as
//! ancillary SCM_RIGHTS data on the same `sendmsg(2)` call.
//!
//! This iteration (0) is deliberately std-blocking; the Tokio wiring
//! lives on top in iterations 2/3.

pub mod proto;
pub mod uds;
