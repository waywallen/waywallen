use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::program::Program;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::RwLock;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use super::stat::{self, Dependency};
use crate::process_stdio::ChildStderrCapture;

const MAX_LINE: usize = 2 * 1024 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(3);
const ITEM_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
#[error("probe protocol error: {0}")]
struct ProtocolError(String);

fn protocol(message: impl Into<String>) -> anyhow::Error {
    ProtocolError(message.into()).into()
}

#[derive(Debug, Clone)]
pub struct ProbeJob {
    pub id: i64,
    pub path: String,
    pub kind: String,
    pub source: String,
    pub wp_type: String,
    pub sub_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProbeResult {
    pub request_id: String,
    pub status: String,
    #[serde(default)]
    pub selected_path: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub width: Option<i32>,
    #[serde(default)]
    pub height: Option<i32>,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Default)]
pub struct ProbeBatch {
    pub errors: Vec<String>,
}

pub struct PreparedPrograms {
    programs: HashMap<String, (Arc<Program>, String)>,
}

impl PreparedPrograms {
    pub fn input_kind(&self, source: &str, ty: &str, sub_type: &str) -> Option<String> {
        if let Some((program, revision)) = self
            .programs
            .get(source)
            .filter(|(program, _)| program.types.iter().any(|t| t == ty))
        {
            return Some(format!("lua:{}:{revision}:{ty}:{sub_type}", program.key));
        }
        matches!(ty, "image" | "video").then(|| {
            if sub_type == "timeline" {
                "timeline".into()
            } else {
                "media".into()
            }
        })
    }

    fn configurations(&self) -> Vec<Program> {
        let mut programs: Vec<_> = self
            .programs
            .values()
            .map(|(program, _)| program.as_ref().clone())
            .collect();
        programs.sort_by(|a, b| a.key.cmp(&b.key));
        programs.dedup_by(|a, b| a.key == b.key);
        programs
    }
}

pub struct ProbeService {
    programs: RwLock<HashMap<String, Arc<Program>>>,
    tool: Option<PathBuf>,
    automatic: bool,
    missing: AtomicBool,
    lock: Mutex<()>,
    item_timeout: Duration,
}

impl Default for ProbeService {
    fn default() -> Self {
        Self::new()
    }
}

impl ProbeService {
    pub fn new() -> Self {
        Self {
            programs: RwLock::new(HashMap::new()),
            tool: None,
            automatic: true,
            missing: AtomicBool::new(false),
            lock: Mutex::new(()),
            item_timeout: ITEM_TIMEOUT,
        }
    }

    pub fn with_tool(tool: Option<PathBuf>) -> Self {
        Self {
            programs: RwLock::new(HashMap::new()),
            tool,
            automatic: false,
            missing: AtomicBool::new(false),
            lock: Mutex::new(()),
            item_timeout: ITEM_TIMEOUT,
        }
    }

    pub fn available(&self) -> bool {
        self.tool_path().is_some()
    }

    pub fn replace_programs(&self, programs: HashMap<String, Arc<Program>>) {
        *self.programs.write().expect("probe programs poisoned") = programs;
    }

    pub async fn prepare_programs(&self) -> Result<PreparedPrograms> {
        let programs = self
            .programs
            .read()
            .expect("probe programs poisoned")
            .clone();
        tokio::task::spawn_blocking(move || {
            let mut revisions = HashMap::new();
            let mut prepared = HashMap::new();
            for (source, program) in programs {
                let revision = revisions
                    .entry(program.key.clone())
                    .or_insert_with(|| program.cache_key())
                    .clone();
                prepared.insert(source, (program, revision));
            }
            PreparedPrograms { programs: prepared }
        })
        .await
        .context("prepare probe programs")
    }

    pub fn program_is_current(&self, prepared: &PreparedPrograms, source: &str) -> bool {
        let programs = self.programs.read().expect("probe programs poisoned");
        match (prepared.programs.get(source), programs.get(source)) {
            (Some((before, _)), Some(now)) => Arc::ptr_eq(before, now),
            (None, None) => true,
            _ => false,
        }
    }

    fn tool_path(&self) -> Option<PathBuf> {
        if self.automatic {
            locate_tool()
        } else {
            self.tool.clone()
        }
        .filter(|path| executable(path))
    }

    #[cfg(test)]
    pub(super) fn with_timeout(tool: PathBuf, timeout: Duration) -> Self {
        let mut service = Self::with_tool(Some(tool));
        service.item_timeout = timeout;
        service
    }

