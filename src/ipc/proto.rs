//! Compatibility re-exports for IPC message types.
//!
//! The authoritative definitions live in
//! `crate::ipc::generated::{Request, Event}`, auto-generated from
//! `protocol/waywallen_ipc_v1.xml`. This module publishes them under
//! the historical `ControlMsg` / `EventMsg` names so existing call
//! sites compile unchanged.

pub use crate::ipc::generated::{
    DecodeError, Event as EventMsg, Request as ControlMsg, PROTOCOL_NAME, PROTOCOL_VERSION,
};
