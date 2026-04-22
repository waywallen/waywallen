//! Lightweight background-task manager.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// How long the supervisor waits after `abort_all` for in-flight tasks
/// to clean up before it returns anyway. The daemon's own `async_main`
/// applies a shorter runtime shutdown timeout on top of this.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(3);

/// Capacity of the broadcast channel used for `TaskEvent`. Slow
/// subscribers that lag behind will see `RecvError::Lagged` and
/// must re-snapshot via `list()` — the supervisor never stalls on
/// them.
const EVENT_CHANNEL_CAP: usize = 256;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Unique per-process task identifier. Monotonically increasing; the
/// first task submitted gets 1.
pub type TaskId = u64;

/// Coarse categorization of a task's purpose. Lets UIs group tasks
/// (e.g. "scanning" vs "applying wallpaper") without parsing names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// One-shot startup work (source scan + DB sync + playlist seed).
    Startup,
    /// Fallback bucket for everything not otherwise classified.
    Generic,
}

impl TaskKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskKind::Startup => "startup",
            TaskKind::Generic => "generic",
        }
    }
}

/// Lifecycle state of a task record. `Failed` carries the error
/// message formatted with `{:#}` so stringly-typed consumers
/// (DBus, logs) can show a one-line reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

impl TaskState {
    /// Short wire-friendly label. Used by DBus `ListTasks`.
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskState::Running => "running",
            TaskState::Completed => "completed",
            TaskState::Failed(_) => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: TaskId,
    pub kind: TaskKind,
    pub name: String,
    /// Milliseconds since UNIX epoch when the task was submitted.
    pub started_at_ms: i64,
    pub state: TaskState,
}

/// Lifecycle events broadcast to every subscriber. `Started` carries
/// a full record so late-joined subscribers can reconstruct state
/// without racing against `list()`.
#[derive(Debug, Clone)]
pub enum TaskEvent {
    Started(TaskRecord),
    Completed(TaskId),
    Failed(TaskId, String),
    Cancelled(TaskId),
}

// ---------------------------------------------------------------------------
// TaskManager — public handle
// ---------------------------------------------------------------------------

type BoxedResultFut = Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>;
type BoxedResultFn = Box<dyn FnOnce() -> Result<()> + Send + 'static>;

enum TaskMsg {
    Async { id: TaskId, name: String, fut: BoxedResultFut },
    Blocking { id: TaskId, name: String, func: BoxedResultFn },
}

pub struct TaskManager {
    tx: mpsc::UnboundedSender<TaskMsg>,
    next_id: AtomicU64,
    records: Arc<RwLock<HashMap<TaskId, TaskRecord>>>,
    events: broadcast::Sender<TaskEvent>,
    /// Per-task cooperative cancellation handles. Entries are added at
    /// submit time and removed once the task transitions out of
    /// `Running` (regardless of success / failure / cancel). Async
    /// tasks are wrapped in `select!` against the token before being
    /// handed to the supervisor; blocking tasks ignore cancellation.
    cancel_tokens: Arc<RwLock<HashMap<TaskId, CancellationToken>>>,
    /// Optional dedup key → currently-Running TaskId. `spawn_async_unique`
    /// cancels any prior task under the same key before spawning a new
    /// one. Stale entries (pointing to a finished task) are GC'd in
    /// `handle_join`, so the map's footprint tracks active uniques.
    unique_keys: Arc<RwLock<HashMap<String, TaskId>>>,
}

impl TaskManager {
    /// Start a supervisor bound to the daemon's shutdown watch. The
    /// returned handle is `Arc`-shareable; every clone feeds the same
    /// supervisor.
    pub fn spawn(shutdown_rx: watch::Receiver<bool>) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (events_tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        let records: Arc<RwLock<HashMap<TaskId, TaskRecord>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let cancel_tokens: Arc<RwLock<HashMap<TaskId, CancellationToken>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let unique_keys: Arc<RwLock<HashMap<String, TaskId>>> =
            Arc::new(RwLock::new(HashMap::new()));

        tokio::spawn(supervisor(
            rx,
            shutdown_rx,
            records.clone(),
            events_tx.clone(),
            cancel_tokens.clone(),
            unique_keys.clone(),
        ));

        Arc::new(Self {
            tx,
            next_id: AtomicU64::new(1),
            records,
            events: events_tx,
            cancel_tokens,
            unique_keys,
        })
    }

