use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ashpd::desktop::wallpaper::{SetOn, WallpaperRequest};
use tokio::sync::{watch, Mutex};

use crate::DaemonContext;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Change {
    pub wallpaper_id: i64,
    pub renderer_id: String,
    pub image: Option<PathBuf>,
    pub cleared: bool,
    pub retake: bool,
}

enum Image {
    Cached(PathBuf),
    Source(PathBuf),
    Snapshot(Arc<std::fs::File>),
}

const CACHE_BYTES: u64 = 512 * 1024 * 1024;
const CACHE_FILES: usize = 128;
const UPDATE_TIMEOUT: Duration = Duration::from_secs(130);

#[derive(Default)]
pub(crate) struct Cache {
    current: Option<PathBuf>,
    installing: Option<PathBuf>,
    discard_installing: bool,
}

pub(crate) fn cache_directory() -> PathBuf {
    crate::settings::data_dir().join("system-wallpaper")
}

struct SnapshotDemand {
    snapshot: Arc<crate::wallframe::snapshot::Snapshot>,
    router: Arc<crate::wallframe::routing::Router>,
}

impl Drop for SnapshotDemand {
    fn drop(&mut self) {
        self.snapshot.cancel();
        let router = Arc::clone(&self.router);
        tokio::spawn(async move { router.reconcile_lifecycle().await });
    }
}

pub(crate) async fn run(app: Arc<DaemonContext>, mut changes: watch::Receiver<Option<Change>>) {
    let directory = cache_directory();
    {
        let mut cache = app.system_wallpaper_cache.lock().await;
        let directory = directory.clone();
        match tokio::task::spawn_blocking(move || load_current(&directory)).await {
            Ok(Ok(current)) => cache.current = current,
            Ok(Err(error)) => log::warn!("load current system wallpaper: {error:#}"),
            Err(error) => log::warn!("load current system wallpaper: {error}"),
        }
    }
    let monitor_app = app.clone();
    app.tasks.spawn_async(
        crate::tasks::TaskKind::Service,
        "wallpaper/integration",
        async move { watch_integration(monitor_app).await },
    );
    let cleanup_app = Arc::clone(&app);
    app.tasks.spawn_async(
        crate::tasks::TaskKind::Service,
        "wallpaper/cleanup",
        async move {
            watch_catalog(cleanup_app).await;
            Ok(())
        },
    );
    let mut shutdown = app.shutdown_subscribe();
    loop {
        let change = changes.borrow_and_update().clone();
        if let Some(change) =
            change.filter(|change| !change.cleared && app.settings.system_wallpaper_enabled())
        {
            let prepared = tokio::select! {
                result = prepare(&app, &change, &directory) => result,
                _ = changes.changed() => continue,
                _ = shutdown.changed() => break,
            };
            let result = match prepared {
                Ok(image) => tokio::select! {
                    result = update(&app, &changes, &directory, &change, image) => result,
                    _ = shutdown.changed() => break,
                },
                Err(error) => Err(error),
            };
            match result {
                Ok(true) => log::info!("system wallpaper updated for {}", change.wallpaper_id),
                Err(error) if !changes.has_changed().unwrap_or(true) => {
                    report_failure(&app, change.wallpaper_id, error);
                }
                _ => {}
            }
        }
        tokio::select! {
            result = changes.changed() => if result.is_err() { break; },
            _ = shutdown.changed() => break,
        }
    }
}

fn report_failure(app: &DaemonContext, wallpaper_id: i64, error: anyhow::Error) {
    log::warn!("system wallpaper {wallpaper_id}: {error:#}");
    app.events
        .publish(crate::events::GlobalEvent::SystemWallpaperFailed {
            wallpaper_id: wallpaper_id.to_string(),
            error: format!("{error:#}"),
        });
}

