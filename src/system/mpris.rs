use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::zvariant::OwnedValue;

use crate::tasks::TaskKind;
use crate::wallframe::renderer_manager::{
    MprisSnapshot, RendererEventKind, RendererId, RendererSubscriptionSnapshot,
};
use crate::DaemonContext;

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

const STATE_STOPPED: u32 = 0;
const STATE_PLAYING: u32 = 1;
const STATE_PAUSED: u32 = 2;
const LOG_TEXT_MAX_CHARS: usize = 80;
const MAX_ART_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ART_CACHE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DATA_URI_BYTES: usize = 12 * 1024 * 1024;
const MAX_ART_DOWNLOADS: usize = 2;
const MAX_ART_FAILURES: usize = 128;
const ART_FAILURE_TTL: Duration = Duration::from_secs(30);
const MAX_ART_URI_BYTES: usize = 4096;

enum PlayerMsg {
    Snapshot {
        name: String,
        snapshot: MprisSnapshot,
    },
    Gone(String),
    ArtReady {
        key: String,
        request_id: u64,
        success: bool,
    },
}

struct PlayerTask {
    handle: JoinHandle<()>,
}

struct ArtDownload {
    request_id: u64,
    handle: JoinHandle<()>,
}

/// Resolves MPRIS artwork into local files. The coordinator retains the raw
/// URI in player snapshots; only outgoing renderer snapshots are rewritten.
struct ArtCache {
    dir: PathBuf,
    client: Option<reqwest::Client>,
    in_flight: HashMap<String, ArtDownload>,
    failed: HashMap<String, Instant>,
    next_request_id: u64,
}

impl ArtCache {
    async fn new(dir: PathBuf) -> Self {
        if let Err(error) = tokio::fs::create_dir_all(&dir).await {
            log::warn!("mpris art cache unavailable at {}: {error}", dir.display());
        } else if let Err(error) = cleanup_art_cache(&dir).await {
            log::warn!("mpris art cache cleanup failed: {error:#}");
        }
        Self {
            dir,
            client: build_art_client()
                .inspect_err(|error| log::warn!("mpris artwork client unavailable: {error:#}"))
                .ok(),
            in_flight: HashMap::new(),
            failed: HashMap::new(),
            next_request_id: 0,
        }
    }

    async fn prepare_snapshot(
        &mut self,
        raw: &MprisSnapshot,
        tx: &mpsc::Sender<PlayerMsg>,
    ) -> MprisSnapshot {
        let mut snapshot = raw.clone();
        snapshot.art_url = self.resolve(&raw.art_url, true, tx).await;
        snapshot.previous_art_url = self.resolve(&raw.previous_art_url, false, tx).await;
        snapshot
    }

    async fn resolve(
        &mut self,
        raw: &str,
        fetch_remote: bool,
        tx: &mpsc::Sender<PlayerMsg>,
    ) -> String {
        if raw.is_empty() || raw.starts_with('/') {
            return raw.to_string();
        }
        if !is_cacheable_art_uri(raw) {
            return String::new();
        }
        let key = art_key(raw);
        let path = self.path_for_key(&key);
        if is_regular_file(&path) {
            self.failed.remove(&key);
            touch(&path);
            return path.to_string_lossy().into_owned();
        }
        if is_data_image_uri(raw) {
            return self.materialize_data(raw, key, path).await;
        }
        if !fetch_remote || self.in_flight.contains_key(&key) || !self.retry_ready(&key) {
            return String::new();
        }
        if self.in_flight.len() >= MAX_ART_DOWNLOADS {
            self.cancel_oldest().await;
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let client = self.client.clone();
        let task_uri = raw.to_string();
        let task_key = key.clone();
        let dir = self.dir.clone();
        let task_path = path;
        let task_tx = tx.clone();
        let handle = tokio::spawn(async move {
            let success = match materialize_art(&task_uri, client.as_ref(), &dir, &task_path).await
            {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("mpris artwork {task_key} failed: {error:#}");
                    false
                }
            };
            let _ = task_tx
                .send(PlayerMsg::ArtReady {
                    key: task_key,
                    request_id,
                    success,
                })
                .await;
        });
        self.in_flight
            .insert(key, ArtDownload { request_id, handle });
        String::new()
    }

    async fn materialize_data(&mut self, uri: &str, key: String, path: PathBuf) -> String {
        if !self.retry_ready(&key) {
            return String::new();
        }
        match materialize_art(uri, None, &self.dir, &path).await {
            Ok(()) => {
                self.failed.remove(&key);
                path.to_string_lossy().into_owned()
            }
            Err(error) => {
                log::warn!("mpris artwork {key} failed: {error:#}");
                self.remember_failure(key);
                String::new()
            }
        }
    }

