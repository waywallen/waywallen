//! Lightweight background-task manager.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

/// How long the supervisor waits after `abort_all` for in-flight tasks
/// to clean up before it returns anyway. The daemon's own `async_main`
/// applies a shorter runtime shutdown timeout on top of this, so setting
/// a generous value here is safe.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(3);

type BoxedResultFut = Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>;
type BoxedResultFn = Box<dyn FnOnce() -> Result<()> + Send + 'static>;

enum TaskMsg {
    Async { name: String, fut: BoxedResultFut },
    Blocking { name: String, func: BoxedResultFn },
}

/// Handle callers use to submit work. Cheap to clone; the actual state
/// lives in the supervisor task.
pub struct TaskManager {
    tx: mpsc::UnboundedSender<TaskMsg>,
}

impl TaskManager {
    /// Start a supervisor bound to the given daemon shutdown watch.
    /// Returns an `Arc<TaskManager>` for callers; the supervisor runs
    /// in the background and joins itself to the Tokio runtime.
    pub fn spawn(shutdown_rx: watch::Receiver<bool>) -> std::sync::Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(supervisor(rx, shutdown_rx));
        std::sync::Arc::new(Self { tx })
    }

    /// Submit an async task. `name` appears in completion / failure logs
    /// and is the only identity the task has in Iter 1.
    pub fn spawn_async<F>(&self, name: impl Into<String>, fut: F)
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let name = name.into();
        if let Err(e) = self.tx.send(TaskMsg::Async {
            name: name.clone(),
            fut: Box::pin(fut),
        }) {
            log::warn!("task '{name}' dropped: supervisor is gone ({e})");
        }
    }

    /// Submit a blocking task (Lua scan, synchronous filesystem work,
    /// anything that would otherwise stall the reactor). Runs on the
    /// Tokio blocking pool.
    pub fn spawn_blocking<F>(&self, name: impl Into<String>, func: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let name = name.into();
        if let Err(e) = self.tx.send(TaskMsg::Blocking {
            name: name.clone(),
            func: Box::new(func),
        }) {
            log::warn!("task '{name}' dropped: supervisor is gone ({e})");
        }
    }
}

async fn supervisor(
    mut rx: mpsc::UnboundedReceiver<TaskMsg>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut set: JoinSet<(String, Result<()>)> = JoinSet::new();
    log::info!("TaskManager supervisor started");

    loop {
        tokio::select! {
            biased;

            // Shutdown beats task scheduling.
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }

            // New work arrives via the mpsc channel.
            msg = rx.recv() => match msg {
                Some(TaskMsg::Async { name, fut }) => {
                    set.spawn(async move { (name, fut.await) });
                }
                Some(TaskMsg::Blocking { name, func }) => {
                    set.spawn_blocking(move || (name, func()));
                }
                None => {
                    // All TaskManager handles dropped — no more work can
                    // arrive. Drain remaining tasks and exit.
                    break;
                }
            },

            // Completed tasks: log outcome and free the slot.
            Some(joined) = set.join_next() => log_join(joined),
        }
    }

    log::info!(
        "TaskManager supervisor draining ({} tasks in flight)",
        set.len()
    );
    set.abort_all();
    let deadline = tokio::time::sleep(SHUTDOWN_DEADLINE);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            _ = &mut deadline => {
                log::warn!(
                    "TaskManager shutdown timeout: {} task(s) did not finish in {:?}",
                    set.len(),
                    SHUTDOWN_DEADLINE
                );
                break;
            }
            opt = set.join_next() => match opt {
                Some(joined) => log_join(joined),
                None => break,
            },
        }
    }
    log::info!("TaskManager supervisor exited");
}

fn log_join(joined: Result<(String, Result<()>), tokio::task::JoinError>) {
    match joined {
        Ok((name, Ok(()))) => log::info!("task '{name}' completed"),
        Ok((name, Err(e))) => log::warn!("task '{name}' failed: {e:#}"),
        Err(e) if e.is_cancelled() => log::info!("task cancelled during shutdown"),
        Err(e) => log::warn!("task join error: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn async_task_runs_to_completion() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let hit = Arc::new(AtomicU32::new(0));
        let h = hit.clone();
        tm.spawn_async("unit/async-ok", async move {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        // Give supervisor + task a moment to run.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hit.load(Ordering::SeqCst), 1);
        let _ = tx.send(true);
    }

    #[tokio::test]
    async fn blocking_task_runs_to_completion() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let hit = Arc::new(AtomicU32::new(0));
        let h = hit.clone();
        tm.spawn_blocking("unit/blocking-ok", move || {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hit.load(Ordering::SeqCst), 1);
        let _ = tx.send(true);
    }

    #[tokio::test]
    async fn shutdown_aborts_long_async_task() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let finished = Arc::new(AtomicU32::new(0));
        let f = finished.clone();
        tm.spawn_async("unit/long-sleeper", async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            f.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = tx.send(true);
        // Allow supervisor to drain.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(finished.load(Ordering::SeqCst), 0, "task should have been aborted");
    }
}