    /// Submit an async task. Returns the freshly-assigned `TaskId` so
    /// callers can correlate their submission with later events / logs.
    /// The task is wrapped in a `select!` against a per-task
    /// `CancellationToken`; calling [`cancel`](Self::cancel) flips it.
    pub fn spawn_async<F>(
        &self,
        kind: TaskKind,
        name: impl Into<String>,
        fut: F,
    ) -> TaskId
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let name = name.into();
        let token = CancellationToken::new();
        self.cancel_tokens
            .write()
            .unwrap()
            .insert(id, token.clone());
        self.record_started(id, kind, name.clone());
        let wrapped = async move {
            tokio::select! {
                _ = token.cancelled() => Err(anyhow::anyhow!("cancelled")),
                r = fut => r,
            }
        };
        if let Err(e) = self.tx.send(TaskMsg::Async {
            id,
            name: name.clone(),
            fut: Box::pin(wrapped),
        }) {
            log::warn!("task '{name}' (id {id}) dropped: supervisor is gone ({e})");
            self.cancel_tokens.write().unwrap().remove(&id);
            self.finalize(id, TaskState::Failed("supervisor gone".into()));
        }
        id
    }

    /// Like [`spawn_async`] but de-duplicates by `key`. If a Running
    /// task already exists under the same key, it is cancelled before
    /// the new one is spawned. Useful for things like
    /// `apply_wallpaper(display_id)` where rapid repeats should
    /// supersede earlier attempts instead of stacking.
    pub fn spawn_async_unique<F>(
        &self,
        kind: TaskKind,
        key: impl Into<String>,
        name: impl Into<String>,
        fut: F,
    ) -> TaskId
    where
        F: Future<Output = Result<()>> + Send + 'static,
    {
        let key = key.into();
        let prev = self.unique_keys.read().unwrap().get(&key).copied();
        if let Some(prev_id) = prev {
            // Best-effort: cancel returns false if the task already
            // finished — that's fine, the unique_keys entry is then
            // stale and gets overwritten below.
            self.cancel(prev_id);
        }
        let id = self.spawn_async(kind, name, fut);
        self.unique_keys.write().unwrap().insert(key, id);
        id
    }

    /// Cooperatively cancel a Running task. Returns `true` if a token
    /// existed (the task was Running at call time) — the task may not
    /// observe the cancellation immediately if it's mid-syscall in a
    /// non-async section. No-op for blocking tasks.
    pub fn cancel(&self, id: TaskId) -> bool {
        let token = self.cancel_tokens.read().unwrap().get(&id).cloned();
        let Some(token) = token else { return false };
        token.cancel();
        // Pre-mark the record as Cancelled so handle_join's eventual
        // observation of "task returned Err(cancelled)" doesn't promote
        // the state to Failed.
        let mut prev_state_was_running = false;
        if let Some(rec) = self.records.write().unwrap().get_mut(&id) {
            if matches!(rec.state, TaskState::Running) {
                rec.state = TaskState::Cancelled;
                prev_state_was_running = true;
            }
        }
        if prev_state_was_running {
            let _ = self.events.send(TaskEvent::Cancelled(id));
        }
        true
    }

    /// Submit a blocking task. Runs on the Tokio blocking pool.
    pub fn spawn_blocking<F>(
        &self,
        kind: TaskKind,
        name: impl Into<String>,
        func: F,
    ) -> TaskId
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let name = name.into();
        self.record_started(id, kind, name.clone());
        if let Err(e) = self.tx.send(TaskMsg::Blocking {
            id,
            name: name.clone(),
            func: Box::new(func),
        }) {
            log::warn!("task '{name}' (id {id}) dropped: supervisor is gone ({e})");
            self.finalize(id, TaskState::Failed("supervisor gone".into()));
        }
        id
    }

    /// Snapshot of all currently-tracked tasks (running + finished).
    /// The registry is trimmed to a bounded history — see
    /// `TRIM_COMPLETED_ABOVE`.
    pub fn list(&self) -> Vec<TaskRecord> {
        self.records.read().unwrap().values().cloned().collect()
    }

    /// Subscribe to lifecycle events. Late subscribers miss historical
    /// events and should re-snapshot via [`list`](Self::list) on start.
    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.events.subscribe()
    }

    fn record_started(&self, id: TaskId, kind: TaskKind, name: String) {
        let record = TaskRecord {
            id,
            kind,
            name,
            started_at_ms: now_ms(),
            state: TaskState::Running,
        };
        {
            let mut recs = self.records.write().unwrap();
            recs.insert(id, record.clone());
            trim_finished(&mut recs);
        }
        let _ = self.events.send(TaskEvent::Started(record));
    }

    fn finalize(&self, id: TaskId, state: TaskState) {
        let event = match &state {
            TaskState::Completed => Some(TaskEvent::Completed(id)),
            TaskState::Failed(msg) => Some(TaskEvent::Failed(id, msg.clone())),
            TaskState::Cancelled => Some(TaskEvent::Cancelled(id)),
            TaskState::Running => None,
        };
        if let Some(rec) = self.records.write().unwrap().get_mut(&id) {
            rec.state = state;
        }
        if let Some(e) = event {
            let _ = self.events.send(e);
        }
    }
}

