//! IPC layer shared by the waywallen daemon and renderer subprocesses.
//!
//! The `generated` submodule is emitted at build time by
//! `build.rs` (via the `wayproto-gen` tool) from
//! `protocol/waywallen_ipc_v1.xml`. It defines `Request` (daemon →
//! subprocess control plane, aliased as `ControlMsg`), `Event`
//! (subprocess → daemon, aliased as `EventMsg`), per-opcode
//! constants, and the binary encode/decode implementations.
//!
//! `uds` layers length-prefixed framing + `SCM_RIGHTS` ancillary fd
//! handling on top of the generated codec. The wire format mirrors
//! `waywallen-display-v1`:
//!
//!     [u16 LE opcode][u16 LE total_length][body...]

#[allow(dead_code, clippy::all)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/ipc_generated.rs"));
}

pub mod proto;
pub mod uds;