    async fn cancel_oldest(&mut self) {
        let Some(key) = self
            .in_flight
            .iter()
            .min_by_key(|(_, download)| download.request_id)
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        let download = self.in_flight.remove(&key).unwrap();
        log::debug!("canceling oldest MPRIS artwork download {key}");
        download.handle.abort();
        let _ = download.handle.await;
    }

    fn finished(&mut self, key: &str, request_id: u64, success: bool) -> bool {
        if self
            .in_flight
            .get(key)
            .is_none_or(|download| download.request_id != request_id)
        {
            return false;
        }
        self.in_flight.remove(key);
        if success {
            self.failed.remove(key);
        } else {
            self.remember_failure(key.to_string());
        }
        true
    }

    fn retry_ready(&mut self, key: &str) -> bool {
        let Some(retry_at) = self.failed.get(key).copied() else {
            return true;
        };
        if retry_at > Instant::now() {
            return false;
        }
        self.failed.remove(key);
        true
    }

    fn remember_failure(&mut self, key: String) {
        if !self.failed.contains_key(&key) && self.failed.len() >= MAX_ART_FAILURES {
            if let Some(oldest) = self
                .failed
                .iter()
                .min_by_key(|(_, retry_at)| **retry_at)
                .map(|(key, _)| key.clone())
            {
                self.failed.remove(&oldest);
            }
        }
        self.failed.insert(key, Instant::now() + ART_FAILURE_TTL);
    }

    fn path_for_key(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.img"))
    }
}

pub fn spawn(app: Arc<DaemonContext>) {
    let task_app = app.clone();
    app.tasks
        .spawn_async(TaskKind::Service, "service/mpris", async move {
            run(task_app).await
        });
}

async fn run(app: Arc<DaemonContext>) -> Result<()> {
    let conn = match zbus::Connection::session().await {
        Ok(conn) => conn,
        Err(e) => {
            log::warn!("cannot connect to D-Bus session bus: {e}");
            return Ok(());
        }
    };
    log::debug!("connected to D-Bus session bus");
    let dbus = match DBusProxy::new(&conn).await {
        Ok(proxy) => proxy,
        Err(e) => {
            log::warn!("org.freedesktop.DBus proxy unavailable: {e}");
            return Ok(());
        }
    };
    let mut name_stream = match dbus.receive_name_owner_changed().await {
        Ok(stream) => stream,
        Err(e) => {
            log::warn!("NameOwnerChanged subscription failed: {e}");
            return Ok(());
        }
    };

    let (tx, mut rx) = mpsc::channel::<PlayerMsg>(64);
    let mut shutdown = app.shutdown_subscribe();
    let mut subscriptions = app.renderer_manager.subscribe_subscriptions();
    let mut tasks: BTreeMap<String, PlayerTask> = BTreeMap::new();
    let mut players: BTreeMap<String, MprisSnapshot> = BTreeMap::new();
    let mut current = MprisSnapshot::default();
    let mut art_cache = ArtCache::new(crate::settings::mpris_art_cache_dir()).await;
    let mut known_subscribers = BTreeMap::new();
    let mut discovered = 0usize;

    match dbus.list_names().await {
        Ok(names) => {
            for name in names {
                let name = name.as_str().to_string();
                if is_mpris_name(&name) {
                    discovered += 1;
                    spawn_player_watch(
                        &mut tasks,
                        conn.clone(),
                        name,
                        tx.clone(),
                        app.shutdown_subscribe(),
                    );
                }
            }
        }
        Err(e) => log::warn!("ListNames failed: {e}"),
    }
    log::debug!("discovered {discovered} existing MPRIS player(s)");
    let initial_subscribers = {
        let snapshot = subscriptions.borrow_and_update();
        mpris_subscribers(&snapshot)
    };
    let initial_targets = updated_subscribers(&known_subscribers, &initial_subscribers);
    known_subscribers = initial_subscribers;
    if !initial_targets.is_empty() {
        publish_to_renderers(
            &app,
            &mut art_cache,
            &current,
            &initial_targets,
            "subscription",
            &tx,
        )
        .await;
    }

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            msg = rx.recv() => {
                let Some(msg) = msg else { break; };
                let mut refresh_art = false;
                match msg {
                    PlayerMsg::Snapshot { name, snapshot } => {
                        log::trace!("snapshot from {name}: {}", snapshot_debug(&snapshot));
                        players.insert(name, snapshot);
                    }
                    PlayerMsg::Gone(name) => {
                        log::debug!("player gone: {name}");
                        players.remove(&name);
                        if let Some(task) = tasks.remove(&name) {
                            stop_player_task(task).await;
                        }
                    }
                    PlayerMsg::ArtReady {
                        key,
                        request_id,
                        success,
                    } => {
                        let accepted = art_cache.finished(&key, request_id, success);
                        let completed_current = is_cacheable_art_uri(&current.art_url)
                            && art_key(&current.art_url) == key;
                        refresh_art = accepted && (success || !completed_current);
                    }
                }
                let next = choose_snapshot(&players);
                if next != current || (refresh_art && !known_subscribers.is_empty()) {
                    current = next;
                    publish_current_snapshot(
                        &app,
                        &mut subscriptions,
                        &mut known_subscribers,
                        &current,
                        &mut art_cache,
                        &tx,
                    ).await;
                }
            }
            signal = name_stream.next() => {
                let Some(signal) = signal else { break; };
                match signal.args() {
                    Ok(args) => {
                        let name = args.name.as_str().to_string();
                        if !is_mpris_name(&name) {
                            continue;
                        }
                        let appeared = args.new_owner.as_ref().is_some();
                        log::debug!("NameOwnerChanged name={name} appeared={appeared}");
                        if appeared {
                            spawn_player_watch(
                                &mut tasks,
                                conn.clone(),
                                name,
                                tx.clone(),
                                app.shutdown_subscribe(),
                            );
                        } else {
                            players.remove(&name);
                            if let Some(task) = tasks.remove(&name) {
                                stop_player_task(task).await;
                            }
                            let next = choose_snapshot(&players);
                            if next != current {
                                current = next;
                                publish_current_snapshot(
                                    &app,
                                    &mut subscriptions,
                                    &mut known_subscribers,
                                    &current,
                                    &mut art_cache,
                                    &tx,
                                ).await;
                            }
                        }
                    }
                    Err(e) => log::warn!("bad NameOwnerChanged signal: {e}"),
                }
            }
            changed = subscriptions.changed() => {
                if changed.is_err() {
                    break;
                }
                let next_subscribers = {
                    let snapshot = subscriptions.borrow_and_update();
                    mpris_subscribers(&snapshot)
                };
                let targets = updated_subscribers(&known_subscribers, &next_subscribers);
                known_subscribers = next_subscribers;
                if !targets.is_empty() {
                    publish_to_renderers(&app, &mut art_cache, &current, &targets, "subscription", &tx).await;
                }
            }
        }
    }

    for (_, task) in tasks {
        stop_player_task(task).await;
    }
    Ok(())
}

