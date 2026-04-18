//! Node/Link routing model.
//!
//! Phase 1 introduces an explicit `Router` that owns a `RoutingTable`
//! (renderers, displays, links) plus a per-renderer subscription task.
//! The `display_endpoint` no longer subscribes to renderer broadcasts
//! directly — it receives `DisplayOutEvent`s on a per-display mpsc
//! produced by the router.
//!
//! Phase 1 policy still mirrors the legacy single-wallpaper behavior:
//! every newly-registered display auto-links to whichever renderer is
//! "first" in the table; `WallpaperApply` re-points every link to the
//! new renderer. Phase 2 will replace this with per-display config and
//! reference-counted lifecycle.

pub mod router;
pub mod table;

pub use router::{
    DisplayHandle, DisplayLinkSnapshot, DisplayOutEvent, DisplayRegistration, DisplaySnapshot,
    Router, RouterEvent,
};
pub use table::{Link, LinkId, RoutingTable};