/// Cap the per-process record history so long-running daemons don't
/// accumulate unbounded finished entries. Runtime cost of the trim is
/// amortized across inserts.
const TRIM_FINISHED_ABOVE: usize = 512;

fn trim_finished(recs: &mut HashMap<TaskId, TaskRecord>) {
    if recs.len() <= TRIM_FINISHED_ABOVE {
        return;
    }
    let mut finished: Vec<TaskId> = recs
        .iter()
        .filter_map(|(id, r)| (!matches!(r.state, TaskState::Running)).then_some(*id))
        .collect();
    // Drop oldest (smallest ids) first until we're back under cap.
    finished.sort_unstable();
    let to_drop = recs.len().saturating_sub(TRIM_FINISHED_ABOVE);
    for id in finished.into_iter().take(to_drop) {
        recs.remove(&id);
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

async fn supervisor(
    mut rx: mpsc::UnboundedReceiver<TaskMsg>,
    mut shutdown_rx: watch::Receiver<bool>,
    records: Arc<RwLock<HashMap<TaskId, TaskRecord>>>,
    events: broadcast::Sender<TaskEvent>,
    cancel_tokens: Arc<RwLock<HashMap<TaskId, CancellationToken>>>,
    unique_keys: Arc<RwLock<HashMap<String, TaskId>>>,
) {
    // The supervisor's JoinSet tasks resolve to (TaskId, Result) so the
    // joiner can look up records and emit the right TaskEvent.
    let mut set: JoinSet<(TaskId, Result<()>)> = JoinSet::new();
    log::info!("TaskManager supervisor started");

    loop {
        tokio::select! {
            biased;

            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }

            msg = rx.recv() => match msg {
                Some(TaskMsg::Async { id, name: _, fut }) => {
                    set.spawn(async move { (id, fut.await) });
                }
                Some(TaskMsg::Blocking { id, name: _, func }) => {
                    set.spawn_blocking(move || (id, func()));
                }
                None => break,
            },

            Some(joined) = set.join_next() => {
                handle_join(joined, &records, &events, &cancel_tokens, &unique_keys);
            }
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
                Some(joined) => handle_join(joined, &records, &events, &cancel_tokens, &unique_keys),
                None => break,
            },
        }
    }
    log::info!("TaskManager supervisor exited");
}