async fn stop_player_task(task: PlayerTask) {
    task.handle.abort();
    let _ = task.handle.await;
}

fn spawn_player_watch(
    tasks: &mut BTreeMap<String, PlayerTask>,
    conn: zbus::Connection,
    name: String,
    tx: mpsc::Sender<PlayerMsg>,
    shutdown: watch::Receiver<bool>,
) {
    if tasks.contains_key(&name) {
        log::debug!("player watch already running for {name}");
        return;
    }
    log::debug!("spawning player watch for {name}");
    let task_name = name.clone();
    let handle = tokio::spawn(async move {
        watch_player(conn, task_name, tx, shutdown).await;
    });
    tasks.insert(name, PlayerTask { handle });
}

async fn watch_player(
    conn: zbus::Connection,
    name: String,
    tx: mpsc::Sender<PlayerMsg>,
    mut shutdown: watch::Receiver<bool>,
) {
    let proxy = match zbus::Proxy::new(&conn, name.as_str(), MPRIS_PATH, MPRIS_PLAYER_IFACE).await {
        Ok(proxy) => proxy,
        Err(e) => {
            log::warn!("player proxy unavailable for {name}: {e}");
            let _ = tx.send(PlayerMsg::Gone(name)).await;
            return;
        }
    };
    let props_builder = match PropertiesProxy::builder(&conn).destination(name.as_str()) {
        Ok(builder) => builder,
        Err(e) => {
            log::warn!("properties proxy destination failed for {name}: {e}");
            let _ = tx.send(PlayerMsg::Gone(name)).await;
            return;
        }
    };
    let props_builder = match props_builder.path(MPRIS_PATH) {
        Ok(builder) => builder,
        Err(e) => {
            log::warn!("properties proxy path failed for {name}: {e}");
            let _ = tx.send(PlayerMsg::Gone(name)).await;
            return;
        }
    };
    let props = match props_builder.build().await {
        Ok(proxy) => proxy,
        Err(e) => {
            log::warn!("properties proxy unavailable for {name}: {e}");
            let _ = tx.send(PlayerMsg::Gone(name)).await;
            return;
        }
    };
    let mut changes = match props.receive_properties_changed().await {
        Ok(stream) => stream,
        Err(e) => {
            log::warn!("PropertiesChanged subscription failed for {name}: {e}");
            let _ = tx.send(PlayerMsg::Gone(name)).await;
            return;
        }
    };

    log::debug!("watching player {name}");
    let mut last_art_url = String::new();
    let mut previous_art_url = String::new();
    if let Some(snapshot) =
        read_player_snapshot(&proxy, &mut last_art_url, &mut previous_art_url).await
    {
        log::trace!("initial snapshot for {name}: {}", snapshot_debug(&snapshot));
        let _ = tx
            .send(PlayerMsg::Snapshot {
                name: name.clone(),
                snapshot,
            })
            .await;
    }

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            signal = changes.next() => {
                let Some(signal) = signal else { break; };
                if signal
                    .args()
                    .map(|args| args.interface_name.as_str() != MPRIS_PLAYER_IFACE)
                    .unwrap_or(false)
                {
                    continue;
                }
                log::trace!("PropertiesChanged for {name}");
                if let Some(snapshot) =
                    read_player_snapshot(&proxy, &mut last_art_url, &mut previous_art_url).await
                {
                    log::trace!(
                        "updated snapshot for {name}: {}",
                        snapshot_debug(&snapshot)
                    );
                    let _ = tx.send(PlayerMsg::Snapshot {
                        name: name.clone(),
                        snapshot,
                    }).await;
                }
            }
        }
    }
}