    pub async fn execute(
        &self,
        prepared: &PreparedPrograms,
        jobs: Vec<ProbeJob>,
        results: tokio::sync::mpsc::Sender<(ProbeJob, ProbeResult)>,
    ) -> Result<Option<ProbeBatch>> {
        if jobs.is_empty() {
            return Ok(Some(ProbeBatch::default()));
        }
        let _lock = self.lock.lock().await;
        let tool = self.tool_path();
        let Some(tool) = tool else {
            if !self.missing.swap(true, Ordering::Relaxed) {
                log::warn!("waywallen-probe not found; skipping media probe");
            }
            return Ok(None);
        };
        self.missing.store(false, Ordering::Relaxed);
        let programs = prepared.configurations();
        let count = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(4)
            .min(jobs.len());
        let jobs = Arc::new(Mutex::new(VecDeque::from(jobs)));
        let mut workers = JoinSet::new();
        for index in 0..count {
            let results = results.clone();
            let item_timeout = self.item_timeout;
            let jobs = jobs.clone();
            let tool = tool.clone();
            let programs = programs.clone();
            workers.spawn(async move {
                let mut batch = ProbeBatch::default();
                let mut worker = None;
                let mut restarts = 0;
                loop {
                    let Some(job) = jobs.lock().await.pop_front() else {
                        break;
                    };
                    if worker.is_none() {
                        match Worker::start(&tool, index, &programs).await {
                            Ok(value) => worker = Some(value),
                            Err(error) => {
                                batch
                                    .errors
                                    .push(format!("probe worker startup: {error:#}"));
                                break;
                            }
                        }
                    }
                    let current = worker.as_mut().unwrap();
                    let mut dependencies = vec![stat::snapshot(&job.path)];
                    let result = tokio::time::timeout(
                        item_timeout,
                        current.request(&job, &mut dependencies),
                    )
                    .await;
                    let value = match result {
                        Ok(Ok(value)) => value,
                        outcome => {
                            if let Ok(Err(error)) = &outcome {
                                if error.is::<ProtocolError>() {
                                    batch.errors.push(format!("{}: {error:#}", job.path));
                                    current.stop().await;
                                    worker = None;
                                    break;
                                }
                            }
                            let error = match outcome {
                                Ok(Err(e)) => format!("{e:#}"),
                                _ => "probe timed out".into(),
                            };
                            log::error!("probe {}: {error}", job.path);
                            let value = ProbeResult {
                                request_id: job.id.to_string(),
                                status: "failed".into(),
                                selected_path: None,
                                dependencies,
                                width: None,
                                height: None,
                                size: None,
                                error: Some(error),
                            };
                            current.stop().await;
                            worker = None;
                            restarts += 1;
                            value
                        }
                    };
                    if results.send((job, value)).await.is_err() {
                        break;
                    }
                    if restarts >= 3 {
                        batch
                            .errors
                            .push("probe worker restart budget exhausted".into());
                        break;
                    }
                }
                if let Some(mut worker) = worker {
                    if let Err(error) = worker.finish().await {
                        batch.errors.push(format!("probe worker finish: {error:#}"));
                    }
                    worker.stop().await;
                }
                batch
            });
        }
        drop(results);
        let mut batch = ProbeBatch::default();
        while let Some(result) = workers.join_next().await {
            let result = result.context("join probe worker")?;
            batch.errors.extend(result.errors);
        }
        Ok(Some(batch))
    }
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn locate_tool() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("WAYWALLEN_PROBE_BIN") {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = std::env::current_exe() {
        let sibling = path.parent()?.join("waywallen-probe");
        if executable(&sibling) {
            return Some(sibling);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join("waywallen-probe"))
        .find(|p| executable(p))
}

struct Worker {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    stderr: ChildStderrCapture,
}

impl Worker {
    async fn start(path: &Path, index: usize, programs: &[Program]) -> Result<Self> {
        let mut command = Command::new(path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                let memory = libc::rlimit {
                    rlim_cur: 2 * 1024 * 1024 * 1024,
                    rlim_max: 2 * 1024 * 1024 * 1024,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let stderr = ChildStderrCapture::spawn(
            child.stderr.take().unwrap(),
            format!("waywallen::probe::worker{index}"),
        );
        let mut worker = Self {
            input: child.stdin.take(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            stderr,
        };
        let ready = match tokio::time::timeout(START_TIMEOUT, worker.message()).await {
            Ok(Ok(ready)) => ready,
            outcome => {
                worker.stop().await;
                bail!("probe handshake failed: {outcome:?}");
            }
        };
        if ready["event"] != "ready" || ready["protocol_version"] != 2 {
            worker.stop().await;
            bail!("unsupported probe handshake: {ready}");
        }
        let mut configuration =
            serde_json::to_vec(&serde_json::json!({"event":"configure", "programs":programs}))?;
        configuration.push(b'\n');
        if configuration.len() > MAX_LINE {
            worker.stop().await;
            bail!("probe configuration exceeds message limit");
        }
        let configure = async {
            worker
                .input
                .as_mut()
                .unwrap()
                .write_all(&configuration)
                .await?;
            worker.message().await
        };
        match tokio::time::timeout(START_TIMEOUT, configure).await {
            Ok(Ok(message)) if message["event"] == "configured" => {}
            outcome => {
                worker.stop().await;
                bail!("probe configuration failed: {outcome:?}");
            }
        }
        Ok(worker)
    }

    async fn message(&mut self) -> Result<serde_json::Value> {
        let mut bytes = Vec::new();
        loop {
            let buffer = self.output.fill_buf().await?;
            if buffer.is_empty() {
                bail!("probe stdout closed");
            }
            let end = buffer.iter().position(|b| *b == b'\n').map(|i| i + 1);
            let count = end.unwrap_or(buffer.len());
            if bytes.len() + count > MAX_LINE {
                return Err(protocol("message exceeds limit"));
            }
            bytes.extend_from_slice(&buffer[..count]);
            self.output.consume(count);
            if end.is_some() {
                return serde_json::from_slice(&bytes).map_err(|e| protocol(e.to_string()));
            }
        }
    }

    async fn request(
        &mut self,
        job: &ProbeJob,
        dependencies: &mut Vec<Dependency>,
    ) -> Result<ProbeResult> {
        self.stderr
            .set_context(format!("request={} path={:?}", job.id, job.path));
        let mut request = serde_json::to_vec(
            &serde_json::json!({"request_id":job.id.to_string(), "path":job.path, "kind":job.kind, "source":job.source, "wp_type":job.wp_type, "sub_type":job.sub_type, "selection_version":1}),
        )?;
        request.push(b'\n');
        self.input
            .as_mut()
            .ok_or_else(|| anyhow!("probe stdin closed"))?
            .write_all(&request)
            .await?;
        let resolved = self.message().await?;
        if resolved["event"] != "resolved" || resolved["request_id"] != job.id.to_string() {
            return Err(protocol("unexpected resolved message"));
        }
        *dependencies = serde_json::from_value(resolved["dependencies"].clone())
            .map_err(|e| protocol(e.to_string()))?;
        validate_dependencies(dependencies, &job.path)?;
        let value = loop {
            let message = self.message().await?;
            if message["event"] == "resolved" && message["request_id"] == job.id.to_string() {
                *dependencies = serde_json::from_value(message["dependencies"].clone())
                    .map_err(|e| protocol(e.to_string()))?;
                validate_dependencies(dependencies, &job.path)?;
            } else {
                break message;
            }
        };
        if value["event"] != "result" {
            return Err(protocol("expected result"));
        }
        let value: ProbeResult =
            serde_json::from_value(value).map_err(|e| protocol(e.to_string()))?;
        if value.request_id != job.id.to_string() {
            return Err(protocol("mismatched result"));
        }
        validate_dependencies(&value.dependencies, &job.path)?;
        if value
            .selected_path
            .as_deref()
            .is_some_and(|path| !local_path(path))
        {
            return Err(protocol("invalid selected path"));
        }
        if !matches!(
            value.status.as_str(),
            "ok" | "failed" | "unsupported" | "input_changed"
        ) || value.size.is_some_and(|n| n < 0)
            || value.width.is_some_and(|n| n <= 0)
            || value.height.is_some_and(|n| n <= 0)
            || value.width.is_some() != value.height.is_some()
            || (value.status == "ok" && value.width.is_none() && value.size.is_none())
        {
            return Err(protocol("invalid metadata"));
        }
        Ok(value)
    }

    async fn stop(&mut self) {
        self.input.take();
        if tokio::time::timeout(Duration::from_millis(500), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_millis(500), self.child.wait()).await;
        }
        let _ = self.stderr.drain(Duration::from_millis(250)).await;
    }

    async fn finish(&mut self) -> Result<()> {
        self.input.take();
        let remaining = tokio::time::timeout(Duration::from_millis(500), self.output.fill_buf())
            .await
            .context("probe stdout did not close")??;
        if !remaining.is_empty() {
            return Err(protocol("unexpected trailing output"));
        }
        let status = tokio::time::timeout(Duration::from_millis(500), self.child.wait())
            .await
            .context("probe worker did not exit")??;
        if !status.success() {
            bail!("probe worker exited {status}");
        }
        Ok(())
    }
}

fn validate_dependencies(values: &[Dependency], root: &str) -> Result<()> {
    if values.is_empty()
        || values.len() > 8192
        || !values.iter().any(|d| d.path == root)
        || values.iter().any(|d| {
            !local_path(&d.path)
                || d.size.is_some_and(|n| n < 0)
                || !matches!(d.kind.as_str(), "file" | "directory" | "missing")
        })
    {
        return Err(protocol("invalid dependencies"));
    }
    Ok(())
}

fn local_path(path: &str) -> bool {
    Path::new(path).is_absolute() && path.len() <= 16384 && !path.contains('\0')
}
