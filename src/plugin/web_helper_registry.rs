use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use futures_util::{stream, StreamExt};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use super::i18n::{LocalizedText, LocalizedTextDef};
use super::source::redact_secrets;

pub(crate) const DEFAULT_HELPER_ID: &str = "system-webkit";
const CHECK_LIMIT: u64 = 64 * 1024;
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
pub struct WebHelperDeclaration {
    pub bin: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub protocol_version: u32,
    pub label: LocalizedTextDef,
}

impl WebHelperDeclaration {
    pub(crate) fn validate(&self, component: &str) -> Result<()> {
        ensure!(
            !component.is_empty() && !component.contains(['/', '\\', '\0']),
            "web helper component must be a non-empty name without path separators"
        );
        ensure!(
            self.label.is_valid(),
            "web helper label requires a non-empty msgid"
        );
        ensure!(
            self.protocol_version == 1,
            "web helper protocol_version must be 1"
        );
        ensure!(
            !self.bin.as_os_str().is_empty(),
            "web helper bin must not be empty"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebHelperDef {
    pub id: String,
    pub plugin_id: String,
    pub package_root: PathBuf,
    pub plugin_version: String,
    pub plugin_system: bool,
    pub bin: Option<PathBuf>,
    pub args: Vec<String>,
    pub label: LocalizedText,
    pub error: Option<String>,
}

impl WebHelperDef {
    fn system(bin: Option<PathBuf>) -> Self {
        Self {
            id: DEFAULT_HELPER_ID.into(),
            plugin_id: String::new(),
            package_root: PathBuf::new(),
            plugin_version: String::new(),
            plugin_system: true,
            bin,
            args: Vec::new(),
            label: "System WebKit".into(),
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HelperAvailability {
    pub available: bool,
    pub error: String,
}

impl HelperAvailability {
    fn failure(error: impl std::fmt::Display) -> Self {
        Self {
            available: false,
            error: redact_secrets(&error.to_string()),
        }
    }
}

pub(crate) struct WebHelper {
    pub(crate) definition: WebHelperDef,
    revoked: CancellationToken,
    spawn_lock: Mutex<()>,
    cache: tokio::sync::Mutex<Option<(Instant, HelperAvailability)>>,
}

impl WebHelper {
    fn new(definition: WebHelperDef) -> Self {
        Self {
            definition,
            revoked: CancellationToken::new(),
            spawn_lock: Mutex::new(()),
            cache: tokio::sync::Mutex::new(None),
        }
    }

    fn revoke(&self) {
        let _guard = self.spawn_lock.lock().unwrap();
        self.revoked.cancel();
    }

    pub(crate) fn active(&self) -> bool {
        !self.revoked.is_cancelled()
    }

    pub(crate) async fn cancelled(&self) {
        self.revoked.cancelled().await;
    }

    pub(crate) fn spawn(&self, check: bool) -> Result<HelperProcess> {
        let _guard = self.spawn_lock.lock().unwrap();
        ensure!(
            self.active(),
            "selected browser helper was removed or reloaded"
        );
        if let Some(error) = &self.definition.error {
            bail!("{error}");
        }
        let bin = self.definition.bin.as_ref().context(
            "waywallen-web-helper was not found; install it next to waywallen or set --web-helper PATH",
        )?;
        let mut command = tokio::process::Command::new(bin);
        command
            .args(&self.definition.args)
            .stdin(if check { Stdio::null() } else { Stdio::piped() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if check {
            command.arg("--check");
        }
        let process = command
            .spawn()
            .context("could not start selected browser helper")?;
        Ok(HelperProcess {
            group: process.id(),
            process,
        })
    }

    pub(crate) async fn availability(&self, force: bool) -> HelperAvailability {
        let mut cache = self.cache.lock().await;
        if !self.active() {
            return HelperAvailability::failure("selected browser helper was removed or reloaded");
        }
        if !force {
            if let Some((checked, result)) = cache.as_ref() {
                if checked.elapsed() < CACHE_TTL {
                    return result.clone();
                }
            }
        }
        let result = tokio::select! {
            biased;
            _ = self.cancelled() => Err(anyhow::anyhow!("selected browser helper was removed or reloaded")),
            result = tokio::time::timeout(CHECK_TIMEOUT, self.check()) => {
                result.context("browser helper availability check timed out").and_then(|result| result)
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => HelperAvailability::failure(format!("{error:#}")),
        };
        *cache = Some((Instant::now(), result.clone()));
        result
    }

    async fn check(&self) -> Result<HelperAvailability> {
        let mut child = self.spawn(true)?;
        let stdout = child
            .process
            .stdout
            .take()
            .context("browser helper stdout unavailable")?;
        let stderr = child
            .process
            .stderr
            .take()
            .context("browser helper stderr unavailable")?;
        let (output, diagnostic, status) =
            tokio::try_join!(limited_output(stdout), limited_output(stderr), async {
                child
                    .process
                    .wait()
                    .await
                    .context("wait for browser helper check")
            })?;
        #[derive(Deserialize)]
        struct Check {
            version: u32,
            available: bool,
            #[serde(default)]
            error: String,
        }
        let result: Check = match serde_json::from_slice(&output) {
            Ok(result) => result,
            Err(_) if !status.success() => {
                let mut diagnostic = redact_secrets(&String::from_utf8_lossy(&diagnostic));
                let mut end = diagnostic.len().min(4096);
                while !diagnostic.is_char_boundary(end) {
                    end -= 1;
                }
                diagnostic.truncate(end);
                let diagnostic = diagnostic.trim();
                bail!(
                    "browser helper availability check exited with {status}{}",
                    if diagnostic.is_empty() {
                        String::new()
                    } else {
                        format!(": {diagnostic}")
                    }
                );
            }
            Err(_) => bail!("invalid browser helper availability response"),
        };
        ensure!(
            result.version == 1,
            "browser helper check returned unsupported protocol version"
        );
        Ok(if result.available {
            ensure!(
                status.success(),
                "browser helper availability check exited with {status}"
            );
            HelperAvailability {
                available: true,
                error: String::new(),
            }
        } else {
            HelperAvailability::failure(if result.error.is_empty() {
                "browser helper is unavailable"
            } else {
                &result.error
            })
        })
    }
}

async fn limited_output(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    reader
        .take(CHECK_LIMIT + 1)
        .read_to_end(&mut output)
        .await?;
    ensure!(
        output.len() as u64 <= CHECK_LIMIT,
        "browser helper check output exceeds 64 KiB"
    );
    Ok(output)
}

pub(crate) struct WebHelperRegistry {
    helpers: RwLock<BTreeMap<String, Arc<WebHelper>>>,
}

impl WebHelperRegistry {
    pub(crate) fn new(system_bin: Option<PathBuf>, definitions: Vec<WebHelperDef>) -> Self {
        let system = WebHelperDef::system(system_bin);
        let registry = Self {
            helpers: RwLock::new(BTreeMap::from([(
                system.id.clone(),
                Arc::new(WebHelper::new(system)),
            )])),
        };
        registry.replace(definitions);
        registry
    }

    pub(crate) fn replace(&self, definitions: Vec<WebHelperDef>) {
        let mut helpers = self.helpers.write().unwrap();
        let mut replacement = BTreeMap::new();
        if let Some(system) = helpers.remove(DEFAULT_HELPER_ID) {
            replacement.insert(DEFAULT_HELPER_ID.into(), system);
        }
        for definition in definitions {
            if replacement.contains_key(&definition.id) {
                continue;
            }
            let helper = match helpers.remove(&definition.id) {
                Some(helper) if helper.active() && helper.definition == definition => helper,
                previous => {
                    if let Some(previous) = previous {
                        previous.revoke();
                    }
                    Arc::new(WebHelper::new(definition))
                }
            };
            replacement.insert(helper.definition.id.clone(), helper);
        }
        for helper in helpers.values() {
            helper.revoke();
        }
        *helpers = replacement;
    }

    pub(crate) async fn suspend_plugin(&self, plugin_id: &str) {
        if plugin_id.is_empty() {
            return;
        }
        let helpers = self
            .helpers
            .read()
            .unwrap()
            .values()
            .filter(|helper| helper.definition.plugin_id == plugin_id)
            .cloned()
            .collect::<Vec<_>>();
        for helper in &helpers {
            helper.revoke();
        }
        for helper in helpers {
            let _completed = helper.cache.lock().await;
        }
    }

    pub(crate) fn select(&self, id: &str) -> Result<Arc<WebHelper>> {
        let id = if id.is_empty() { DEFAULT_HELPER_ID } else { id };
        let helper = self
            .helpers
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .with_context(|| format!("unknown browser helper '{id}'"))?;
        ensure!(
            helper.active(),
            "selected browser helper was removed or reloaded"
        );
        Ok(helper)
    }

    pub(crate) async fn availability(
        &self,
        force: bool,
    ) -> Vec<(WebHelperDef, HelperAvailability)> {
        let helpers = self
            .helpers
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut results = stream::iter(helpers)
            .map(|helper| async move {
                let availability = helper.availability(force).await;
                (helper.definition.clone(), availability)
            })
            .buffer_unordered(4)
            .collect::<Vec<_>>()
            .await;
        results.sort_by(|(a, _), (b, _)| {
            (a.id != DEFAULT_HELPER_ID, &a.id).cmp(&(b.id != DEFAULT_HELPER_ID, &b.id))
        });
        results
    }
}

pub(crate) struct HelperProcess {
    pub(crate) process: tokio::process::Child,
    group: Option<u32>,
}

impl HelperProcess {
    pub(crate) async fn stop(&mut self) {
        let _ = tokio::time::timeout(Duration::from_millis(500), self.process.wait()).await;
        self.kill();
        let _ = self.process.wait().await;
    }

    fn kill(&mut self) {
        if let Some(id) = self.group.take() {
            unsafe {
                libc::kill(-(id as i32), libc::SIGKILL);
            }
        }
        let _ = self.process.start_kill();
    }
}

impl Drop for HelperProcess {
    fn drop(&mut self) {
        self.kill();
    }
}
