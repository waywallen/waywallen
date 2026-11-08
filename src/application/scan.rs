use crate::error::{Error, Result};
use crate::DaemonContext;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    All,
    Source(String),
    Metadata,
}
impl Scope {
    fn merge(&mut self, other: Self) {
        match (&*self, &other) {
            (Self::All, _) => {}
            (Self::Metadata, Self::Metadata) => {}
            (Self::Source(a), Self::Source(b)) if a == b => {}
            _ => *self = Self::All,
        }
    }
}
type Reply = oneshot::Sender<std::result::Result<usize, String>>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_scopes_merge_without_losing_metadata_work() {
        let mut same = Scope::Source("image".into());
        same.merge(Scope::Source("image".into()));
        assert_eq!(same, Scope::Source("image".into()));
        same.merge(Scope::Metadata);
        assert_eq!(same, Scope::All);
        let mut metadata = Scope::Metadata;
        metadata.merge(Scope::Source("image".into()));
        assert_eq!(metadata, Scope::All);
        let mut different = Scope::Source("image".into());
        different.merge(Scope::Source("video".into()));
        assert_eq!(different, Scope::All);
    }
}
#[derive(Default)]
struct State {
    running: bool,
    pending: Option<Scope>,
    replies: Vec<Reply>,
}
#[derive(Default)]
pub(crate) struct ScanCoordinator {
    state: Mutex<State>,
}
impl ScanCoordinator {
    pub async fn request(
        self: &Arc<Self>,
        app: &Arc<DaemonContext>,
        scope: Scope,
    ) -> Result<usize> {
        if *app.shutdown.borrow() {
            return Err(Error::Internal(anyhow::anyhow!("daemon shutting down")));
        }
        let (tx, rx) = oneshot::channel();
        let mut state = self.state.lock().await;
        if let Some(pending) = &mut state.pending {
            pending.merge(scope);
        } else {
            state.pending = Some(scope);
        }
        state.replies.push(tx);
        if !state.running {
            state.running = true;
            let owner = self.clone();
            let app = app.clone();
            tokio::spawn(async move {
                let mut shutdown = app.shutdown_subscribe();
                loop {
                    let (scope, replies) = {
                        let mut state = owner.state.lock().await;
                        let Some(scope) = state.pending.take() else {
                            state.running = false;
                            break;
                        };
                        (scope, std::mem::take(&mut state.replies))
                    };
                    let shutting_down = *shutdown.borrow();
                    let result = if shutting_down {
                        Err(Error::Internal(anyhow::anyhow!("daemon shutting down")))
                    } else {
                        tokio::select! {
                            _ = shutdown.changed() => Err(Error::Internal(anyhow::anyhow!("scan cancelled by shutdown"))),
                            result = super::run_scan_pass(&app, scope) => result,
                        }
                    };
                    let result = result.map_err(|e| format!("{e:#}"));
                    for reply in replies {
                        let _ = reply.send(result.clone());
                    }
                    if *shutdown.borrow() {
                        let mut state = owner.state.lock().await;
                        state.pending = None;
                        state.running = false;
                        for reply in state.replies.drain(..) {
                            let _ = reply.send(Err("daemon shutting down".into()));
                        }
                        break;
                    }
                }
            });
        }
        drop(state);
        rx.await
            .map_err(|e| Error::Internal(e.into()))?
            .map_err(|e| Error::Internal(anyhow::anyhow!(e)))
    }
}
pub(crate) async fn run_media_input_watcher(
    app: Arc<DaemonContext>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(300));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = tick.tick() => {
                if app.scan_in_progress.load(std::sync::atomic::Ordering::SeqCst) || !app.probe.available() { continue; }
                match crate::probe::task::inputs_changed(&app.db, &app.probe).await {
                    Ok(true) => { if let Err(e) = app.scans.request(&app, Scope::Metadata).await { log::error!("media input refresh failed: {e:#}"); } },
                    Ok(false) => {},
                    Err(e) => log::warn!("media input check failed: {e:#}"),
                }
            }
        }
    }
}