async fn read_player_snapshot(
    proxy: &zbus::Proxy<'_>,
    last_art_url: &mut String,
    previous_art_url: &mut String,
) -> Option<MprisSnapshot> {
    let status = proxy
        .get_property::<String>("PlaybackStatus")
        .await
        .unwrap_or_else(|_| "Stopped".to_string());
    let metadata = proxy
        .get_property::<HashMap<String, OwnedValue>>("Metadata")
        .await
        .unwrap_or_default();

    let art_url = normalize_metadata_art_url(&metadata_string(&metadata, "mpris:artUrl"));
    if art_url != *last_art_url {
        if !last_art_url.is_empty() {
            *previous_art_url = last_art_url.clone();
        }
        *last_art_url = art_url.clone();
    }

    Some(MprisSnapshot {
        state: playback_state_from_status(&status),
        title: metadata_string(&metadata, "xesam:title"),
        artist: metadata_string_list(&metadata, "xesam:artist"),
        album: metadata_string(&metadata, "xesam:album"),
        album_artist: metadata_string_list(&metadata, "xesam:albumArtist"),
        art_url,
        previous_art_url: previous_art_url.clone(),
    })
}

fn choose_snapshot(players: &BTreeMap<String, MprisSnapshot>) -> MprisSnapshot {
    players
        .values()
        .find(|s| s.state == STATE_PLAYING)
        .or_else(|| players.values().find(|s| snapshot_has_media(s)))
        .or_else(|| players.values().next())
        .cloned()
        .unwrap_or_default()
}

fn snapshot_has_media(s: &MprisSnapshot) -> bool {
    !s.title.is_empty()
        || !s.artist.is_empty()
        || !s.album.is_empty()
        || !s.album_artist.is_empty()
        || !s.art_url.is_empty()
}

type MprisSubscribers = BTreeMap<RendererId, u64>;

fn mpris_subscribers(snapshot: &RendererSubscriptionSnapshot) -> MprisSubscribers {
    snapshot
        .subscribers_with_membership_generation(RendererEventKind::Mpris)
        .into_iter()
        .collect()
}

fn updated_subscribers(previous: &MprisSubscribers, current: &MprisSubscribers) -> Vec<RendererId> {
    current
        .iter()
        .filter_map(|(id, revision)| (previous.get(id) != Some(revision)).then(|| id.clone()))
        .collect()
}

async fn publish_current_snapshot(
    app: &DaemonContext,
    subscriptions: &mut watch::Receiver<RendererSubscriptionSnapshot>,
    known_subscribers: &mut MprisSubscribers,
    snapshot: &MprisSnapshot,
    art_cache: &mut ArtCache,
    tx: &mpsc::Sender<PlayerMsg>,
) {
    let subscribers = {
        let snapshot = subscriptions.borrow_and_update();
        mpris_subscribers(&snapshot)
    };
    let targets: Vec<_> = subscribers.keys().cloned().collect();
    *known_subscribers = subscribers;
    publish_to_renderers(app, art_cache, snapshot, &targets, "state change", tx).await;
}

async fn publish_to_renderers(
    app: &DaemonContext,
    art_cache: &mut ArtCache,
    raw_snapshot: &MprisSnapshot,
    ids: &[RendererId],
    reason: &str,
    tx: &mpsc::Sender<PlayerMsg>,
) {
    if ids.is_empty() {
        return;
    }
    if reason == "subscription" && !raw_snapshot.art_url.is_empty() {
        art_cache.failed.remove(&art_key(&raw_snapshot.art_url));
    }
    let snapshot = art_cache.prepare_snapshot(raw_snapshot, tx).await;
    log::debug!(
        "publishing {reason} snapshot to {} renderer(s): {}",
        ids.len(),
        snapshot_debug(&snapshot)
    );
    for id in ids {
        if let Err(e) = app.renderer_manager.send_mpris(id, snapshot.clone()).await {
            log::warn!("failed to send snapshot to renderer {id}: {e:#}");
        }
    }
}

