//! Shared wallpaper control logic.
//!
//! The same operations (apply, next, previous, pause, resume, rescan) are
//! driven from three surfaces — the WebSocket control plane (`ws_server`),
//! the session-bus `Daemon1` interface (`dbus_iface`), and the tray menu
//! (`tray`). This module owns the canonical implementation so all three
//! paths converge on identical semantics.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::Mutex;

use crate::ipc::proto::ControlMsg;
use crate::renderer_manager;
use crate::wallpaper_type::WallpaperEntry;
use crate::AppState;

/// Playlist cursor over the flat wallpaper list emitted by the source
/// manager. Refreshed on every successful `list_snapshot`; `cursor` clamps
/// to the new length.
#[derive(Default)]
pub struct PlaylistState {
    pub ids: Vec<String>,
    pub cursor: usize,
    /// Id of the wallpaper currently applied by the daemon, regardless of
    /// whether it came from Next/Previous or WS `WallpaperApply`.
    pub current: Option<String>,
}

impl PlaylistState {
    pub fn refresh(&mut self, ids: Vec<String>) {
        self.ids = ids;
        if self.cursor >= self.ids.len() {
            self.cursor = 0;
        }
    }

    pub fn locate(&mut self, id: &str) {
        if let Some(pos) = self.ids.iter().position(|x| x == id) {
            self.cursor = pos;
        }
    }
}

pub struct ApplyResult {
    pub renderer_id: String,
    pub entry: WallpaperEntry,
}

/// Apply the wallpaper identified by `id`. Spawn-before-kill: the new
/// renderer is brought up first so any active display frame loop can
/// rebind before the old broadcast closes.
pub async fn apply_wallpaper_by_id(
    app: &Arc<AppState>,
    id: &str,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<ApplyResult> {
    let entry = {
        let mgr = app.source_manager.lock().await;
        mgr.get(id).cloned()
    };
    let entry = entry.ok_or_else(|| anyhow!("wallpaper '{id}' not found"))?;

    if app
        .renderer_manager
        .registry()
        .resolve(&entry.wp_type)
        .is_none()
    {
        return Err(anyhow!("no renderer for wallpaper type '{}'", entry.wp_type));
    }

    let pre_existing: Vec<String> = app.renderer_manager.list().await;

    let width = if width == 0 { 1920 } else { width };
    let height = if height == 0 { 1080 } else { height };
    let fps = if fps == 0 { 30 } else { fps };
    let spawn_req = renderer_manager::SpawnRequest {
        wp_type: entry.wp_type.clone(),
        metadata: entry.metadata.clone(),
        width,
        height,
        fps,
        test_pattern: false,
    };
    let renderer_id = app.renderer_manager.spawn(spawn_req).await?;
    for old_id in pre_existing {
        if old_id != renderer_id {
            let _ = app.renderer_manager.kill(&old_id).await;
        }
    }

    {
        let mut playlist = app.playlist.lock().await;
        playlist.locate(&entry.id);
        playlist.current = Some(entry.id.clone());
    }

    Ok(ApplyResult { renderer_id, entry })
}

/// Refresh the playlist cursor from the current source manager listing and
/// advance by `delta` (±1). Returns the newly-applied wallpaper id.
pub async fn step(app: &Arc<AppState>, delta: i32) -> Result<String> {
    let (ids, current) = {
        let mgr = app.source_manager.lock().await;
        let ids: Vec<String> = mgr.list().iter().map(|e| e.id.clone()).collect();
        let current = app.playlist.lock().await.current.clone();
        (ids, current)
    };

    if ids.is_empty() {
        return Err(anyhow!("playlist is empty"));
    }

    let next_id = {
        let mut playlist = app.playlist.lock().await;
        playlist.refresh(ids.clone());
        if let Some(cur) = current.as_deref() {
            playlist.locate(cur);
        }
        let len = playlist.ids.len() as i32;
        let cursor = playlist.cursor as i32;
        let mut next = (cursor + delta) % len;
        if next < 0 {
            next += len;
        }
        playlist.cursor = next as usize;
        playlist.ids[playlist.cursor].clone()
    };

    apply_wallpaper_by_id(app, &next_id, 0, 0, 0).await?;
    Ok(next_id)
}

pub async fn pause_all(app: &Arc<AppState>) -> Result<()> {
    send_all(app, ControlMsg::Pause).await
}

pub async fn resume_all(app: &Arc<AppState>) -> Result<()> {
    send_all(app, ControlMsg::Play).await
}

async fn send_all(app: &Arc<AppState>, msg: ControlMsg) -> Result<()> {
    let ids = app.renderer_manager.list().await;
    for id in ids {
        if let Err(e) = app.renderer_manager.send_control(&id, msg.clone()).await {
            log::warn!("control {id}: {e}");
        }
    }
    Ok(())
}

pub async fn rescan(app: &Arc<AppState>) -> Result<usize> {
    let mut mgr = app.source_manager.lock().await;
    mgr.scan_all()?;
    let count = mgr.list().len();
    let ids: Vec<String> = mgr.list().iter().map(|e| e.id.clone()).collect();
    drop(mgr);
    app.playlist.lock().await.refresh(ids);
    Ok(count)
}

/// Helper for refreshing the playlist snapshot (e.g. after WallpaperList).
pub async fn refresh_playlist(app: &Arc<AppState>) {
    let ids: Vec<String> = {
        let mgr = app.source_manager.lock().await;
        mgr.list().iter().map(|e| e.id.clone()).collect()
    };
    app.playlist.lock().await.refresh(ids);
}

pub struct PlaylistHandle(pub Arc<Mutex<PlaylistState>>);
