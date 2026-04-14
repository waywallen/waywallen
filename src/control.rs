//! Shared wallpaper control logic.
//!
//! The same operations (apply, next, previous, pause, resume, rescan) are
//! driven from two surfaces — the WebSocket control plane (`ws_server`)
//! and the session-bus `Daemon1` interface (`dbus_iface`) plus the tray.
//! This module owns the canonical implementation so both paths converge
//! on identical semantics (spawn-before-kill, router relink, playlist
//! cursor tracking).

use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::ipc::proto::ControlMsg;
use crate::renderer_manager;
use crate::wallpaper_type::WallpaperEntry;
use crate::AppState;

#[derive(Default)]
pub struct PlaylistState {
    pub ids: Vec<String>,
    pub cursor: usize,
    pub current: Option<String>,
}

impl PlaylistState {
    pub fn refresh(&mut self, ids: Vec<String>) {
        self.ids = ids;
        if self.ids.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.ids.len() {
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
    if let Some(handle) = app.renderer_manager.get(&renderer_id).await {
        app.router.register_renderer(handle).await;
    }
    app.router.relink_all_displays_to(&renderer_id).await;
    for old_id in pre_existing {
        if old_id != renderer_id {
            app.router.unregister_renderer(&old_id).await;
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

/// Advance the playlist cursor by `delta` and apply the result.
pub async fn step(app: &Arc<AppState>, delta: i32) -> Result<String> {
    let ids: Vec<String> = {
        let mgr = app.source_manager.lock().await;
        mgr.list().iter().map(|e| e.id.clone()).collect()
    };

    if ids.is_empty() {
        return Err(anyhow!("playlist is empty"));
    }

    let next_id = {
        let mut playlist = app.playlist.lock().await;
        let current = playlist.current.clone();
        playlist.refresh(ids);
        if let Some(cur) = current.as_deref() {
            playlist.locate(cur);
        }
        let len = playlist.ids.len() as i32;
        let mut next = (playlist.cursor as i32 + delta) % len;
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
    let (count, ids) = {
        let mut mgr = app.source_manager.lock().await;
        mgr.scan_all()?;
        let ids: Vec<String> = mgr.list().iter().map(|e| e.id.clone()).collect();
        (mgr.list().len(), ids)
    };
    app.playlist.lock().await.refresh(ids);
    Ok(count)
}