fn handle_join(
    joined: Result<(TaskId, Result<()>), tokio::task::JoinError>,
    records: &Arc<RwLock<HashMap<TaskId, TaskRecord>>>,
    events: &broadcast::Sender<TaskEvent>,
    cancel_tokens: &Arc<RwLock<HashMap<TaskId, CancellationToken>>>,
    unique_keys: &Arc<RwLock<HashMap<String, TaskId>>>,
) {
    let (id, name, observed_state) = match joined {
        Ok((id, Ok(()))) => {
            let name = lookup_name(records, id);
            (id, name, TaskState::Completed)
        }
        Ok((id, Err(e))) => {
            let name = lookup_name(records, id);
            let msg = format!("{e:#}");
            (id, name, TaskState::Failed(msg))
        }
        Err(e) if e.is_cancelled() => {
            // JoinError::Cancelled is the JoinSet::abort_all path on
            // shutdown — the task didn't get a chance to run its
            // wrapper, so its record was never updated. Don't have an
            // id to clean up here; bulk cleanup happens on shutdown.
            log::info!("task aborted during shutdown");
            return;
        }
        Err(e) => {
            log::warn!("task join error: {e}");
            return;
        }
    };

    // GC the per-task cancel token regardless of outcome.
    cancel_tokens.write().unwrap().remove(&id);
    // GC any unique_keys mapping that pointed at us.
    unique_keys.write().unwrap().retain(|_, v| *v != id);

    // Pre-cancellation: if `cancel(id)` already moved the record to
    // Cancelled, leave it alone — the future returned Err("cancelled")
    // but the user's intent was a cancel, not a failure.
    let already_cancelled = matches!(
        records.read().unwrap().get(&id).map(|r| r.state.clone()),
        Some(TaskState::Cancelled)
    );
    if already_cancelled {
        log::info!("task '{name}' (id {id}) cancelled");
        return;
    }

    {
        let mut recs = records.write().unwrap();
        if let Some(rec) = recs.get_mut(&id) {
            rec.state = observed_state.clone();
        }
    }
    match &observed_state {
        TaskState::Completed => {
            log::info!("task '{name}' (id {id}) completed");
            let _ = events.send(TaskEvent::Completed(id));
        }
        TaskState::Failed(msg) => {
            log::warn!("task '{name}' (id {id}) failed: {msg}");
            let _ = events.send(TaskEvent::Failed(id, msg.clone()));
        }
        _ => {}
    }
}

