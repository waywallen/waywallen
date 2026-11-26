use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, watch, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

use super::browser_protocol::{self, BrowserCommand, BrowserEvent, FRAME_LIMIT};
use super::i18n::LocalizedText;
use super::source::BrowserLoginOutcome;
use super::source::{redact_secrets, SourceManager};
use super::web_helper_registry::{WebHelper, WebHelperDef, WebHelperRegistry};
use crate::control_proto::BrowserLoginState as State;
use crate::events::GlobalEvent;

struct Control {
    cancel: CancellationToken,
    done: CancellationToken,
    helper_plugin: String,
}

pub(crate) struct BrowserLoginManager {
    sources: Arc<SourceManager>,
    events: broadcast::Sender<GlobalEvent>,
    shutdown: watch::Receiver<bool>,
    sessions: Mutex<HashMap<String, Control>>,
    slot: Arc<Semaphore>,
    pub(crate) helpers: WebHelperRegistry,
}

impl BrowserLoginManager {
    pub(crate) fn new(
        sources: Arc<SourceManager>,
        events: broadcast::Sender<GlobalEvent>,
        shutdown: watch::Receiver<bool>,
        helpers: WebHelperRegistry,
    ) -> Arc<Self> {
        Arc::new(Self {
            sources,
            events,
            shutdown,
            sessions: Mutex::new(HashMap::new()),
            slot: Arc::new(Semaphore::new(1)),
            helpers,
        })
    }

    pub(crate) async fn start(
        self: &Arc<Self>,
        plugin: &str,
        action: &str,
        helper_id: &str,
    ) -> Result<String> {
        let permit = self
            .slot
            .clone()
            .try_acquire_owned()
            .context("another browser login is already active")?;
        let helper = self.helpers.select(helper_id)?;
        let availability = helper.availability(true).await;
        if !availability.available {
            bail!("{}", availability.error);
        }
        anyhow::ensure!(
            helper.active(),
            "selected browser helper was removed or reloaded"
        );
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let done = CancellationToken::new();
        self.sessions.lock().await.insert(
            id.clone(),
            Control {
                cancel: cancel.clone(),
                done: done.clone(),
                helper_plugin: helper.definition.plugin_id.clone(),
            },
        );
        self.publish(
            &id,
            plugin,
            action,
            State::Starting,
            "".into(),
            "".into(),
            "".into(),
            "".into(),
        );
        let manager = self.clone();
        let (plugin, action, session_id) = (plugin.to_string(), action.to_string(), id.clone());
        tokio::spawn(async move {
            let _permit = permit;
            manager
                .run(&session_id, &plugin, &action, helper, &cancel)
                .await;
            manager.sessions.lock().await.remove(&session_id);
            done.cancel();
        });
        Ok(id)
    }

    pub(crate) async fn cancel(&self, id: &str) -> bool {
        if let Some(control) = self.sessions.lock().await.get(id) {
            control.cancel.cancel();
            true
        } else {
            false
        }
    }

    pub(crate) async fn cancel_all_and_wait(&self) -> Result<()> {
        let done = {
            let sessions = self.sessions.lock().await;
            sessions
                .values()
                .map(|control| {
                    control.cancel.cancel();
                    control.done.clone()
                })
                .collect::<Vec<_>>()
        };
        Self::wait_for_cleanup(done).await
    }

    pub(crate) async fn suspend_helper_plugin(&self, plugin: &str) -> Result<()> {
        if plugin.is_empty() {
            return Ok(());
        }
        self.helpers.suspend_plugin(plugin).await;
        let done = self
            .sessions
            .lock()
            .await
            .values()
            .filter(|control| control.helper_plugin == plugin)
            .map(|control| {
                control.cancel.cancel();
                control.done.clone()
            })
            .collect();
        Self::wait_for_cleanup(done).await
    }

    pub(crate) fn replace_helpers(&self, helpers: Vec<WebHelperDef>) {
        self.helpers.replace(helpers);
    }

