use tokio::sync::{broadcast, watch};

const DEFAULT_BUS_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteDownloadState {
    Pending,
    Downloading,
    Done,
    Timeout,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteDownloadTransfer {
    pub received_bytes: u64,
    pub total_bytes: Option<u64>,
}

/// Transient process-wide notifications.
#[derive(Debug, Clone)]
pub enum GlobalEvent {
    /// Source plugins finished loading and the initial DB sync ran.
    /// Dependent startup work can now read source/plugin state.
    SourcesReady,
    /// At least one display has registered with the router and is
    /// available to wallpaper assignment.
    DisplayReady,
    /// The startup-restore task succeeded.
    /// Carries the applied wallpaper id, if any.
    RestoreApplied(Option<String>),
    /// The startup-restore task failed at some stage. The string is
    /// the formatted error for log/UI subscribers.
    RestoreFailed(String),
    /// Core services are up (WS bound, DBus published). Latched so
    /// late subscribers can still observe readiness.
    DaemonReady,
    /// A wallpaper sync finished successfully; `count` is the total
    /// entry count after the swap.
    SyncFinished {
        count: usize,
    },
    /// A wallpaper sync failed.
    /// The string is the formatted error.
    SyncFailed(String),
    /// One or more libraries were just added — manually via
    /// `LibraryAdd` or `LibraryAutoDetect`.
    LibrariesAdded {
        paths: Vec<String>,
    },
    /// Daemon-side runtime state changed.
    /// Receivers re-snapshot via the `StatusSync` builder.
    StatusChanged,
    /// The persisted settings table just changed (either via
    /// `SettingsSet` RPC or startup reconciliation).
    SettingsChanged,
    WallpaperHiddenChanged {
        wallpaper_ids: Vec<String>,
        hidden: bool,
    },
    /// External display client failed the UDS handshake because of
    /// malformed or unsupported protocol data.
    DisplayConnectionFailed {
        client_name: String,
        client_protocol_version: u32,
        error_code: u32,
        reason: String,
    },
    RemoteDownloadProgress {
        source_id: String,
        id: String,
        state: RemoteDownloadState,
        error: String,
        transfer: Option<RemoteDownloadTransfer>,
    },
    QrLoginProgress {
        session_id: String,
        plugin_id: String,
        action_id: String,
        state: i32,
        qr_image: String,
        display_value: crate::plugin::i18n::LocalizedText,
        error: crate::plugin::i18n::LocalizedText,
        title: crate::plugin::i18n::LocalizedText,
        instruction: crate::plugin::i18n::LocalizedText,
    },
    PlaylistChanged,
    PluginChanged,
    PluginStateChanged,
    PluginUpdateChanged,
    PluginRestartFailed {
        plugin_id: String,
        error: String,
    },
    TaskProgress(crate::tasks::TaskProgress),
}

pub struct EventBus {
    bus: broadcast::Sender<GlobalEvent>,
    sources_ready: watch::Sender<bool>,
    display_ready: watch::Sender<bool>,
    daemon_ready: watch::Sender<bool>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_BUS_CAPACITY)
    }
}

impl EventBus {
    pub fn with_capacity(cap: usize) -> Self {
        let (bus, _) = broadcast::channel(cap);
        let (sources_ready, _) = watch::channel(false);
        let (display_ready, _) = watch::channel(false);
        let (daemon_ready, _) = watch::channel(false);
        Self {
            bus,
            sources_ready,
            display_ready,
            daemon_ready,
        }
    }

    /// Publish a transient event and latch any readiness marker it implies.
    /// Re-publishing a marker is idempotent.
    pub fn publish(&self, e: GlobalEvent) {
        // send_replace succeeds even when no receivers exist; send would
        // fail because we drop the initial receiver immediately.
        match &e {
            GlobalEvent::SourcesReady => {
                self.sources_ready.send_replace(true);
            }
            GlobalEvent::DisplayReady => {
                self.display_ready.send_replace(true);
            }
            GlobalEvent::DaemonReady => {
                self.daemon_ready.send_replace(true);
            }
            _ => {}
        }
        let _ = self.bus.send(e);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<GlobalEvent> {
        self.bus.subscribe()
    }

    /// Clone of the broadcast sender for callers that need to publish
    /// transient events from sites without an EventBus reference.
    pub fn sender(&self) -> broadcast::Sender<GlobalEvent> {
        self.bus.clone()
    }

    pub fn watch_sources_ready(&self) -> watch::Receiver<bool> {
        self.sources_ready.subscribe()
    }

    pub fn watch_display_ready(&self) -> watch::Receiver<bool> {
        self.display_ready.subscribe()
    }

    pub fn watch_daemon_ready(&self) -> watch::Receiver<bool> {
        self.daemon_ready.subscribe()
    }

    pub fn is_sources_ready(&self) -> bool {
        *self.sources_ready.borrow()
    }

    pub fn is_display_ready(&self) -> bool {
        *self.display_ready.borrow()
    }

    pub fn is_daemon_ready(&self) -> bool {
        *self.daemon_ready.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn phase_marker_is_latched_for_late_subscribers() {
        let bus = EventBus::default();
        bus.publish(GlobalEvent::SourcesReady);
        let mut rx = bus.watch_sources_ready();
        // Late subscribe still sees the latched value immediately —
        // wait_for returns the borrowed-ref or, if the value already
        let v = tokio::time::timeout(Duration::from_millis(50), rx.wait_for(|v| *v))
            .await
            .expect("late subscribe blocked")
            .expect("watch closed");
        assert!(*v);
    }

    #[tokio::test]
    async fn transient_event_visible_to_subscribers_only_after_subscribe() {
        let bus = EventBus::default();
        // Subscribe first so we don't miss anything.
        let mut rx = bus.subscribe();
        bus.publish(GlobalEvent::RestoreApplied(Some("abc".into())));
        let evt = tokio::time::timeout(Duration::from_millis(50), rx.recv())
            .await
            .expect("recv timeout")
            .expect("recv error");
        match evt {
            GlobalEvent::RestoreApplied(Some(s)) => assert_eq!(s, "abc"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn daemon_ready_is_latched_for_late_subscribers() {
        let bus = EventBus::default();
        assert!(!bus.is_daemon_ready());
        bus.publish(GlobalEvent::DaemonReady);
        assert!(bus.is_daemon_ready());
        let mut rx = bus.watch_daemon_ready();
        let v = tokio::time::timeout(Duration::from_millis(50), rx.wait_for(|v| *v))
            .await
            .expect("late subscribe blocked")
            .expect("watch closed");
        assert!(*v);
    }
}