async fn prepare(app: &DaemonContext, change: &Change, directory: &Path) -> Result<Image> {
    if !change.retake {
        let directory = directory.to_path_buf();
        let id = change.wallpaper_id;
        if let Some(path) =
            tokio::task::spawn_blocking(move || cached_image(&directory, id)).await??
        {
            return Ok(Image::Cached(path));
        }
    }
    if let Some(image) = &change.image {
        return Ok(Image::Source(image.clone()));
    }
    let renderer = loop {
        if let Some(renderer) = app.renderer_manager.get(&change.renderer_id).await {
            break renderer;
        }
        // Deferred assignments start their renderer when a desktop becomes visible.
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let captured = tokio::time::timeout(Duration::from_secs(30), async {
        renderer.snapshot.request().await;
        let _demand = SnapshotDemand {
            snapshot: Arc::clone(&renderer.snapshot),
            router: Arc::clone(&app.router),
        };
        app.router.reconcile_lifecycle().await;
        app.renderer_manager
            .request_frame(&change.renderer_id)
            .await?;
        renderer.snapshot.wait().await
    })
    .await
    .context("timed out waiting for wallpaper snapshot")??;
    Ok(Image::Snapshot(captured))
}

async fn update(
    app: &DaemonContext,
    changes: &watch::Receiver<Option<Change>>,
    directory: &Path,
    change: &Change,
    image: Image,
) -> Result<bool> {
    let Some(path) = persist_current(
        image,
        directory.to_path_buf(),
        change.wallpaper_id,
        changes,
        &app.db,
        &app.system_wallpaper_cache,
    )
    .await?
    else {
        return Ok(false);
    };
    if change.retake {
        // A new provider can reuse the persisted retake. Preserve any newer request.
        app.system_wallpaper.send_if_modified(|selected| {
            if !changes.has_changed().unwrap_or(true) {
                if let Some(selected) = selected {
                    selected.retake = false;
                }
            }
            false
        });
    }
    // The provider may wait for user confirmation. Only the image is pinned during this call.
    let result = tokio::time::timeout(UPDATE_TIMEOUT, async {
        let connection = zbus::Connection::session().await?;
        let settings = app.settings.global();
        if !settings.effective_system_wallpaper_enabled() {
            return Ok(false);
        }
        install_on(
            &connection,
            &path,
            settings.system_wallpaper_enabled.is_none(),
            is_current(changes, &app.db, change.wallpaper_id),
        )
        .await
    })
    .await
    .unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "desktop integration did not respond to the wallpaper update"
        ))
    });
    let cache = Arc::clone(&app.system_wallpaper_cache).lock_owned().await;
    let installed = matches!(result, Ok(true));
    if let Err(error) = tokio::task::spawn_blocking(move || {
        let mut cache = cache;
        cache.finish_install(installed)
    })
    .await?
    {
        log::warn!("finish system wallpaper cache update: {error:#}");
    }
    result
}

async fn is_current(
    changes: &watch::Receiver<Option<Change>>,
    db: &sea_orm::DatabaseConnection,
    wallpaper_id: i64,
) -> Result<bool> {
    Ok(!changes.has_changed()?
        && crate::model::repo::get_entry(db, wallpaper_id)
            .await?
            .is_some()
        && !changes.has_changed()?)
}

async fn persist_current(
    image: Image,
    directory: PathBuf,
    wallpaper_id: i64,
    changes: &watch::Receiver<Option<Change>>,
    db: &sea_orm::DatabaseConnection,
    cache: &Arc<Mutex<Cache>>,
) -> Result<Option<PathBuf>> {
    let cache = Arc::clone(cache).lock_owned().await;
    if !is_current(changes, db, wallpaper_id).await? {
        return Ok(None);
    }
    tokio::task::spawn_blocking(move || {
        let mut cache = cache;
        let slot = cache
            .current
            .as_deref()
            .and_then(cache_owner)
            .map_or(0, |(_, slot)| 1 - slot);
        let reused = matches!(&image, Image::Cached(_));
        let path = persist_image(image, &directory, slot, wallpaper_id)?;
        if let Err(error) = prune_cache(
            &directory,
            cache.current.as_deref(),
            Some(&path),
            CACHE_BYTES,
            CACHE_FILES,
        ) {
            if !reused {
                remove_file(&path).context("remove excess wallpaper cache file")?;
            }
            return Err(error);
        }
        cache.installing = Some(path.clone());
        cache.discard_installing = false;
        Ok(Some(path))
    })
    .await?
}