    async fn wait_for_cleanup(done: Vec<CancellationToken>) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(55), async {
            for token in done {
                token.cancelled().await;
            }
        })
        .await
        .context("browser login cleanup timed out")
    }

    async fn run(
        &self,
        id: &str,
        plugin: &str,
        action: &str,
        helper: Arc<WebHelper>,
        cancel: &CancellationToken,
    ) {
        let mut shutdown = self.shutdown.clone();
        let shutdown_wait = async move {
            while !*shutdown.borrow() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
        };
        tokio::pin!(shutdown_wait);
        let started = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            _ = helper.cancelled() => None,
            _ = &mut shutdown_wait => None,
            begin = self.sources.begin_browser_login(plugin, action) => Some(begin),
        };
        let (session, begin) = match started {
            Some(Ok(value)) => value,
            Some(Err(error)) => {
                self.failed(id, plugin, action, error.into(), "".into(), "".into());
                return;
            }
            None => {
                self.publish(
                    id,
                    plugin,
                    action,
                    State::Cancelled,
                    "".into(),
                    "".into(),
                    "".into(),
                    "".into(),
                );
                return;
            }
        };
        let title = begin.title.clone();
        let instruction = begin.instruction.clone();
        if cancel.is_cancelled() || !helper.active() || *self.shutdown.borrow() || !session.active()
        {
            if let Err(error) = session.cancel().await {
                log::error!(
                    "browser login cleanup failed: {}",
                    redact_secrets(&error.to_string())
                );
            }
            self.publish(
                id,
                plugin,
                action,
                State::Cancelled,
                "".into(),
                "".into(),
                title,
                instruction,
            );
            return;
        }
        let result = async {
            let mut child = helper.spawn(false)?;
            let mut input = child.process.stdin.take().context("browser helper stdin unavailable")?;
            let mut output = BufReader::new(child.process.stdout.take().context("browser helper stdout unavailable")?);
            let stderr = child.process.stderr.take().context("browser helper stderr unavailable")?;
            let mut stderr_task = tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).take(1024 * 1024);
                let mut line = Vec::new();
                while let Ok(length) = reader.read_until(b'\n', &mut line).await {
                    if length == 0 { break; }
                    if line.len() > 4096 {
                        log::warn!("browser helper diagnostic exceeded 4 KiB and was suppressed");
                    } else {
                        log::warn!("browser helper: {}", redact_secrets(&String::from_utf8_lossy(&line)));
                    }
                    line.clear();
                }
            });
            let loop_result = async {
                write_command(&mut input, &begin.browser.open_command()?).await?;
                let deadline = tokio::time::sleep(Duration::from_millis(begin.expires_in_ms));
                tokio::pin!(deadline);
                let startup = tokio::time::sleep(Duration::from_secs(20));
                tokio::pin!(startup);
                let mut ready = false;
                let stale = async { loop { tokio::time::sleep(Duration::from_millis(100)).await; if !session.active() { break; } } };
                tokio::pin!(stale);
                loop {
                    let event = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                        _ = helper.cancelled() => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                        _ = &mut shutdown_wait => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                        _ = &mut stale => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                        _ = &mut deadline => return Ok((State::Expired, LocalizedText::default(), "Browser login expired".into())),
                        _ = &mut startup, if !ready => bail!("browser helper did not become ready within 20 seconds"),
                        event = read_event(&mut output) => event?,
                    };
                    match event {
                        BrowserEvent::Ready => {
                            ready = true;
                            self.publish(id, plugin, action, State::Waiting, "".into(), "".into(), title.clone(), instruction.clone());
                        }
                        BrowserEvent::Closed => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                        BrowserEvent::Error { message, .. } => bail!("{}", redact_secrets(&message)),
                        event => {
                            self.publish(id, plugin, action, State::Verifying, "".into(), "".into(), title.clone(), instruction.clone());
                            let update = tokio::select! {
                                biased;
                                _ = cancel.cancelled() => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                                _ = helper.cancelled() => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                                _ = &mut shutdown_wait => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                                _ = &mut stale => return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default())),
                                _ = &mut deadline => return Ok((State::Expired, LocalizedText::default(), "Browser login expired".into())),
                                result = session.handle_event(&event) => result?,
                            };
                            if cancel.is_cancelled() || !helper.active() || *self.shutdown.borrow() || !session.active() {
                                return Ok((State::Cancelled, LocalizedText::default(), LocalizedText::default()));
                            }
                            match update.state {
                                BrowserLoginOutcome::Succeeded => return Ok((State::Succeeded, update.display_value, update.error)),
                                BrowserLoginOutcome::Failed => return Ok((State::Failed, update.display_value, update.error)),
                                BrowserLoginOutcome::Waiting => {
                                    for command in update.commands { write_command(&mut input, &command.wire()?).await?; }
                                    self.publish(id, plugin, action, State::Waiting, update.display_value, "".into(), title.clone(), instruction.clone());
                                }
                            }
                        }
                    }
                }
            }.await;
            let _ = tokio::time::timeout(Duration::from_millis(300), write_command(&mut input, &BrowserCommand::Close.wire()?)).await;
            drop(input);
            child.stop().await;
            if tokio::time::timeout(Duration::from_secs(1), &mut stderr_task).await.is_err() {
                stderr_task.abort();
            }
            loop_result
        }.await;
        if let Err(error) = session.cancel().await {
            log::error!(
                "browser login cleanup failed: {}",
                redact_secrets(&error.to_string())
            );
        }
        match result {
            Ok((state, display_value, error)) => {
                if state == State::Failed {
                    log::error!("browser login failed: {}", redact_secrets(error.text()));
                }
                self.publish(
                    id,
                    plugin,
                    action,
                    state,
                    display_value,
                    error,
                    title,
                    instruction,
                );
                if state == State::Succeeded {
                    let _ = self.events.send(GlobalEvent::PluginStateChanged);
                }
            }
            Err(error) => self.failed(id, plugin, action, error, title, instruction),
        }
    }

    fn failed(
        &self,
        id: &str,
        plugin: &str,
        action: &str,
        error: anyhow::Error,
        title: LocalizedText,
        instruction: LocalizedText,
    ) {
        let error = redact_secrets(&format!("{error:#}"));
        log::error!("browser login failed for {plugin}: {error}");
        self.publish(
            id,
            plugin,
            action,
            State::Failed,
            "".into(),
            error.into(),
            title,
            instruction,
        );
    }

    fn publish(
        &self,
        id: &str,
        plugin: &str,
        action: &str,
        state: State,
        display_value: LocalizedText,
        error: LocalizedText,
        title: LocalizedText,
        instruction: LocalizedText,
    ) {
        let _ = self.events.send(GlobalEvent::BrowserLoginProgress {
            session_id: id.into(),
            plugin_id: plugin.into(),
            action_id: action.into(),
            state: state as i32,
            display_value,
            error,
            title,
            instruction,
        });
    }
}

async fn write_command(
    input: &mut (impl tokio::io::AsyncWrite + Unpin),
    value: &serde_json::Value,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > FRAME_LIMIT {
        bail!("browser command exceeds frame limit");
    }
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_secs(5), async {
        input
            .write_all(&bytes)
            .await
            .context("write browser helper command")?;
        input.flush().await.context("flush browser helper command")
    })
    .await
    .context("browser helper command timed out")?
}

async fn read_event(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<BrowserEvent> {
    let mut bytes = Vec::new();
    let read = reader
        .take(FRAME_LIMIT as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .await
        .context("read browser helper event")?;
    if read == 0 {
        bail!("browser helper exited without completing login");
    }
    browser_protocol::parse_event(&bytes)
}
