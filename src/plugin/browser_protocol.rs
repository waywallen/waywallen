use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const FRAME_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CookieRequest {
    pub uri: String,
    #[serde(default)]
    pub names: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Script {
    pub source: String,
    pub origins: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowserRequest {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub redirects: Vec<String>,
    #[serde(default)]
    pub cookie_requests: Vec<CookieRequest>,
    #[serde(default)]
    pub scripts: Vec<Script>,
}

fn http_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value).context("invalid browser HTTP URL")?;
    ensure!(
        matches!(url.scheme(), "https" | "http")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none(),
        "browser URL must be HTTP(S) without credentials"
    );
    ensure!(value.len() <= 16 * 1024, "browser URL exceeds limit");
    Ok(())
}

fn http_origin(value: &str) -> Result<()> {
    http_url(value)?;
    let url = reqwest::Url::parse(value)?;
    ensure!(
        matches!(url.path(), "" | "/") && url.query().is_none() && url.fragment().is_none(),
        "browser script target must be an HTTP(S) origin"
    );
    Ok(())
}

impl BrowserRequest {
    pub fn validate(&self) -> Result<()> {
        http_url(&self.url)?;
        ensure!(
            self.title.len() <= 4096
                && self.redirects.len() <= 32
                && self.cookie_requests.len() <= 32
                && self.scripts.len() <= 16,
            "browser request exceeds limits"
        );
        for value in &self.redirects {
            let url = reqwest::Url::parse(value).context("invalid browser redirect URI")?;
            ensure!(
                value.len() <= 16 * 1024
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && !matches!(url.scheme(), "file" | "javascript" | "data"),
                "invalid browser redirect URI"
            );
        }
        for request in &self.cookie_requests {
            request.validate()?;
        }
        for script in &self.scripts {
            ensure!(
                script.source.len() <= 256 * 1024
                    && !script.origins.is_empty()
                    && script.origins.len() <= 32,
                "browser script exceeds limits"
            );
            for origin in &script.origins {
                http_origin(origin)?;
            }
        }
        ensure!(
            serde_json::to_vec(self)?.len() < FRAME_LIMIT - 128,
            "browser request exceeds frame limit"
        );
        Ok(())
    }

    pub fn open_command(&self) -> Result<Value> {
        let mut value = serde_json::to_value(self)?;
        value["version"] = 1.into();
        value["command"] = "open".into();
        Ok(value)
    }
}

impl CookieRequest {
    fn validate(&self) -> Result<()> {
        http_url(&self.uri)?;
        ensure!(
            self.names.len() <= 64
                && self.names.iter().all(|name| !name.is_empty()
                    && name.len() <= 256
                    && !name.chars().any(char::is_control)),
            "invalid browser cookie selector"
        );
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum BrowserCommand {
    Close,
    GetCookies {
        id: u64,
        uri: String,
        #[serde(default)]
        names: Vec<String>,
    },
    Evaluate {
        id: u64,
        origin: String,
        source: String,
    },
    Navigate {
        url: String,
    },
}

impl BrowserCommand {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Close => Ok(()),
            Self::GetCookies { uri, names, .. } => CookieRequest {
                uri: uri.clone(),
                names: names.clone(),
            }
            .validate(),
            Self::Evaluate { origin, source, .. } => {
                http_origin(origin)?;
                ensure!(source.len() <= 256 * 1024, "browser script exceeds limit");
                Ok(())
            }
            Self::Navigate { url } => http_url(url),
        }
    }
    pub fn wire(&self) -> Result<Value> {
        self.validate()?;
        let mut value = serde_json::to_value(self)?;
        value["version"] = 1.into();
        Ok(value)
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BrowserCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    pub expires: Option<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum BrowserEvent {
    Ready,
    Redirect {
        uri: String,
    },
    Cookies {
        #[serde(default)]
        id: u64,
        uri: String,
        cookies: Vec<BrowserCookie>,
    },
    Javascript {
        id: u64,
        value: Value,
    },
    ScriptMessage {
        origin: String,
        value: Value,
    },
    Closed,
    Error {
        code: String,
        message: String,
    },
}

pub(crate) fn parse_event(bytes: &[u8]) -> Result<BrowserEvent> {
    ensure!(
        bytes.len() <= FRAME_LIMIT,
        "browser helper frame exceeds 1 MiB"
    );
    let mut value: Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid browser helper JSON"))?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        bail!("unsupported browser helper protocol version");
    }
    value
        .as_object_mut()
        .context("browser helper event must be an object")?
        .remove("version");
    serde_json::from_value(value).map_err(|_| anyhow::anyhow!("invalid browser helper event"))
}