fn persist_image(image: Image, directory: &Path, slot: u8, wallpaper_id: i64) -> Result<PathBuf> {
    if wallpaper_id <= 0 || slot > 1 {
        bail!("invalid snapshot owner or slot");
    }
    let (input, extension) = match image {
        Image::Cached(path) => {
            if !std::fs::symlink_metadata(&path)?.is_file() {
                bail!("cached system wallpaper is not a regular file");
            }
            return Ok(path);
        }
        Image::Source(source) => {
            let extension = source
                .extension()
                .and_then(|value| value.to_str())
                .filter(|value| {
                    value.len() <= 12 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
                .unwrap_or("img")
                .to_owned();
            (
                std::fs::File::options()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(source)
                    .context("open system wallpaper image")?,
                extension,
            )
        }
        Image::Snapshot(file) => {
            let mut input = file.try_clone()?;
            input.seek(SeekFrom::Start(0))?;
            (input, "bmp".to_owned())
        }
    };
    if !input.metadata()?.is_file() {
        bail!("system wallpaper image is not a regular file");
    }
    std::fs::create_dir_all(directory)?;
    let target = directory.join(format!("wallpaper-{wallpaper_id}-{slot}.{extension}"));
    let mut output = tempfile::NamedTempFile::new_in(directory)?;
    // Leave room for the installed image while the integration loads its replacement.
    if std::io::copy(&mut input.take(CACHE_BYTES / 2 + 1), &mut output)? > CACHE_BYTES / 2 {
        bail!("system wallpaper image exceeds the 256 MiB cache limit per image");
    }
    output.as_file().sync_all()?;
    output.persist(&target)?;
    Ok(target)
}

fn remove_replaced_snapshots(path: &Path) -> Result<()> {
    let Some((wallpaper_id, _)) = cache_owner(path) else {
        return Ok(());
    };
    let directory = path.parent().context("saved wallpaper has no directory")?;
    for (id, previous) in cache_files(directory)? {
        if id == wallpaper_id && previous != path {
            remove_file(&previous)?;
        }
    }
    Ok(())
}

fn cache_owner(path: &Path) -> Option<(i64, u8)> {
    let (id, rest) = path
        .file_name()?
        .to_str()?
        .strip_prefix("wallpaper-")?
        .split_once('-')?;
    let (slot, extension) = rest.split_once('.')?;
    let id = id.parse::<i64>().ok()?;
    let slot = slot.parse::<u8>().ok()?;
    (id > 0 && slot <= 1 && !extension.is_empty()).then_some((id, slot))
}

fn cache_files(directory: &Path) -> Result<Vec<(i64, PathBuf)>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if let Some((id, _)) = cache_owner(&path) {
            files.push((id, path));
        }
    }
    Ok(files)
}

fn cached_image(directory: &Path, wallpaper_id: i64) -> Result<Option<PathBuf>> {
    newest_cached_file(directory, |id, _| id == wallpaper_id)
}

fn newest_cached_file(
    directory: &Path,
    matches: impl Fn(i64, &Path) -> bool,
) -> Result<Option<PathBuf>> {
    let mut latest = None;
    for (id, path) in cache_files(directory)? {
        if matches(id, &path) {
            if let Some(metadata) = regular_file_metadata(&path)? {
                let candidate = (metadata.modified()?, path);
                if latest.as_ref().is_none_or(|saved| &candidate > saved) {
                    latest = Some(candidate);
                }
            }
        }
    }
    Ok(latest.map(|(_, path)| path))
}

