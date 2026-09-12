use serde::Deserialize;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::plugin::renderer_registry::PluginPackageMeta;
use crate::plugin::source;
use crate::tasks;
use crate::wallframe::renderer_manager;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PluginUpdatePackage {
    pub zip_url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PluginUpdateManifest {
    pub version: String,
    pub entry_version: u32,
    pub spawn_version: u32,
    #[serde(default)]
    pub x86_64: Option<PluginUpdatePackage>,
    #[serde(default)]
    pub aarch64: Option<PluginUpdatePackage>,
}

impl PluginUpdateManifest {
    pub fn from_json_str(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn package_for_current_arch(&self) -> Option<PluginUpdatePackage> {
        self.package_for_arch(std::env::consts::ARCH)
    }

    pub fn package_for_arch(&self, arch: &str) -> Option<PluginUpdatePackage> {
        match arch {
            "x86_64" => self.x86_64.clone(),
            "aarch64" => self.aarch64.clone(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginUpdateState {
    Unknown,
    NoUrl,
    Checking,
    UpToDate,
    Available,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginUpdateInfo {
    pub plugin_id: String,
    pub state: PluginUpdateState,
    pub latest_version: String,
    pub zip_url: String,
    pub sha256: String,
    pub error: String,
    pub checked_at_ms: i64,
}

pub type PluginUpdateStore = Arc<RwLock<HashMap<String, PluginUpdateInfo>>>;

pub fn new_store() -> PluginUpdateStore {
    Arc::new(RwLock::new(HashMap::new()))
}

pub fn default_info(pkg: &PluginPackageMeta) -> PluginUpdateInfo {
    PluginUpdateInfo {
        plugin_id: pkg.id.clone(),
        state: if pkg.update.as_deref().is_some_and(|u| !u.is_empty()) {
            PluginUpdateState::Unknown
        } else {
            PluginUpdateState::NoUrl
        },
        latest_version: String::new(),
        zip_url: String::new(),
        sha256: String::new(),
        error: String::new(),
        checked_at_ms: 0,
    }
}

pub fn became_available(previous: Option<&PluginUpdateInfo>, current: &PluginUpdateInfo) -> bool {
    if current.state != PluginUpdateState::Available {
        return false;
    }
    !matches!(
        previous,
        Some(prev)
            if prev.state == PluginUpdateState::Available
                && prev.latest_version == current.latest_version
                && prev.zip_url == current.zip_url
                && prev.sha256 == current.sha256
    )
}

pub async fn snapshot_for_package(
    store: &PluginUpdateStore,
    pkg: &PluginPackageMeta,
) -> PluginUpdateInfo {
    store
        .read()
        .await
        .get(&pkg.id)
        .cloned()
        .unwrap_or_else(|| default_info(pkg))
}

pub async fn check_packages(
    store: &PluginUpdateStore,
    packages: Vec<PluginPackageMeta>,
    retain_missing: bool,
) -> Vec<PluginUpdateInfo> {
    check_packages_with_progress(store, packages, retain_missing, |_| {}).await
}

pub async fn check_packages_with_progress<F>(
    store: &PluginUpdateStore,
    packages: Vec<PluginPackageMeta>,
    retain_missing: bool,
    mut on_progress: F,
) -> Vec<PluginUpdateInfo>
where
    F: FnMut(f32) + Send,
{
    {
        let mut w = store.write().await;
        if retain_missing {
            let ids: HashSet<_> = packages.iter().map(|p| p.id.clone()).collect();
            w.retain(|id, _| ids.contains(id));
        }
        for pkg in &packages {
            let info = if pkg.update.as_deref().is_some_and(|u| !u.is_empty()) {
                checking_info(pkg)
            } else {
                no_url_info(pkg)
            };
            w.insert(pkg.id.clone(), info);
        }
    }
    on_progress(0.0);

    if packages.is_empty() {
        on_progress(1.0);
        return Vec::new();
    }

    let total = packages.len() as f32;
    let client = reqwest::Client::new();
    let mut out = Vec::with_capacity(packages.len());
    for (idx, pkg) in packages.into_iter().enumerate() {
        let info = if pkg.update.as_deref().is_some_and(|u| !u.is_empty()) {
            check_one(&client, &pkg).await
        } else {
            no_url_info(&pkg)
        };
        store.write().await.insert(pkg.id.clone(), info.clone());
        out.push(info);
        on_progress((idx + 1) as f32 / total);
    }
    out
}

fn checking_info(pkg: &PluginPackageMeta) -> PluginUpdateInfo {
    PluginUpdateInfo {
        plugin_id: pkg.id.clone(),
        state: PluginUpdateState::Checking,
        latest_version: String::new(),
        zip_url: String::new(),
        sha256: String::new(),
        error: String::new(),
        checked_at_ms: 0,
    }
}

fn no_url_info(pkg: &PluginPackageMeta) -> PluginUpdateInfo {
    PluginUpdateInfo {
        plugin_id: pkg.id.clone(),
        state: PluginUpdateState::NoUrl,
        latest_version: String::new(),
        zip_url: String::new(),
        sha256: String::new(),
        error: String::new(),
        checked_at_ms: tasks::now_ms(),
    }
}

async fn check_one(client: &reqwest::Client, pkg: &PluginPackageMeta) -> PluginUpdateInfo {
    let checked_at_ms = tasks::now_ms();
    let Some(url) = pkg.update.as_deref().filter(|u| !u.is_empty()) else {
        return no_url_info(pkg);
    };

    match fetch_manifest(client, url).await {
        Ok(manifest) => info_from_manifest(pkg, manifest, checked_at_ms),
        Err(e) => PluginUpdateInfo {
            plugin_id: pkg.id.clone(),
            state: PluginUpdateState::Failed,
            latest_version: String::new(),
            zip_url: String::new(),
            sha256: String::new(),
            error: e,
            checked_at_ms,
        },
    }
}

async fn fetch_manifest(
    client: &reqwest::Client,
    url: &str,
) -> Result<PluginUpdateManifest, String> {
    let url = normalize_update_manifest_url(url);
    let response = tokio::time::timeout(REQUEST_TIMEOUT, client.get(url.as_ref()).send())
        .await
        .map_err(|_| {
            format!(
                "update request timed out after {}s",
                REQUEST_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("update request failed: {e}"))?;
    let response = response
        .error_for_status()
        .map_err(|e| format!("update response failed: {e}"))?;
    let text = response
        .text()
        .await
        .map_err(|e| format!("read update manifest failed: {e}"))?;
    PluginUpdateManifest::from_json_str(&text).map_err(|e| format!("parse update manifest: {e}"))
}

fn normalize_update_manifest_url(url: &str) -> Cow<'_, str> {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return Cow::Borrowed(url);
    };
    if parsed.host_str() != Some("github.com") {
        return Cow::Borrowed(url);
    }

    let Some(segments) = parsed.path_segments() else {
        return Cow::Borrowed(url);
    };
    let segments = segments.collect::<Vec<_>>();
    if segments.len() < 5
        || segments[0].is_empty()
        || segments[1].is_empty()
        || segments[2] != "blob"
        || segments[3].is_empty()
    {
        return Cow::Borrowed(url);
    }

    let mut raw_segments = Vec::with_capacity(segments.len() + 2);
    raw_segments.extend_from_slice(&segments[..2]);
    raw_segments.extend_from_slice(&["raw", "refs", "heads", segments[3]]);
    raw_segments.extend_from_slice(&segments[4..]);
    parsed.set_path(&format!("/{}", raw_segments.join("/")));
    Cow::Owned(parsed.into())
}

fn info_from_manifest(
    pkg: &PluginPackageMeta,
    manifest: PluginUpdateManifest,
    checked_at_ms: i64,
) -> PluginUpdateInfo {
    if !version_cmp(&manifest.version, &pkg.version).is_gt() {
        return PluginUpdateInfo {
            plugin_id: pkg.id.clone(),
            state: PluginUpdateState::UpToDate,
            latest_version: manifest.version,
            zip_url: String::new(),
            sha256: String::new(),
            error: String::new(),
            checked_at_ms,
        };
    }

    let Some(package) = manifest.package_for_current_arch() else {
        return unsupported_info(
            pkg,
            manifest.version,
            format!("no package for {}", std::env::consts::ARCH),
            checked_at_ms,
        );
    };
    if !source::supports_entry_version(manifest.entry_version) {
        return unsupported_info(
            pkg,
            manifest.version,
            format!(
                "entry_version {} is unsupported; supported versions are {:?}",
                manifest.entry_version,
                source::SUPPORTED_ENTRY_VERSIONS
            ),
            checked_at_ms,
        );
    }
    if manifest.spawn_version != renderer_manager::SPAWN_VERSION {
        return unsupported_info(
            pkg,
            manifest.version,
            format!(
                "spawn_version {} is unsupported; expected {}",
                manifest.spawn_version,
                renderer_manager::SPAWN_VERSION
            ),
            checked_at_ms,
        );
    }

    PluginUpdateInfo {
        plugin_id: pkg.id.clone(),
        state: PluginUpdateState::Available,
        latest_version: manifest.version,
        zip_url: package.zip_url,
        sha256: package.sha256,
        error: String::new(),
        checked_at_ms,
    }
}

fn unsupported_info(
    pkg: &PluginPackageMeta,
    latest_version: String,
    error: String,
    checked_at_ms: i64,
) -> PluginUpdateInfo {
    PluginUpdateInfo {
        plugin_id: pkg.id.clone(),
        state: PluginUpdateState::Unsupported,
        latest_version,
        zip_url: String::new(),
        sha256: String::new(),
        error,
        checked_at_ms,
    }
}

fn version_cmp(a: &str, b: &str) -> Ordering {
    match (parse_version(a), parse_version(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

fn parse_version(v: &str) -> Option<semver::Version> {
    semver::Version::parse(v.trim().trim_start_matches(['v', 'V'])).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_arch_packages_and_versions() {
        let src = r#"
            {
                "version": "0.2.0",
                "entry_version": 4,
                "spawn_version": 6,
                "x86_64": {
                    "zip_url": "https://example.org/owe/x86_64.zip",
                    "sha256": "x86_64-sha256"
                },
                "aarch64": {
                    "zip_url": "https://example.org/owe/aarch64.zip",
                    "sha256": "aarch64-sha256"
                }
            }
        "#;
        let m = PluginUpdateManifest::from_json_str(src).expect("parses");
        assert_eq!(m.entry_version, 4);
        assert_eq!(m.spawn_version, 6);
        let x86_64 = m.package_for_arch("x86_64").expect("x86_64 package");
        assert_eq!(x86_64.zip_url, "https://example.org/owe/x86_64.zip");
        assert_eq!(x86_64.sha256, "x86_64-sha256");
        let aarch64 = m.package_for_arch("aarch64").expect("aarch64 package");
        assert_eq!(aarch64.zip_url, "https://example.org/owe/aarch64.zip");
        assert_eq!(aarch64.sha256, "aarch64-sha256");
    }

    #[test]
    fn rejects_version_lists() {
        let src = r#"
            {
                "version": "0.2.0",
                "entry_version": [4],
                "spawn_version": [6],
                "x86_64": {
                    "zip_url": "https://example.org/owe/x86_64.zip",
                    "sha256": "x86_64-sha256"
                }
            }
        "#;
        assert!(PluginUpdateManifest::from_json_str(src).is_err());
    }

    #[test]
    fn compares_semver_with_v_prefix() {
        assert!(version_cmp("v1.2.0", "1.1.9").is_gt());
        assert!(version_cmp("1.0.0", "1.0.0").is_eq());
    }

    #[test]
    fn normalizes_github_blob_update_urls() {
        assert_eq!(
            normalize_update_manifest_url(
                "https://github.com/example/plugin/blob/main/update.json"
            ),
            "https://github.com/example/plugin/raw/refs/heads/main/update.json"
        );
        assert_eq!(
            normalize_update_manifest_url(
                "https://github.com/example/plugin/blob/release/meta/update.json"
            ),
            "https://github.com/example/plugin/raw/refs/heads/release/meta/update.json"
        );
    }

    #[test]
    fn leaves_non_github_blob_update_urls_unchanged() {
        let url = "https://example.invalid/plugin/blob/main/update.json";
        assert_eq!(normalize_update_manifest_url(url), url);
        let url = "https://github.com/example/plugin/raw/refs/heads/main/update.json";
        assert_eq!(normalize_update_manifest_url(url), url);
    }

    #[test]
    fn marks_missing_arch_package_unsupported() {
        let pkg = PluginPackageMeta {
            id: "org.test".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            update: Some("https://example.invalid/update.json".into()),
            has_entry: false,
            system: true,
            incompat: None,
        };
        let manifest = PluginUpdateManifest {
            version: "2.0.0".into(),
            entry_version: source::ENTRY_VERSION,
            spawn_version: renderer_manager::SPAWN_VERSION,
            x86_64: None,
            aarch64: None,
        };
        let info = info_from_manifest(&pkg, manifest, 7);
        assert_eq!(info.state, PluginUpdateState::Unsupported);
        assert_eq!(info.checked_at_ms, 7);
    }

    #[test]
    fn accepts_update_manifests_for_entry_v3_and_v4() {
        let pkg = PluginPackageMeta {
            id: "org.test".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            update: Some("https://example.invalid/update.json".into()),
            has_entry: true,
            system: true,
            incompat: None,
        };
        let package = PluginUpdatePackage {
            zip_url: "https://example.invalid/plugin.zip".into(),
            sha256: "00".repeat(32),
        };
        for entry_version in [source::ENTRY_VERSION_V3, source::ENTRY_VERSION_V4] {
            let manifest = PluginUpdateManifest {
                version: "2.0.0".into(),
                entry_version,
                spawn_version: renderer_manager::SPAWN_VERSION,
                x86_64: Some(package.clone()),
                aarch64: Some(package.clone()),
            };
            assert_eq!(
                info_from_manifest(&pkg, manifest, 7).state,
                PluginUpdateState::Available
            );
        }

        let manifest = PluginUpdateManifest {
            version: "2.0.0".into(),
            entry_version: source::ENTRY_VERSION_V3 - 1,
            spawn_version: renderer_manager::SPAWN_VERSION,
            x86_64: Some(package.clone()),
            aarch64: Some(package),
        };
        assert_eq!(
            info_from_manifest(&pkg, manifest, 7).state,
            PluginUpdateState::Unsupported
        );
    }

    #[test]
    fn treats_old_manifest_without_arch_package_as_up_to_date() {
        let pkg = PluginPackageMeta {
            id: "org.test".into(),
            name: "Test".into(),
            version: "2.0.0".into(),
            update: Some("https://example.invalid/update.json".into()),
            has_entry: false,
            system: true,
            incompat: None,
        };
        let manifest = PluginUpdateManifest {
            version: "1.0.0".into(),
            entry_version: source::ENTRY_VERSION,
            spawn_version: renderer_manager::SPAWN_VERSION,
            x86_64: None,
            aarch64: None,
        };
        let info = info_from_manifest(&pkg, manifest, 7);
        assert_eq!(info.state, PluginUpdateState::UpToDate);
        assert_eq!(info.latest_version, "1.0.0");
    }

    #[tokio::test]
    async fn preserves_existing_store_entries_for_partial_checks() {
        let store = new_store();
        let a = PluginPackageMeta {
            id: "org.a".into(),
            name: "A".into(),
            version: "1.0.0".into(),
            update: None,
            has_entry: false,
            system: true,
            incompat: None,
        };
        let b = PluginPackageMeta {
            id: "org.b".into(),
            name: "B".into(),
            version: "1.0.0".into(),
            update: None,
            has_entry: false,
            system: true,
            incompat: None,
        };

        check_packages(&store, vec![a.clone(), b.clone()], true).await;
        check_packages(&store, vec![a.clone()], false).await;
        assert!(store.read().await.contains_key("org.b"));

        check_packages(&store, vec![a], true).await;
        assert!(!store.read().await.contains_key("org.b"));
    }

    #[test]
    fn became_available_only_for_new_available_state() {
        let current = PluginUpdateInfo {
            plugin_id: "org.test".into(),
            state: PluginUpdateState::Available,
            latest_version: "2.0.0".into(),
            zip_url: "https://example.invalid/plugin.zip".into(),
            sha256: "abc".into(),
            error: String::new(),
            checked_at_ms: 1,
        };
        assert!(became_available(None, &current));

        let mut previous = current.clone();
        previous.checked_at_ms = 0;
        assert!(!became_available(Some(&previous), &current));

        previous.latest_version = "1.9.0".into();
        assert!(became_available(Some(&previous), &current));

        let mut failed = current.clone();
        failed.state = PluginUpdateState::Failed;
        assert!(!became_available(None, &failed));
    }
}
