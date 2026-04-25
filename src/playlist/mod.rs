//! Playlist subsystem: filter predicates (P2), playlist state (P3+),
//! rotation scheduler (P5+).
//!
//! Today this module exposes only [`filter::Filter`] and
//! [`filter::AspectClass`]; later iterations will add membership
//! resolution, cursor stepping, and the rotator service.

pub mod filter;
pub mod resolve;
pub mod rotator;
pub mod state;

pub use filter::{AspectClass, Filter};
pub use rotator::{RotationConfig, RotationHandle};
pub use state::{Mode, PlaylistState};