fn regular_file_metadata(path: &Path) -> Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_file().then_some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn load_current(directory: &Path) -> Result<Option<PathBuf>> {
    let value = match std::fs::read_to_string(directory.join("current")) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let name = Path::new(value.trim());
    if let Ok(slot) = value.trim().parse::<u8>() {
        // Older snapshots recorded the alternating slot without the owner's ID.
        return newest_cached_file(directory, |_, path| {
            cache_owner(path).is_some_and(|(_, saved_slot)| saved_slot == slot)
        });
    }
    if name.file_name() == Some(name.as_os_str()) && cache_owner(name).is_some() {
        let path = directory.join(name);
        if regular_file_metadata(&path)?.is_some() {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn prune_cache(
    directory: &Path,
    current: Option<&Path>,
    installing: Option<&Path>,
    byte_limit: u64,
    file_limit: usize,
) -> Result<()> {
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    let mut count = 0;
    for (_, path) in cache_files(directory)? {
        let Some(metadata) = regular_file_metadata(&path)? else {
            continue;
        };
        bytes = bytes.saturating_add(metadata.len());
        count += 1;
        if Some(path.as_path()) != current && Some(path.as_path()) != installing {
            entries.push((metadata.modified()?, path, metadata.len()));
        }
    }
    entries.sort_unstable();
    for (_, path, len) in entries {
        if bytes <= byte_limit && count <= file_limit {
            break;
        }
        remove_file(&path)?;
        bytes -= len;
        count -= 1;
    }
    if bytes > byte_limit || count > file_limit {
        bail!("the installed and pending system wallpaper images exceed the cache limit");
    }
    Ok(())
}

fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

impl Cache {
    fn remove(&mut self, path: &Path) -> Result<()> {
        if self.installing.as_deref() == Some(path) {
            self.discard_installing = true;
        } else {
            remove_file(path)?;
        }
        if self.current.as_deref() == Some(path) {
            self.current = None;
        }
        Ok(())
    }

    fn clear(&mut self, directory: &Path) -> Result<()> {
        for (_, path) in cache_files(directory)? {
            self.remove(&path)?;
        }
        remove_file(&directory.join("current"))
    }

    fn finish_install(&mut self, installed: bool) -> Result<()> {
        let Some(path) = self.installing.take() else {
            return Ok(());
        };
        if std::mem::take(&mut self.discard_installing) {
            return remove_file(&path);
        }
        if installed {
            self.current = Some(path.clone());
            let directory = path.parent().context("saved wallpaper has no directory")?;
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("invalid saved wallpaper filename")?;
            let mut marker = tempfile::NamedTempFile::new_in(directory)?;
            marker.write_all(filename.as_bytes())?;
            marker.as_file().sync_all()?;
            marker.persist(directory.join("current"))?;
            remove_replaced_snapshots(&path)?;
        }
        Ok(())
    }
}

pub(crate) async fn clear_snapshots(app: &DaemonContext) -> Result<()> {
    let cache = Arc::clone(&app.system_wallpaper_cache).lock_owned().await;
    app.system_wallpaper.send_modify(|change| {
        if let Some(change) = change {
            change.cleared = true;
        }
    });
    tokio::task::spawn_blocking(move || {
        let mut cache = cache;
        cache.clear(&cache_directory())
    })
    .await?
}

pub(crate) fn retake_snapshot(app: &DaemonContext) -> Result<()> {
    if !app.settings.system_wallpaper_enabled() {
        bail!("Enable system wallpaper updates before retaking a snapshot");
    }
    let change = app
        .system_wallpaper
        .borrow()
        .clone()
        .context("No wallpaper is selected for the system background")?;
    request_retake(&app.system_wallpaper, change)
}

fn request_retake(changes: &watch::Sender<Option<Change>>, expected: Change) -> Result<()> {
    if !changes.send_if_modified(|current| {
        if current.as_ref() != Some(&expected) {
            return false;
        }
        let mut change = expected.clone();
        change.cleared = false;
        change.retake = true;
        *current = Some(change);
        true
    }) {
        bail!("The selected wallpaper changed while preparing the snapshot; try again");
    }
    Ok(())
}

async fn watch_catalog(app: Arc<DaemonContext>) {
    let directory = cache_directory();
    let mut catalog = app.wallpaper_changes.subscribe();
    let mut shutdown = app.shutdown_subscribe();
    loop {
        if let Err(error) = cleanup_missing(
            &app.db,
            &directory,
            &app.system_wallpaper_cache,
            &app.system_wallpaper,
        )
        .await
        {
            log::warn!("system wallpaper snapshot cleanup: {error:#}");
        }
        tokio::select! {
            result = catalog.changed() => if result.is_err() { break; },
            _ = shutdown.changed() => break,
        }
    }
}

async fn cleanup_missing(
    db: &sea_orm::DatabaseConnection,
    directory: &Path,
    cache: &Arc<Mutex<Cache>>,
    changes: &watch::Sender<Option<Change>>,
) -> Result<()> {
    let cache = Arc::clone(cache).lock_owned().await;
    let directory = directory.to_path_buf();
    let files = {
        let directory = directory.clone();
        tokio::task::spawn_blocking(move || cache_files(&directory)).await??
    };
    let ids = files
        .iter()
        .map(|(id, _)| *id)
        .chain(changes.borrow().iter().map(|change| change.wallpaper_id))
        .collect::<std::collections::HashSet<_>>();
    let mut removed = std::collections::HashSet::new();
    // Legacy caches can contain more items than a single SQLite IN clause accepts.
    let ids: Vec<_> = ids.into_iter().collect();
    for chunk in ids.chunks(CACHE_FILES) {
        let present = crate::model::repo::get_entries_ordered(db, chunk).await?;
        removed.extend(
            chunk
                .iter()
                .copied()
                .filter(|id| !present.iter().any(|entry| entry.item_id == *id)),
        );
    }
    changes.send_if_modified(|change| {
        if change
            .as_ref()
            .is_some_and(|change| removed.contains(&change.wallpaper_id))
        {
            *change = None;
            true
        } else {
            false
        }
    });
    tokio::task::spawn_blocking(move || {
        let mut cache = cache;
        for (id, path) in files {
            if removed.contains(&id) {
                cache.remove(&path)?;
            }
        }
        prune_cache(
            &directory,
            cache.current.as_deref(),
            cache.installing.as_deref(),
            CACHE_BYTES,
            CACHE_FILES,
        )
    })
    .await?
}

pub(crate) async fn apply_portal(path: &Path) -> Result<()> {
    let image = std::fs::File::open(path).context("open portal wallpaper")?;
    WallpaperRequest::default()
        .set_on(SetOn::Background)
        .show_preview(false)
        .build_file(&image.as_fd())
        .await
        .context("SetWallpaperFile")?
        .response()
        .context("SetWallpaperFile response")?;
    Ok(())
}

const BACKGROUND_BUS_NAME: &str = "org.waywallen.Display.Background";
const BACKGROUND_OBJECT_PATH: &str = "/org/waywallen/Display/Background";
const BACKGROUND_INTERFACE: &str = "org.waywallen.Display.Background1";
const AUTO_UPDATE_PROPERTY: &str = "CanAutoUpdateStaticBackground";

async fn background_proxy(connection: &zbus::Connection) -> Result<zbus::Proxy<'_>> {
    let bus = zbus::fdo::DBusProxy::new(connection).await?;
    let owner = bus.get_name_owner(BACKGROUND_BUS_NAME.try_into()?).await?;
    // Keep capability checks and installation on the same integration after a name handover.
    Ok(zbus::Proxy::new(
        connection,
        owner,
        BACKGROUND_OBJECT_PATH,
        BACKGROUND_INTERFACE,
    )
    .await?)
}

async fn automatic_updates_allowed(integration: &zbus::Proxy<'_>) -> bool {
    tokio::time::timeout(
        Duration::from_secs(3),
        integration.get_property::<bool>(AUTO_UPDATE_PROPERTY),
    )
    .await
    .is_ok_and(|result| result.unwrap_or(false))
}

async fn watch_integration(app: Arc<DaemonContext>) -> Result<()> {
    use futures_util::StreamExt;
    let connection = zbus::Connection::session().await?;
    let proxy = zbus::fdo::DBusProxy::new(&connection).await?;
    let mut owners = proxy
        .receive_name_owner_changed_with_args(&[(0, BACKGROUND_BUS_NAME)])
        .await?;
    let mut shutdown = app.shutdown_subscribe();
    loop {
        let enabled = match background_proxy(&connection).await {
            Ok(integration) => automatic_updates_allowed(&integration).await,
            Err(_) => false,
        };
        if app.settings.set_system_wallpaper_default_enabled(enabled) {
            app.events
                .publish(crate::events::GlobalEvent::SettingsChanged);
        }
        // Also retry when the integration was ready before the subscription.
        app.system_wallpaper.send_modify(|_| {});
        tokio::select! {
            signal = owners.next() => {
                if signal.is_none() { break; }
            }
            _ = shutdown.changed() => break,
        }
    }
    if app.settings.set_system_wallpaper_default_enabled(false) {
        app.events
            .publish(crate::events::GlobalEvent::SettingsChanged);
        app.system_wallpaper.send_modify(|_| {});
    }
    Ok(())
}

async fn install_on(
    connection: &zbus::Connection,
    path: &Path,
    automatic: bool,
    is_current: impl std::future::Future<Output = Result<bool>>,
) -> Result<bool> {
    let integration = background_proxy(connection).await?;
    if (automatic && !automatic_updates_allowed(&integration).await) || !is_current.await? {
        return Ok(false);
    }
    integration
        .call::<_, _, ()>(
            "SetStaticBackground",
            &(path.to_str().context("non-UTF8 wallpaper path")?,),
        )
        .await
        .context("desktop integration could not update the system wallpaper")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn install(connection: &zbus::Connection, path: &Path, automatic: bool) -> Result<bool> {
        install_on(connection, path, automatic, std::future::ready(Ok(true))).await
    }

    #[tokio::test]
    async fn cancelled_retake_clears_the_snapshot_demand() {
        let snapshot = Arc::new(crate::wallframe::snapshot::Snapshot::default());
        let router = crate::wallframe::routing::Router::new(Arc::new(
            crate::wallframe::renderer_manager::RendererManager::new_default(),
        ));
        let (started, running) = tokio::sync::oneshot::channel();
        let captured = Arc::clone(&snapshot);
        let task = tokio::spawn(async move {
            captured.request().await;
            let _demand = SnapshotDemand {
                snapshot: captured,
                router,
            };
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        running.await.unwrap();
        assert!(snapshot.requested());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!snapshot.requested());
    }

    #[test]
    fn retake_reuses_the_selected_wallpaper_after_clearing_and_rejects_stale_requests() {
        let selected = Change {
            wallpaper_id: 1,
            renderer_id: "running-web".into(),
            image: None,
            cleared: true,
            retake: false,
        };
        let (changes, mut receiver) = watch::channel(Some(selected.clone()));
        request_retake(&changes, selected.clone()).unwrap();
        let current = receiver.borrow_and_update().clone().unwrap();
        assert!(!current.cleared);
        assert!(current.retake);
        assert_eq!(current.wallpaper_id, selected.wallpaper_id);
        assert_eq!(current.renderer_id, selected.renderer_id);
        request_retake(&changes, current.clone()).unwrap();
        assert!(receiver.has_changed().unwrap());

        let next = Change {
            wallpaper_id: 2,
            ..current.clone()
        };
        changes.send_replace(Some(next.clone()));
        assert!(request_retake(&changes, current).is_err());
        assert!(changes.borrow().as_ref() == Some(&next));
        changes.send_replace(None);
        assert!(request_retake(&changes, selected).is_err());
        assert!(changes.borrow().is_none());
    }

    #[test]
    fn clearing_snapshots_keeps_originals_and_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bmp");
        let cache = directory.path().join("backgrounds");
        Cache::default().clear(&cache).unwrap();
        std::fs::write(&source, b"original").unwrap();
        let first = persist_image(Image::Source(source.clone()), &cache, 0, 1).unwrap();
        let second = persist_image(Image::Source(source.clone()), &cache, 1, 2).unwrap();
        let unrelated = cache.join("unrelated.bmp");
        std::fs::write(&unrelated, b"keep").unwrap();
        std::os::unix::fs::symlink(&source, cache.join("wallpaper-3-0.bmp")).unwrap();
        Cache::default().clear(&cache).unwrap();
        assert!(!first.exists());
        assert!(!second.exists());
        assert!(!cache.join("wallpaper-3-0.bmp").exists());
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
        Cache::default().clear(&cache).unwrap();
    }

    #[tokio::test]
    async fn integration_receives_the_image_and_its_failure_is_not_success() {
        use std::io::BufRead;
        struct Bus(std::process::Child);
        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn();
        let Ok(child) = child else {
            eprintln!("dbus-daemon unavailable; skipping private-bus integration test");
            return;
        };
        let mut bus = Bus(child);
        let mut address = String::new();
        std::io::BufReader::new(bus.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim();
        struct Integration(
            Arc<tokio::sync::Mutex<Vec<String>>>,
            Arc<std::sync::atomic::AtomicBool>,
        );
        #[zbus::interface(name = "org.waywallen.Display.Background1")]
        impl Integration {
            #[zbus(property)]
            fn can_auto_update_static_background(&self) -> bool {
                self.1.load(std::sync::atomic::Ordering::SeqCst)
            }

            async fn set_static_background(&self, path: String) -> zbus::fdo::Result<()> {
                self.0.lock().await.push(path.clone());
                if path.ends_with("fail.png") {
                    return Err(zbus::fdo::Error::Failed(
                        "test wallpaper service unavailable".into(),
                    ));
                }
                Ok(())
            }
        }
        let received = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let allowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let provider = zbus::connection::Builder::address(address)
            .unwrap()
            .name(BACKGROUND_BUS_NAME)
            .unwrap()
            .serve_at(
                BACKGROUND_OBJECT_PATH,
                Integration(received.clone(), allowed.clone()),
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        let client = zbus::connection::Builder::address(address)
            .unwrap()
            .build()
            .await
            .unwrap();
        let path = Path::new("/tmp/image with 'quotes'.png");
        assert!(!automatic_updates_allowed(&background_proxy(&client).await.unwrap()).await);
        assert!(!install(&client, path, true).await.unwrap());
        assert!(received.lock().await.is_empty());
        allowed.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(automatic_updates_allowed(&background_proxy(&client).await.unwrap()).await);
        assert!(install(&client, path, true).await.unwrap());
        received.lock().await.clear();
        allowed.store(false, std::sync::atomic::Ordering::SeqCst);
        install(&client, path, false).await.unwrap();
        assert_eq!(*received.lock().await, ["/tmp/image with 'quotes'.png"]);
        assert!(
            !install_on(&client, path, false, std::future::ready(Ok(false)))
                .await
                .unwrap()
        );
        assert_eq!(received.lock().await.len(), 1);
        let error = install(&client, Path::new("/tmp/fail.png"), false)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("test wallpaper service unavailable"));

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bmp");
        let cache = directory.path().join("backgrounds");
        std::fs::write(&source, b"old snapshot").unwrap();
        let old = persist_image(Image::Source(source.clone()), &cache, 0, 1).unwrap();
        let other = persist_image(Image::Source(source.clone()), &cache, 0, 2).unwrap();
        let failed = cache.join("wallpaper-1-1.fail.png");
        std::fs::write(&failed, b"failed snapshot").unwrap();
        assert!(install(&client, &failed, false).await.is_err());
        assert_eq!(std::fs::read(&old).unwrap(), b"old snapshot");
        assert!(failed.exists());

        std::fs::write(&source, b"new snapshot").unwrap();
        let new = persist_image(Image::Source(source.clone()), &cache, 1, 1).unwrap();
        assert!(old.exists());
        install(&client, &new, false).await.unwrap();
        let mut state = Cache {
            installing: Some(new.clone()),
            ..Cache::default()
        };
        state.finish_install(true).unwrap();
        assert_eq!(load_current(&cache).unwrap(), Some(new.clone()));
        assert!(!old.exists());
        assert!(!failed.exists());
        assert_eq!(std::fs::read(&new).unwrap(), b"new snapshot");
        assert_eq!(std::fs::read(&other).unwrap(), b"old snapshot");
        assert_eq!(std::fs::read(&source).unwrap(), b"new snapshot");

        provider.close().await.unwrap();
        struct LegacyIntegration;
        #[zbus::interface(name = "org.waywallen.Display.Background1")]
        impl LegacyIntegration {
            fn set_static_background(&self, _path: String) {}
        }
        let legacy = zbus::connection::Builder::address(address)
            .unwrap()
            .name(BACKGROUND_BUS_NAME)
            .unwrap()
            .serve_at(BACKGROUND_OBJECT_PATH, LegacyIntegration)
            .unwrap()
            .build()
            .await
            .unwrap();
        assert!(!automatic_updates_allowed(&background_proxy(&client).await.unwrap()).await);
        assert!(!install(&client, path, true).await.unwrap());
        assert!(install(&client, path, false).await.unwrap());
        legacy.close().await.unwrap();
        assert!(install(&client, Path::new("/tmp/image.png"), false)
            .await
            .is_err());
    }

    #[test]
    fn snapshots_survive_source_removal_and_switching_wallpapers() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bmp");
        let cache = directory.path().join("backgrounds");
        std::fs::write(&source, b"first").unwrap();
        let first = persist_image(Image::Source(source.clone()), &cache, 0, 1).unwrap();
        std::fs::write(&source, b"second").unwrap();
        let second = persist_image(Image::Source(source.clone()), &cache, 1, 2).unwrap();
        std::fs::remove_file(&source).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        let replacement = directory.path().join("source.png");
        std::fs::write(&replacement, b"third").unwrap();
        let third = persist_image(Image::Source(replacement.clone()), &cache, 0, 3).unwrap();
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 3);
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        assert_eq!(std::fs::read(&third).unwrap(), b"third");

        let retaken = persist_image(Image::Source(replacement), &cache, 0, 1).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        remove_replaced_snapshots(&retaken).unwrap();
        assert!(!first.exists());
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 3);
        assert_eq!(std::fs::read(retaken).unwrap(), b"third");
        assert_eq!(std::fs::read(second).unwrap(), b"second");
        assert_eq!(std::fs::read(third).unwrap(), b"third");
    }

    #[test]
    fn reapplying_uses_the_latest_saved_image_without_rewriting_it() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("backgrounds");
        assert!(cached_image(&cache, 1).unwrap().is_none());
        let source = directory.path().join("source.bmp");
        std::fs::write(&source, b"old snapshot").unwrap();
        let old = persist_image(Image::Source(source.clone()), &cache, 0, 1).unwrap();
        std::fs::write(&source, b"retaken snapshot").unwrap();
        let latest = persist_image(Image::Source(source.clone()), &cache, 1, 1).unwrap();
        persist_image(Image::Source(source), &cache, 0, 2).unwrap();
        let saved_at = std::time::UNIX_EPOCH + Duration::from_secs(2);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1))
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&latest)
            .unwrap()
            .set_modified(saved_at)
            .unwrap();

        let saved = cached_image(&cache, 1).unwrap().unwrap();
        assert_eq!(saved, latest);
        let reused = persist_image(Image::Cached(saved), &cache, 0, 1).unwrap();
        assert_eq!(reused, latest);
        assert_eq!(std::fs::read(&reused).unwrap(), b"retaken snapshot");
        assert_eq!(
            std::fs::metadata(reused).unwrap().modified().unwrap(),
            saved_at
        );
        assert_eq!(std::fs::read(old).unwrap(), b"old snapshot");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 3);
        assert!(cached_image(&cache, 3).unwrap().is_none());
        Cache::default().clear(&cache).unwrap();
        assert!(cached_image(&cache, 1).unwrap().is_none());
    }

    #[tokio::test]
    async fn superseded_snapshot_never_reaches_desktop() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bmp");
        std::fs::write(&source, b"old frame").unwrap();
        let (changes, receiver) = watch::channel(None);
        changes.send_replace(Some(Change {
            wallpaper_id: 1,
            renderer_id: "new".into(),
            image: None,
            cleared: false,
            retake: false,
        }));
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let installed = persist_current(
            Image::Source(source),
            directory.path().join("backgrounds"),
            1,
            &receiver,
            &db,
            &Arc::new(Mutex::new(Cache::default())),
        )
        .await
        .unwrap();
        assert!(installed.is_none());
    }

    async fn item(db: &sea_orm::DatabaseConnection, path: &str) -> i64 {
        use crate::model::repo;
        let plugin = repo::upsert_plugin(db, "snapshot-test", "1").await.unwrap();
        let library = repo::add_library(db, plugin.id, &format!("/wallpapers/{path}"))
            .await
            .unwrap();
        repo::upsert_item(
            db,
            repo::ItemUpsertArgs {
                plugin_id: plugin.id,
                library_id: library.id,
                path,
                ty: "image",
                sub_type: "",
                display_name: "",
                preview_path: None,
                description: None,
                external_id: None,
                web_url: None,
                update_available: false,
                size: None,
                width: None,
                height: None,
                content_rating: None,
                can_remove: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn deletion_removes_only_the_deleted_wallpapers_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bmp");
        let cache = directory.path().join("backgrounds");
        std::fs::write(&source, b"frame").unwrap();
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let removed = item(&db, "removed").await;
        let retained = item(&db, "retained").await;
        let deleted = persist_image(Image::Source(source.clone()), &cache, 0, removed).unwrap();
        let kept = persist_image(Image::Source(source.clone()), &cache, 1, retained).unwrap();
        let deleted_other =
            persist_image(Image::Source(source.clone()), &cache, 1, removed).unwrap();
        let kept_other = persist_image(Image::Source(source.clone()), &cache, 0, retained).unwrap();
        std::fs::write(cache.join("unrelated.bmp"), b"untouched").unwrap();
        let state = Arc::new(Mutex::new(Cache {
            installing: Some(deleted.clone()),
            ..Cache::default()
        }));
        let (changes, _) = watch::channel(Some(Change {
            wallpaper_id: removed,
            renderer_id: "removed".into(),
            image: None,
            cleared: false,
            retake: false,
        }));
        crate::model::repo::delete_item(&db, removed).await.unwrap();
        cleanup_missing(&db, &cache, &state, &changes)
            .await
            .unwrap();
        assert!(
            deleted.exists(),
            "the active RPC still needs to open its image"
        );
        assert!(!deleted_other.exists());
        assert!(kept.exists());
        assert!(kept_other.exists());
        assert!(source.exists());
        assert!(cache.join("unrelated.bmp").exists());
        assert!(changes.borrow().is_none());
        state.lock().await.finish_install(true).unwrap();
        assert!(!deleted.exists());
        assert!(!cache.join("current").exists());
        cleanup_missing(&db, &cache, &state, &changes)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn late_update_cannot_recreate_a_deleted_wallpapers_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.bmp");
        std::fs::write(&source, b"old frame").unwrap();
        let db = crate::model::connect_url("sqlite::memory:").await.unwrap();
        let id = item(&db, "removed").await;
        let (_changes, receiver) = watch::channel(None);
        let cache = directory.path().join("backgrounds");
        let io = Arc::new(Mutex::new(Cache::default()));
        let guard = Arc::clone(&io).lock_owned().await;
        let write_db = db.clone();
        let write_cache = cache.clone();
        let writer = tokio::spawn(async move {
            persist_current(
                Image::Source(source),
                write_cache,
                id,
                &receiver,
                &write_db,
                &io,
            )
            .await
        });
        tokio::task::yield_now().await;
        crate::model::repo::delete_item(&db, id).await.unwrap();
        drop(guard);
        assert!(writer.await.unwrap().unwrap().is_none());
        assert!(!cache.exists());
    }

    #[test]
    fn cache_prunes_old_files_and_keeps_installed_and_pending_images() {
        let directory = tempfile::tempdir().unwrap();
        let current = directory.path().join("wallpaper-1-0.bmp");
        let candidate = directory.path().join("wallpaper-2-1.bmp");
        let old = directory.path().join("wallpaper-3-1.bmp");
        let recent = directory.path().join("wallpaper-4-1.bmp");
        for (seconds, path) in [&current, &candidate, &old, &recent]
            .into_iter()
            .enumerate()
        {
            std::fs::write(path, b"1234").unwrap();
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(seconds as u64))
                .unwrap();
        }
        prune_cache(directory.path(), Some(&current), Some(&candidate), 12, 4).unwrap();
        assert!(!old.exists());
        assert!(recent.exists());
        prune_cache(directory.path(), Some(&current), Some(&candidate), 12, 2).unwrap();
        assert!(!recent.exists());
        assert!(current.exists() && candidate.exists());
    }

    #[test]
    fn cached_frame_can_be_copied_more_than_once() {
        let directory = tempfile::tempdir().unwrap();
        let mut file = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut file, b"captured frame").unwrap();
        let file = Arc::new(file);
        for slot in [0, 1] {
            let path = persist_image(
                Image::Snapshot(Arc::clone(&file)),
                directory.path(),
                slot,
                1,
            )
            .unwrap();
            assert_eq!(std::fs::read(path).unwrap(), b"captured frame");
        }
    }
}