fn is_mpris_name(name: &str) -> bool {
    name.starts_with(MPRIS_PREFIX) && name.len() > MPRIS_PREFIX.len()
}

fn playback_state_from_status(status: &str) -> u32 {
    match status {
        "Playing" => STATE_PLAYING,
        "Paused" => STATE_PAUSED,
        _ => STATE_STOPPED,
    }
}

fn playback_state_label(state: u32) -> &'static str {
    match state {
        STATE_PLAYING => "Playing",
        STATE_PAUSED => "Paused",
        _ => "Stopped",
    }
}

fn snapshot_debug(snapshot: &MprisSnapshot) -> String {
    format!(
        "state={} title={:?} artist={:?} art={} previous_art={}",
        playback_state_label(snapshot.state),
        truncate_log_text(&snapshot.title),
        truncate_log_text(&snapshot.artist),
        art_url_summary(&snapshot.art_url),
        art_url_summary(&snapshot.previous_art_url),
    )
}

fn truncate_log_text(value: &str) -> String {
    let mut chars = value.chars();
    let mut truncated: String = chars.by_ref().take(LOG_TEXT_MAX_CHARS).collect();
    if chars.next().is_some() {
        truncated.push('…');
    }
    truncated
}

fn art_url_summary(value: &str) -> String {
    let kind = if value.is_empty() {
        return "none".to_string();
    } else if value.starts_with("data:") {
        "data"
    } else if value.starts_with('/') {
        "file"
    } else if value.contains("://") {
        "url"
    } else {
        "value"
    };
    format!("{kind}({} bytes)", value.len())
}

fn metadata_string(metadata: &HashMap<String, OwnedValue>, key: &str) -> String {
    metadata
        .get(key)
        .and_then(owned_value_string)
        .unwrap_or_default()
}

fn metadata_string_list(metadata: &HashMap<String, OwnedValue>, key: &str) -> String {
    metadata
        .get(key)
        .and_then(|v| {
            v.try_clone()
                .ok()
                .and_then(|v| Vec::<String>::try_from(v).ok())
                .map(|items| items.join(", "))
                .or_else(|| owned_value_string(v))
        })
        .unwrap_or_default()
}

fn owned_value_string(value: &OwnedValue) -> Option<String> {
    value
        .try_clone()
        .ok()
        .and_then(|value| String::try_from(value).ok())
}

fn normalize_art_url(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix("file://") else {
        return raw.to_string();
    };
    let path = if let Some(after_localhost) = rest.strip_prefix("localhost/") {
        format!("/{after_localhost}")
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        return raw.to_string();
    };
    percent_decode(&path).unwrap_or(path)
}