fn lookup_name(records: &Arc<RwLock<HashMap<TaskId, TaskRecord>>>, id: TaskId) -> String {
    records
        .read()
        .unwrap()
        .get(&id)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| format!("id={id}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    async fn wait_for<F: Fn() -> bool>(pred: F, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if pred() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn async_task_runs_to_completion() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let hit = Arc::new(AtomicU32::new(0));
        let h = hit.clone();
        let id = tm.spawn_async(TaskKind::Generic, "unit/async-ok", async move {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(id >= 1);
        assert!(
            wait_for(|| hit.load(Ordering::SeqCst) == 1, Duration::from_secs(1)).await,
            "task never ran"
        );
        let _ = tx.send(true);
    }

    #[tokio::test]
    async fn blocking_task_runs_to_completion() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let hit = Arc::new(AtomicU32::new(0));
        let h = hit.clone();
        tm.spawn_blocking(TaskKind::Generic, "unit/blocking-ok", move || {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(
            wait_for(|| hit.load(Ordering::SeqCst) == 1, Duration::from_secs(1)).await,
            "blocking task never ran"
        );
        let _ = tx.send(true);
    }

    #[tokio::test]
    async fn shutdown_aborts_long_async_task() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let finished = Arc::new(AtomicU32::new(0));
        let f = finished.clone();
        tm.spawn_async(
            TaskKind::Generic,
            "unit/long-sleeper",
            async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                f.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = tx.send(true);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(finished.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn list_and_events_reflect_lifecycle() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let mut events = tm.subscribe();

        let id = tm.spawn_async(TaskKind::Generic, "unit/list", async move { Ok(()) });

        // Immediately after submit, the task should appear in list() as Running.
        let snap = tm.list();
        assert!(snap.iter().any(|r| r.id == id && matches!(r.state, TaskState::Running)));

        // Started event fires synchronously during spawn_async.
        match tokio::time::timeout(Duration::from_millis(100), events.recv())
            .await
            .expect("no Started event")
            .unwrap()
        {
            TaskEvent::Started(r) => assert_eq!(r.id, id),
            other => panic!("expected Started, got {other:?}"),
        }

        // Completed event follows once the future resolves.
        let done = tokio::time::timeout(Duration::from_secs(1), events.recv()).await;
        match done.expect("no completion event").unwrap() {
            TaskEvent::Completed(i) => assert_eq!(i, id),
            other => panic!("expected Completed, got {other:?}"),
        }

        assert!(wait_for(
            || tm.list().iter().any(|r| r.id == id && matches!(r.state, TaskState::Completed)),
            Duration::from_secs(1)
        ).await);

        let _ = tx.send(true);
    }

    #[tokio::test]
    async fn cancel_marks_running_task_cancelled() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let mut events = tm.subscribe();

        let id = tm.spawn_async(TaskKind::Generic, "unit/cancel-me", async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        });
        // Drain Started.
        let _ = tokio::time::timeout(Duration::from_millis(100), events.recv()).await;

        // Wait until supervisor has picked it up so the wrapper future
        // is actually awaiting on the cancel token.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(tm.cancel(id), "cancel returned false on a running task");

        match tokio::time::timeout(Duration::from_millis(500), events.recv())
            .await
            .expect("no Cancelled event")
            .unwrap()
        {
            TaskEvent::Cancelled(i) => assert_eq!(i, id),
            other => panic!("expected Cancelled, got {other:?}"),
        }

        // Final state observed by list() must stay Cancelled, not flip
        // to Failed when the wrapper future returns Err("cancelled").
        assert!(wait_for(
            || tm
                .list()
                .iter()
                .any(|r| r.id == id && matches!(r.state, TaskState::Cancelled)),
            Duration::from_secs(1)
        )
        .await);

        // Cancel-token entry should be GC'd after handle_join runs.
        assert!(wait_for(|| !tm.cancel(id), Duration::from_secs(1)).await,
            "cancel token leaked");
        let _ = tx.send(true);
    }

    #[tokio::test]
    async fn unique_key_supersedes_prior_running_task() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);

        let first = tm.spawn_async_unique(
            TaskKind::Generic,
            "apply/output-1",
            "unit/first",
            async move {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(())
            },
        );
        // Let supervisor pick it up.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let second = tm.spawn_async_unique(
            TaskKind::Generic,
            "apply/output-1",
            "unit/second",
            async move { Ok(()) },
        );
        assert_ne!(first, second);

        // First should end up Cancelled, second Completed.
        assert!(wait_for(
            || {
                let snap = tm.list();
                let f = snap.iter().find(|r| r.id == first);
                let s = snap.iter().find(|r| r.id == second);
                matches!(f.map(|r| &r.state), Some(TaskState::Cancelled))
                    && matches!(s.map(|r| &r.state), Some(TaskState::Completed))
            },
            Duration::from_secs(1)
        )
        .await);
        let _ = tx.send(true);
    }

    #[tokio::test]
    async fn failed_task_surfaces_error_string() {
        let (tx, rx) = watch::channel(false);
        let tm = TaskManager::spawn(rx);
        let mut events = tm.subscribe();

        let id = tm.spawn_async(
            TaskKind::Generic,
            "unit/failing",
            async move { anyhow::bail!("nope") },
        );

        // Drain Started.
        let _ = tokio::time::timeout(Duration::from_millis(100), events.recv()).await;

        let failed = tokio::time::timeout(Duration::from_secs(1), events.recv()).await;
        match failed.expect("no event").unwrap() {
            TaskEvent::Failed(i, msg) => {
                assert_eq!(i, id);
                assert!(msg.contains("nope"), "msg was {msg:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        let _ = tx.send(true);
    }
}
