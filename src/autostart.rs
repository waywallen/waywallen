use std::path::{Path, PathBuf};

use ashpd::desktop::background::Background;

use crate::error::{Error, Result};
use crate::settings::SettingsStore;

const FLATPAK_ID_ENV: &str = "FLATPAK_ID";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortalState {
    autostart: bool,
}

trait PortalClient {
    async fn set_enabled(&self, enabled: bool) -> Result<PortalState>;
}

struct XdpPortal;

impl PortalClient for XdpPortal {
    async fn set_enabled(&self, enabled: bool) -> Result<PortalState> {
        let command = autostart_command();
        let request = Background::request()
            .auto_start(enabled)
            .command(&command)
            .dbus_activatable(false)
            .send()
            .await
            .map_err(|e| Error::PortalCallFailed(format!("RequestBackground: {e}")))?;
        let response = request
            .response()
            .map_err(|e| Error::PortalCallFailed(format!("RequestBackground response: {e}")))?;

        Ok(PortalState {
            autostart: response.auto_start(),
        })
    }
}

fn is_sandboxed() -> bool {
    std::env::var_os(FLATPAK_ID_ENV).is_some_and(|id| !id.is_empty())
}

/// Login-start command line. Inside Flatpak the portal rewrites it to
/// `flatpak run <app-id> ...`, so the bare binary name suffices. On the
/// host the daemon is often not on PATH, so pin the running executable.
fn autostart_command() -> Vec<String> {
    let bin = if is_sandboxed() {
        "waywallen".to_string()
    } else {
        std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "waywallen".to_string())
    };
    vec![bin, "--no-ui".to_string()]
}

/// XDG autostart entry location. Order mirrors
/// `settings::default_config_path`:
///   1. `$XDG_CONFIG_HOME/autostart/waywallen.desktop`
///   2. `$HOME/.config/autostart/waywallen.desktop`
fn xdg_autostart_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("autostart/waywallen.desktop");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config/autostart/waywallen.desktop");
    }
    PathBuf::from("waywallen-autostart.desktop")
}

/// Quote one Exec argument per the Desktop Entry spec: double-quote and
/// backslash-escape the reserved characters that stay live inside quotes.
fn quote_exec_arg(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn write_autostart_entry(path: &Path, command: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let exec = command
        .iter()
        .map(|a| quote_exec_arg(a))
        .collect::<Vec<_>>()
        .join(" ");
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Waywallen\n\
         Comment=Wallpaper manager daemon\n\
         Exec={exec}\n\
         Icon=org.waywallen.waywallen\n\
         Terminal=false\n"
    );
    std::fs::write(path, entry)
}

