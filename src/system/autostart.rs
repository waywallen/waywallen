mod desktop;

use ashpd::desktop::background::Background;

use crate::error::{Error, Result};
use crate::settings::SettingsStore;

const FLATPAK_ID_ENV: &str = "FLATPAK_ID";
const AUTOSTART_COMMAND: [&str; 2] = ["waywallen", "--no-ui"];

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
        let request = Background::request()
            .auto_start(enabled)
            .command(&AUTOSTART_COMMAND)
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

#[derive(Default)]
pub struct AutostartService {
    mutation: tokio::sync::Mutex<()>,
}

impl AutostartService {
    pub fn status(&self, settings: &SettingsStore) -> Status {
        if is_flatpak() {
            return Status {
                enabled: settings.global().autostart_enabled,
                supported: true,
                unavailable_reason: String::new(),
            };
        }
        let mut enabled = false;
        match desktop::DesktopAutostart::new().and_then(|entry| {
            enabled = entry.enabled()?;
            entry.ensure_writable()?;
            desktop::launcher()?;
            Ok(())
        }) {
            Ok(()) => Status {
                enabled,
                supported: true,
                unavailable_reason: String::new(),
            },
            Err(error) => Status {
                enabled,
                supported: false,
                unavailable_reason: error.to_string(),
            },
        }
    }

    pub fn refresh(&self) -> Result<()> {
        if is_flatpak() {
            return Ok(());
        }
        match desktop::DesktopAutostart::new() {
            Ok(entry) => entry.refresh(),
            Err(Error::FailedPrecondition(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub async fn set_enabled(&self, settings: &SettingsStore, enabled: bool) -> Result<bool> {
        if is_flatpak() {
            return self.set_enabled_with(settings, enabled, &XdpPortal).await;
        }
        let _guard = self.mutation.lock().await;
        desktop::DesktopAutostart::new()?.set_enabled(enabled)?;
        settings.update(|s| s.global.autostart_enabled = enabled);
        settings.flush_now().await;
        Ok(enabled)
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
}

pub struct Status {
    pub enabled: bool,
    pub supported: bool,
    pub unavailable_reason: String,
}

fn is_flatpak() -> bool {
    std::env::var_os(FLATPAK_ID_ENV).is_some_and(|id| !id.is_empty())
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
}