fn normalize_metadata_art_url(raw: &str) -> String {
    if is_data_image_uri(raw) {
        if raw.len() <= MAX_DATA_URI_BYTES {
            return raw.to_string();
        }
    } else if !is_data_uri(raw) && raw.len() <= MAX_ART_URI_BYTES {
        return normalize_art_url(raw);
    }

    log::debug!(
        "rejecting unsupported or oversized MPRIS artwork URI: {}",
        art_url_summary(raw)
    );
    String::new()
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_val(bytes[i + 1])?;
            let lo = hex_val(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn is_cacheable_art_uri(raw: &str) -> bool {
    is_data_image_uri(raw)
        || reqwest::Url::parse(raw)
            .map(|url| url.scheme() == "https")
            .unwrap_or(false)
}

fn is_data_image_uri(raw: &str) -> bool {
    raw.get(..11)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:image/"))
}

fn is_data_uri(raw: &str) -> bool {
    raw.get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
}

fn art_key(uri: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(uri.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn touch(path: &Path) {
    let _ = std::fs::File::open(path)
        .and_then(|file| file.set_times(std::fs::FileTimes::new().set_modified(SystemTime::now())));
}

fn build_art_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(3))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent("waywallen-mpris-art/1")
        .build()
        .context("build MPRIS artwork client")
}

async fn materialize_art(
    uri: &str,
    client: Option<&reqwest::Client>,
    dir: &Path,
    path: &Path,
) -> Result<()> {
    let bytes = if is_data_uri(uri) {
        let uri = uri.to_string();
        tokio::task::spawn_blocking(move || decode_data_image(&uri))
            .await
            .context("join artwork decode task")??
    } else {
        download_art(
            client.ok_or_else(|| anyhow!("artwork HTTP client is unavailable"))?,
            uri,
        )
        .await?
    };
    write_art_file(dir, path, &bytes).await?;
    if let Err(error) = cleanup_art_cache(dir).await {
        log::warn!("mpris art cache cleanup failed: {error:#}");
    }
    Ok(())
}

async fn download_art(client: &reqwest::Client, uri: &str) -> Result<Vec<u8>> {
    let response = client
        .get(uri)
        .send()
        .await
        .context("request artwork")?
        .error_for_status()
        .context("artwork response")?;
    if let Some(length) = response.content_length() {
        if length > MAX_ART_BYTES {
            return Err(anyhow!("artwork exceeds {MAX_ART_BYTES} byte limit"));
        }
    }
    if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let content_type = content_type.to_str().unwrap_or_default();
        if !content_type.to_ascii_lowercase().starts_with("image/") {
            return Err(anyhow!("artwork content type is not an image"));
        }
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read artwork response")?;
        if bytes.len().saturating_add(chunk.len()) > MAX_ART_BYTES as usize {
            return Err(anyhow!("artwork exceeds {MAX_ART_BYTES} byte limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(anyhow!("artwork response is empty"));
    }
    Ok(bytes)
}

fn decode_data_image(uri: &str) -> Result<Vec<u8>> {
    if uri.len() > MAX_DATA_URI_BYTES {
        return Err(anyhow!("data URI is too large"));
    }
    let (meta, payload) = uri
        .get(5..)
        .filter(|_| is_data_uri(uri))
        .and_then(|value| value.split_once(','))
        .ok_or_else(|| anyhow!("invalid data URI"))?;
    let mime = meta
        .split(';')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !mime.starts_with("image/") || mime == "image/svg+xml" {
        return Err(anyhow!("unsupported data URI media type"));
    }
    let bytes = if meta
        .split(';')
        .any(|parameter| parameter.eq_ignore_ascii_case("base64"))
    {
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .context("decode base64 artwork")?
    } else {
        percent_decode_bytes(payload).ok_or_else(|| anyhow!("invalid percent-encoded artwork"))?
    };
    if bytes.is_empty() || bytes.len() > MAX_ART_BYTES as usize {
        return Err(anyhow!("decoded data artwork exceeds size limit"));
    }
    Ok(bytes)
}

fn percent_decode_bytes(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            out.push((hex_val(bytes[i + 1])? << 4) | hex_val(bytes[i + 2])?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

struct PartialArtFile {
    path: Option<PathBuf>,
}

impl PartialArtFile {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn commit(&mut self) {
        self.path = None;
    }
}

impl Drop for PartialArtFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn write_art_file(dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    let part = dir.join(format!(".part-{}", uuid::Uuid::new_v4()));
    let mut partial = PartialArtFile::new(part.clone());
    let mut file = tokio::fs::File::create(&part).await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&part, path).await?;
    partial.commit();
    Ok(())
}

async fn cleanup_art_cache(dir: &Path) -> Result<()> {
    cleanup_art_cache_with_limit(dir, MAX_ART_CACHE_BYTES).await
}

async fn cleanup_art_cache_with_limit(dir: &Path, max_bytes: u64) -> Result<()> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    let mut total = 0u64;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".part-") {
            let _ = tokio::fs::remove_file(path).await;
            continue;
        }
        let metadata = entry.metadata().await?;
        if !metadata.is_file() {
            continue;
        }
        total = total.saturating_add(metadata.len());
        files.push((
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            metadata.len(),
            path,
        ));
    }
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in files {
        if total <= max_bytes {
            break;
        }
        if tokio::fs::remove_file(path).await.is_ok() {
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_art_cache(dir: PathBuf) -> ArtCache {
        ArtCache {
            dir,
            client: None,
            in_flight: HashMap::new(),
            failed: HashMap::new(),
            next_request_id: 0,
        }
    }

    #[test]
    fn normalizes_local_file_art_url() {
        assert_eq!(
            normalize_art_url("file:///home/me/Cover%20Art.png"),
            "/home/me/Cover Art.png"
        );
        assert_eq!(
            normalize_art_url("file://localhost/tmp/a%23b.jpg"),
            "/tmp/a#b.jpg"
        );
        assert_eq!(
            normalize_art_url("file://remote/tmp/a.jpg"),
            "file://remote/tmp/a.jpg"
        );
    }

    #[test]
    fn preserves_bounded_data_images_for_cache_materialization() {
        let data_image = "data:image/png;base64,aGVsbG8=";
        assert_eq!(normalize_metadata_art_url(data_image), data_image);
        assert_eq!(
            normalize_metadata_art_url("file:///tmp/Cover%20Art.png"),
            "/tmp/Cover Art.png"
        );
        assert!(normalize_metadata_art_url("data:text/plain;base64,aGVsbG8=").is_empty());

        let oversized = format!("data:image/png;base64,{}", "A".repeat(MAX_DATA_URI_BYTES));
        assert!(normalize_metadata_art_url(&oversized).is_empty());
    }

    #[test]
    fn maps_playback_status() {
        assert_eq!(playback_state_from_status("Stopped"), STATE_STOPPED);
        assert_eq!(playback_state_from_status("Playing"), STATE_PLAYING);
        assert_eq!(playback_state_from_status("Paused"), STATE_PAUSED);
        assert_eq!(playback_state_from_status("Other"), STATE_STOPPED);
    }

    #[test]
    fn chooses_playing_player_first() {
        let mut players = BTreeMap::new();
        players.insert(
            "org.mpris.MediaPlayer2.paused".to_string(),
            MprisSnapshot {
                state: STATE_PAUSED,
                title: "Paused".to_string(),
                ..MprisSnapshot::default()
            },
        );
        players.insert(
            "org.mpris.MediaPlayer2.playing".to_string(),
            MprisSnapshot {
                state: STATE_PLAYING,
                title: "Playing".to_string(),
                ..MprisSnapshot::default()
            },
        );
        assert_eq!(choose_snapshot(&players).title, "Playing");
    }

    #[test]
    fn selects_new_and_resubscribed_renderers_in_stable_order() {
        let previous = BTreeMap::from([
            ("keep".to_string(), 2),
            ("revised".to_string(), 3),
            ("removed".to_string(), 1),
        ]);
        let current = BTreeMap::from([
            ("added".to_string(), 1),
            ("keep".to_string(), 2),
            ("revised".to_string(), 4),
        ]);

        assert_eq!(
            updated_subscribers(&previous, &current),
            vec!["added".to_string(), "revised".to_string()]
        );
        assert!(updated_subscribers(&current, &current).is_empty());
        assert!(updated_subscribers(&current, &BTreeMap::new()).is_empty());
    }

    #[test]
    fn snapshot_debug_bounds_text_and_hides_art_payloads() {
        let long_title = "猫".repeat(LOG_TEXT_MAX_CHARS + 10);
        let payload = "A".repeat(256);
        let art_url = format!("data:image/jpeg;base64,{payload}");
        let snapshot = MprisSnapshot {
            state: STATE_PLAYING,
            title: long_title,
            artist: "Artist".to_string(),
            art_url: art_url.clone(),
            previous_art_url: "/tmp/cover.jpg".to_string(),
            ..MprisSnapshot::default()
        };

        let summary = snapshot_debug(&snapshot);
        assert!(!summary.contains(&payload));
        assert!(summary.contains(&format!("data({} bytes)", art_url.len())));
        assert!(summary.contains("file(14 bytes)"));
        assert_eq!(
            truncate_log_text(&snapshot.title).chars().count(),
            LOG_TEXT_MAX_CHARS + 1
        );
        assert!(truncate_log_text(&snapshot.title).ends_with('…'));
    }

    #[test]
    fn decodes_base64_and_percent_encoded_data_images() {
        assert_eq!(
            decode_data_image("data:image/png;base64,aGVsbG8=").unwrap(),
            b"hello"
        );
        assert_eq!(
            decode_data_image("data:image/png,hello%20world").unwrap(),
            b"hello world"
        );
        assert!(decode_data_image("data:text/plain;base64,aGVsbG8=").is_err());
        assert!(decode_data_image("data:image/svg+xml,%3Csvg%2F%3E").is_err());
        assert!(is_cacheable_art_uri("HTTPS://example.invalid/cover.png"));
        assert!(!is_cacheable_art_uri("http://example.invalid/cover.png"));
        assert!(!is_cacheable_art_uri("ftp://example.invalid/cover.png"));
    }

    #[tokio::test]
    async fn cache_materializes_data_images_before_preparing_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = test_art_cache(tmp.path().to_path_buf());
        let (tx, mut rx) = mpsc::channel(1);
        let current = "data:image/png;base64,aGVsbG8=";
        let previous = "data:image/png;base64,d29ybGQ=";

        let prepared = cache
            .prepare_snapshot(
                &MprisSnapshot {
                    art_url: current.to_string(),
                    previous_art_url: previous.to_string(),
                    ..MprisSnapshot::default()
                },
                &tx,
            )
            .await;
        assert_eq!(tokio::fs::read(&prepared.art_url).await.unwrap(), b"hello");
        assert_eq!(
            tokio::fs::read(&prepared.previous_art_url).await.unwrap(),
            b"world"
        );
        assert!(rx.try_recv().is_err());
        assert!(cache.in_flight.is_empty());
    }

    #[tokio::test]
    async fn cache_keeps_http_materialization_in_background() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = test_art_cache(tmp.path().to_path_buf());
        let (tx, mut rx) = mpsc::channel(1);
        let uri = "https://example.invalid/cover.png";
        let key = art_key(uri);

        assert!(cache.resolve(uri, true, &tx).await.is_empty());
        let request_id = cache.in_flight[&key].request_id;
        let PlayerMsg::ArtReady {
            key: completed_key,
            request_id: completed_request_id,
            success,
        } = rx.recv().await.unwrap()
        else {
            panic!("unexpected MPRIS message");
        };
        assert_eq!(completed_key, key);
        assert_eq!(completed_request_id, request_id);
        assert!(!success);
        assert!(cache.finished(&key, request_id, success));
    }

    #[tokio::test]
    async fn cache_hit_rewrites_https_art_without_starting_a_download() {
        let tmp = tempfile::tempdir().unwrap();
        let uri = "https://example.invalid/cover.png";
        let path = tmp.path().join(format!("{}.img", art_key(uri)));
        tokio::fs::write(&path, b"cached").await.unwrap();
        let mut cache = ArtCache::new(tmp.path().to_path_buf()).await;
        let (tx, _rx) = mpsc::channel(1);

        let resolved = cache.resolve(uri, true, &tx).await;
        assert_eq!(resolved, path.to_string_lossy());
        assert!(cache.in_flight.is_empty());
    }

    #[tokio::test]
    async fn cache_cancels_oldest_download_before_starting_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = test_art_cache(tmp.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(8);
        let first = "https://example.invalid/first.png";
        let second = "https://example.invalid/second.png";
        let third = "https://example.invalid/third.png";

        cache
            .prepare_snapshot(
                &MprisSnapshot {
                    art_url: first.to_string(),
                    ..MprisSnapshot::default()
                },
                &tx,
            )
            .await;
        cache
            .prepare_snapshot(
                &MprisSnapshot {
                    art_url: second.to_string(),
                    previous_art_url: first.to_string(),
                    ..MprisSnapshot::default()
                },
                &tx,
            )
            .await;
        assert_eq!(cache.in_flight.len(), MAX_ART_DOWNLOADS);
        let first_request_id = cache.in_flight[&art_key(first)].request_id;

        cache
            .prepare_snapshot(
                &MprisSnapshot {
                    art_url: third.to_string(),
                    previous_art_url: second.to_string(),
                    ..MprisSnapshot::default()
                },
                &tx,
            )
            .await;
        assert_eq!(cache.in_flight.len(), MAX_ART_DOWNLOADS);
        assert!(!cache.in_flight.contains_key(&art_key(first)));
        assert!(cache.in_flight.contains_key(&art_key(second)));
        assert!(cache.in_flight.contains_key(&art_key(third)));

        cache.resolve(first, true, &tx).await;
        let restarted_request_id = cache.in_flight[&art_key(first)].request_id;
        assert_ne!(restarted_request_id, first_request_id);
        assert!(!cache.finished(&art_key(first), first_request_id, false));
        assert!(cache.in_flight.contains_key(&art_key(first)));
        assert!(!cache.failed.contains_key(&art_key(first)));
    }

    #[tokio::test(start_paused = true)]
    async fn cache_expires_bounded_failure_entries_on_demand() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = test_art_cache(tmp.path().to_path_buf());
        let (tx, _rx) = mpsc::channel(1);
        let uri = "https://example.invalid/cover.png";
        let key = art_key(uri);
        cache.remember_failure(key.clone());

        assert!(cache.resolve(uri, true, &tx).await.is_empty());
        assert!(cache.in_flight.is_empty());

        tokio::time::advance(ART_FAILURE_TTL).await;
        assert!(cache.resolve(uri, true, &tx).await.is_empty());
        assert!(cache.in_flight.contains_key(&key));

        for index in 0..(MAX_ART_FAILURES + 16) {
            cache.remember_failure(format!("failure-{index}"));
        }
        assert_eq!(cache.failed.len(), MAX_ART_FAILURES);
    }

    #[tokio::test]
    async fn cache_cleanup_removes_partial_and_oldest_entries_over_limit() {
        let tmp = tempfile::tempdir().unwrap();
        tokio::fs::write(tmp.path().join(".part-interrupted"), b"partial")
            .await
            .unwrap();
        let old = tmp.path().join("old.img");
        let new = tmp.path().join("new.img");
        tokio::fs::write(&old, b"123456").await.unwrap();
        std::fs::File::open(&old)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new().set_modified(
                    SystemTime::now()
                        .checked_sub(Duration::from_secs(60))
                        .unwrap(),
                ),
            )
            .unwrap();
        tokio::fs::write(&new, b"123456").await.unwrap();

        cleanup_art_cache_with_limit(tmp.path(), 10).await.unwrap();
        assert!(!old.exists());
        assert!(new.exists());
        assert!(!tmp.path().join(".part-interrupted").exists());
    }
}