fn remove_autostart_entry(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[derive(Default)]
pub struct AutostartService {
    mutation: tokio::sync::Mutex<()>,
}

impl AutostartService {
    /// Persisted intent, not live desktop state — an entry removed
    /// behind our back (portal side or a hand-deleted .desktop file)
    /// is not detected.
    pub fn enabled(&self, settings: &SettingsStore) -> Result<bool> {
        Ok(settings.global().autostart_enabled)
    }

    /// Sandboxed: the Background portal is the only mechanism, and it
    /// resolves the Flatpak app id reliably. On the host the portal
    /// derives the caller's app id from the launch context (cgroup),
    /// which can attribute the entry to the wrong application — so
    /// write the XDG autostart entry directly instead.
    pub async fn set_enabled(&self, settings: &SettingsStore, enabled: bool) -> Result<bool> {
        if is_sandboxed() {
            self.set_enabled_with(settings, enabled, &XdpPortal).await
        } else {
            self.set_enabled_host(settings, enabled, &xdg_autostart_path())
                .await
        }
    }

    async fn set_enabled_with<C: PortalClient>(
        &self,
        settings: &SettingsStore,
        enabled: bool,
        portal: &C,
    ) -> Result<bool> {
        let _guard = self.mutation.lock().await;
        let response = portal.set_enabled(enabled).await?;
        if response.autostart != enabled {
            return Err(Error::PortalCallFailed(format!(
                "RequestBackground returned autostart={} for requested autostart={enabled}",
                response.autostart
            )));
        }

        settings.update(|s| s.global.autostart_enabled = enabled);
        settings.flush_now().await;
        Ok(enabled)
    }

    async fn set_enabled_host(
        &self,
        settings: &SettingsStore,
        enabled: bool,
        entry: &Path,
    ) -> Result<bool> {
        let _guard = self.mutation.lock().await;
        if enabled {
            write_autostart_entry(entry, &autostart_command())?;
        } else {
            remove_autostart_entry(entry)?;
        }

        settings.update(|s| s.global.autostart_enabled = enabled);
        settings.flush_now().await;
        Ok(enabled)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::*;

    struct MockPortal {
        result: Mutex<Option<Result<PortalState>>>,
        requested: Mutex<Vec<bool>>,
    }

    impl MockPortal {
        fn returning(result: Result<PortalState>) -> Self {
            Self {
                result: Mutex::new(Some(result)),
                requested: Mutex::new(Vec::new()),
            }
        }
    }

    impl PortalClient for MockPortal {
        async fn set_enabled(&self, enabled: bool) -> Result<PortalState> {
            self.requested.lock().unwrap().push(enabled);
            self.result.lock().unwrap().take().unwrap()
        }
    }

    async fn settings() -> (tempfile::TempDir, std::sync::Arc<SettingsStore>) {
        let dir = tempdir().unwrap();
        let store = SettingsStore::load_or_default(dir.path().join("config.toml")).await;
        (dir, store)
    }

    #[tokio::test]
    async fn successful_response_updates_persisted_state() {
        let (_dir, settings) = settings().await;
        let portal = MockPortal::returning(Ok(PortalState { autostart: true }));

        let enabled = AutostartService::default()
            .set_enabled_with(&settings, true, &portal)
            .await
            .unwrap();

        assert!(enabled);
        assert_eq!(*portal.requested.lock().unwrap(), [true]);
        assert!(settings.global().autostart_enabled);
        let persisted = tokio::fs::read_to_string(settings.path()).await.unwrap();
        assert!(persisted.contains("autostart_enabled = true"));
    }

    #[tokio::test]
    async fn successful_disable_updates_persisted_state() {
        let (_dir, settings) = settings().await;
        settings.update(|s| s.global.autostart_enabled = true);
        settings.flush_now().await;
        let portal = MockPortal::returning(Ok(PortalState { autostart: false }));

        let enabled = AutostartService::default()
            .set_enabled_with(&settings, false, &portal)
            .await
            .unwrap();

        assert!(!enabled);
        assert_eq!(*portal.requested.lock().unwrap(), [false]);
        assert!(!settings.global().autostart_enabled);
        let persisted = tokio::fs::read_to_string(settings.path()).await.unwrap();
        assert!(persisted.contains("autostart_enabled = false"));
    }

    #[tokio::test]
    async fn mismatched_response_does_not_update_state() {
        let (_dir, settings) = settings().await;
        let portal = MockPortal::returning(Ok(PortalState { autostart: false }));

        let result = AutostartService::default()
            .set_enabled_with(&settings, true, &portal)
            .await;

        assert!(matches!(result, Err(Error::PortalCallFailed(_))));
        assert!(!settings.global().autostart_enabled);
    }

    #[tokio::test]
    async fn portal_error_does_not_update_state() {
        let (_dir, settings) = settings().await;
        let portal = MockPortal::returning(Err(Error::PortalCallFailed("cancelled".to_string())));

        let result = AutostartService::default()
            .set_enabled_with(&settings, true, &portal)
            .await;

        assert!(matches!(result, Err(Error::PortalCallFailed(_))));
        assert!(!settings.global().autostart_enabled);
    }

    #[tokio::test]
    async fn host_enable_writes_entry_and_persists() {
        let (_dir, settings) = settings().await;
        let dir = tempdir().unwrap();
        let entry = dir.path().join("autostart/waywallen.desktop");

        let enabled = AutostartService::default()
            .set_enabled_host(&settings, true, &entry)
            .await
            .unwrap();

        assert!(enabled);
        assert!(settings.global().autostart_enabled);
        let content = std::fs::read_to_string(&entry).unwrap();
        assert!(content.starts_with("[Desktop Entry]"));
        assert!(content.contains("--no-ui"));
    }

    #[tokio::test]
    async fn host_disable_removes_entry_and_persists() {
        let (_dir, settings) = settings().await;
        settings.update(|s| s.global.autostart_enabled = true);
        settings.flush_now().await;
        let dir = tempdir().unwrap();
        let entry = dir.path().join("waywallen.desktop");
        write_autostart_entry(&entry, &["waywallen".into(), "--no-ui".into()]).unwrap();

        let enabled = AutostartService::default()
            .set_enabled_host(&settings, false, &entry)
            .await
            .unwrap();

        assert!(!enabled);
        assert!(!settings.global().autostart_enabled);
        assert!(!entry.exists());
    }

    #[tokio::test]
    async fn host_write_failure_does_not_update_state() {
        let (_dir, settings) = settings().await;
        let dir = tempdir().unwrap();
        // A directory at the entry path makes the write fail.
        let entry = dir.path().join("waywallen.desktop");
        std::fs::create_dir(&entry).unwrap();

        let result = AutostartService::default()
            .set_enabled_host(&settings, true, &entry)
            .await;

        assert!(matches!(result, Err(Error::Io(_))));
        assert!(!settings.global().autostart_enabled);
    }

    #[test]
    fn remove_missing_entry_is_ok() {
        let dir = tempdir().unwrap();
        assert!(remove_autostart_entry(&dir.path().join("absent.desktop")).is_ok());
    }

    #[test]
    fn exec_args_are_quoted() {
        assert_eq!(
            quote_exec_arg("/opt/way wallen/bin"),
            "\"/opt/way wallen/bin\""
        );
        assert_eq!(quote_exec_arg("a\"b$c"), "\"a\\\"b\\$c\"");
    }
}
