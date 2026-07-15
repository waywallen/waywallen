use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::watch;

use crate::plugin::display_registry::{DisplayDef, DisplayRegistry, SpawnMode};

pub const DISPLAY_BACKEND_STATE_DISABLED: &str = "disabled";
pub const DISPLAY_BACKEND_STATE_UNMATCHED: &str = "unmatched";
pub const DISPLAY_BACKEND_STATE_EXTERNAL: &str = "external";
pub const DISPLAY_BACKEND_STATE_READY: &str = "ready";
pub const DISPLAY_BACKEND_STATE_BINARY_MISSING: &str = "binary_missing";
pub const DISPLAY_BACKEND_STATE_FLATPAK_RESTRICTED: &str = "flatpak_restricted";

/// Observed desktop environment + Wayland capability snapshot.
#[derive(Debug, Default, Clone)]
pub struct DeCaps {
    /// Tokens from `XDG_CURRENT_DESKTOP` (lower-cased, split on `:`).
    /// Empty when the env var is unset.
    pub xdg_desktop: Vec<String>,
    /// `WAYLAND_DISPLAY` value, if any.
    pub wayland_display: Option<String>,
    /// True when `XDG_SESSION_TYPE == "wayland"`.
    pub is_wayland_session: bool,
    /// Placeholder for future `wl_registry` probe — list of global names
    /// like `"wlr-layer-shell"`, `"linux-dmabuf-v4"`, `"plasma-shell"`.
    pub probed_globals: Vec<String>,
    pub flatpak_id: Option<String>,
}

impl DeCaps {
    pub fn is_kde(&self) -> bool {
        self.xdg_desktop.iter().any(|t| t == "kde")
    }

    pub fn is_flatpak(&self) -> bool {
        self.flatpak_id.is_some()
    }
}

#[derive(Debug, Clone, Default)]
pub struct DisplayBackendStatus {
    pub name: String,
    pub state: String,
    pub desktop: String,
    pub binary: String,
    pub reason: String,
    pub flatpak_id: String,
}

impl DisplayBackendStatus {
    pub fn disabled(caps: &DeCaps) -> Self {
        Self {
            state: DISPLAY_BACKEND_STATE_DISABLED.to_string(),
            desktop: primary_desktop(caps),
            reason: "--no-display".to_string(),
            flatpak_id: flatpak_id(caps),
            ..Default::default()
        }
    }

    pub fn unmatched(caps: &DeCaps) -> Self {
        Self {
            state: DISPLAY_BACKEND_STATE_UNMATCHED.to_string(),
            desktop: primary_desktop(caps),
            reason: "no display backend matched this desktop".to_string(),
            flatpak_id: flatpak_id(caps),
            ..Default::default()
        }
    }

    pub fn external(def: &DisplayDef, caps: &DeCaps) -> Self {
        Self {
            name: def.name.clone(),
            state: DISPLAY_BACKEND_STATE_EXTERNAL.to_string(),
            desktop: primary_desktop(caps),
            binary: def.bin.display().to_string(),
            reason: "display backend is managed by the desktop".to_string(),
            flatpak_id: flatpak_id(caps),
        }
    }

    pub fn ready(def: &DisplayDef, caps: &DeCaps) -> Self {
        Self {
            name: def.name.clone(),
            state: DISPLAY_BACKEND_STATE_READY.to_string(),
            desktop: primary_desktop(caps),
            binary: def.bin.display().to_string(),
            reason: String::new(),
            flatpak_id: flatpak_id(caps),
        }
    }

    pub fn binary_missing(def: &DisplayDef, caps: &DeCaps) -> Self {
        Self {
            name: def.name.clone(),
            state: DISPLAY_BACKEND_STATE_BINARY_MISSING.to_string(),
            desktop: primary_desktop(caps),
            binary: def.bin.display().to_string(),
            reason: format!(
                "display backend binary '{}' was not found",
                def.bin.display()
            ),
            flatpak_id: flatpak_id(caps),
        }
    }

    pub fn flatpak_restricted(def: &DisplayDef, caps: &DeCaps) -> Self {
        Self {
            name: def.name.clone(),
            state: DISPLAY_BACKEND_STATE_FLATPAK_RESTRICTED.to_string(),
            desktop: primary_desktop(caps),
            binary: def.bin.display().to_string(),
            reason: "layer-shell Wayland protocols are not available inside Flatpak".to_string(),
            flatpak_id: flatpak_id(caps),
        }
    }
}

fn primary_desktop(caps: &DeCaps) -> String {
    caps.xdg_desktop.first().cloned().unwrap_or_default()
}

fn flatpak_id(caps: &DeCaps) -> String {
    caps.flatpak_id.clone().unwrap_or_default()
}

/// Read environment to populate `DeCaps`. Never panics; unset values are
/// left at their defaults.
pub fn detect_de() -> DeCaps {
    let xdg_desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .map(|s| {
            s.split(':')
                .filter(|p| !p.is_empty())
                .map(|p| p.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let wayland_display = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|s| !s.is_empty());
    let is_wayland_session = std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false);
    let flatpak_id = std::env::var("FLATPAK_ID").ok().filter(|s| !s.is_empty());
    DeCaps {
        xdg_desktop,
        wayland_display,
        is_wayland_session,
        probed_globals: Vec::new(),
        flatpak_id,
    }
}

/// Discover a live Wayland socket when `WAYLAND_DISPLAY` is unset by
/// scanning the runtime dir for `wayland-*` sockets. Unlike the env
/// var — frozen at process start — the socket appears on disk when the
/// compositor comes up, so this works for a daemon launched before the
/// session existed. Returns a value usable as `WAYLAND_DISPLAY`: the
/// socket name, or an absolute path when `XDG_RUNTIME_DIR` itself is
/// unset (libwayland accepts both).
pub fn scan_wayland_socket() -> Option<String> {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return scan_wayland_socket_in(Path::new(&dir));
    }
    // systemd's default runtime dir; keep the absolute path so the
    // spawned client doesn't need XDG_RUNTIME_DIR either.
    let uid = unsafe { libc::getuid() };
    let dir = PathBuf::from(format!("/run/user/{uid}"));
    scan_wayland_socket_in(&dir).map(|name| dir.join(name).to_string_lossy().into_owned())
}

fn scan_wayland_socket_in(dir: &Path) -> Option<String> {
    let mut best: Option<String> = None;
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("wayland-") || name.ends_with(".lock") {
            continue;
        }
        let is_socket = entry
            .file_type()
            .map(|t| {
                use std::os::unix::fs::FileTypeExt;
                t.is_socket()
            })
            .unwrap_or(false);
        if !is_socket {
            continue;
        }
        if best.as_deref().is_none_or(|cur| name < cur) {
            best = Some(name.to_string());
        }
    }
    best
}

/// Session environment as recorded by the systemd user manager.
/// Compositors publish `WAYLAND_DISPLAY` / `XDG_CURRENT_DESKTOP` there
/// via `dbus-update-activation-environment --systemd`, so unlike this
/// process's own environment it keeps updating after early boot.
/// Empty on non-systemd systems.
pub async fn systemd_user_environment(conn: &zbus::Connection) -> HashMap<String, String> {
    match read_systemd_environment(conn).await {
        Ok(map) => map,
        Err(e) => {
            log::debug!("systemd user environment unavailable: {e}");
            HashMap::new()
        }
    }
}

async fn read_systemd_environment(
    conn: &zbus::Connection,
) -> zbus::Result<HashMap<String, String>> {
    let proxy = zbus::Proxy::new(
        conn,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await?;
    let vars: Vec<String> = proxy.get_property("Environment").await?;
    Ok(vars
        .into_iter()
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect())
}

/// `detect_de()` plus runtime fallbacks for the early-boot case.
/// Returns the merged caps and the env overrides a spawned backend
/// needs — values discovered here are not in the daemon's own
/// environment, so children would not inherit them.
pub async fn detect_de_runtime(conn: Option<&zbus::Connection>) -> (DeCaps, Vec<(String, String)>) {
    let session = match conn {
        Some(conn) => systemd_user_environment(conn).await,
        None => HashMap::new(),
    };
    merge_caps(detect_de(), &session, scan_wayland_socket())
}

/// Fill holes in `base` from the systemd session env, then from an
/// on-disk socket scan. Process env always wins.
fn merge_caps(
    base: DeCaps,
    session: &HashMap<String, String>,
    socket: Option<String>,
) -> (DeCaps, Vec<(String, String)>) {
    let mut caps = base;
    let mut overrides = Vec::new();
    if caps.xdg_desktop.is_empty() {
        if let Some(raw) = session.get("XDG_CURRENT_DESKTOP") {
            let tokens: Vec<String> = raw
                .split(':')
                .filter(|p| !p.is_empty())
                .map(|p| p.to_ascii_lowercase())
                .collect();
            if !tokens.is_empty() {
                caps.xdg_desktop = tokens;
                overrides.push(("XDG_CURRENT_DESKTOP".to_string(), raw.clone()));
            }
        }
    }
    if !caps.is_wayland_session {
        if let Some(ty) = session.get("XDG_SESSION_TYPE") {
            caps.is_wayland_session = ty.eq_ignore_ascii_case("wayland");
        }
    }
    if caps.wayland_display.is_none() {
        let discovered = session
            .get("WAYLAND_DISPLAY")
            .filter(|s| !s.is_empty())
            .cloned()
            .or(socket);
        if let Some(display) = discovered {
            overrides.push(("WAYLAND_DISPLAY".to_string(), display.clone()));
            caps.wayland_display = Some(display);
        }
    }
    (caps, overrides)
}

/// Why `pick_backend` returned the choice it did. Mostly for logging.
#[derive(Debug, Clone)]
pub enum PickOutcome {
    /// KDE session hard-rule matched this backend.
    KdeHardMatch(DisplayDef),
    /// Highest-priority backend whose `de` matched and `requires` soft-passed.
    Matched(DisplayDef),
    /// No applicable backend — caller should log and run headless.
    None,
}

/// Hardcoded display backends bundled with the daemon. These are used
/// when no external manifest overrides them.
pub fn builtin_display_defs() -> Vec<DisplayDef> {
    let mut defs = Vec::new();

    // kde-plasma — Plasma 6 integration via the waywallen-kde kpackage.
    defs.push(DisplayDef {
        name: "kde-plasma".to_string(),
        bin: PathBuf::new(),
        de: vec!["kde".to_string()],
        priority: 100,
        requires: Vec::new(),
        extra_args: Vec::new(),
        spawn: SpawnMode::External,
    });

    // waywallen-layer-shell — Wayland layer-shell wallpaper client.
    // We look for the binary in the same directory as the daemon.
    let mut layer_shell_bin = PathBuf::from("waywallen-layer-shell");
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("waywallen-layer-shell");
            if candidate.exists() {
                layer_shell_bin = candidate;
            }
        }
    }

    defs.push(DisplayDef {
        name: "layer-shell".to_string(),
        bin: layer_shell_bin,
        de: vec![
            "hyprland".to_string(),
            "sway".to_string(),
            "niri".to_string(),
            "river".to_string(),
            "cosmic".to_string(),
        ],
        priority: 50,
        requires: vec!["wlr-layer-shell".to_string(), "linux-dmabuf-v4".to_string()],
        extra_args: Vec::new(),
        spawn: SpawnMode::Daemon,
    });

    defs
}

/// Select a backend from the registry or built-ins for the current environment.
/// See module docs for rules.
pub fn pick_backend(reg: &DisplayRegistry, caps: &DeCaps) -> PickOutcome {
    // Merge built-ins with registry. Registry entries shadow built-ins
    // by name (allowing user overrides).
    let mut all_defs: Vec<DisplayDef> = reg.all().to_vec();
    for builtin in builtin_display_defs() {
        if !all_defs.iter().any(|d| d.name == builtin.name) {
            all_defs.push(builtin);
        }
    }
    // Sort descending by priority.
    all_defs.sort_by(|a, b| b.priority.cmp(&a.priority));

    // Hard rule: KDE sessions use their dedicated backend (usually
    // spawn=external) and never fall back.
    if caps.is_kde() {
        if let Some(def) = all_defs
            .iter()
            .find(|d| d.de.iter().any(|t| t.eq_ignore_ascii_case("kde")))
        {
            return PickOutcome::KdeHardMatch(def.clone());
        }
        return PickOutcome::None;
    }

    let de_matches = |d: &DisplayDef| -> bool {
        if d.de.is_empty() {
            return true;
        }
        if d.de.iter().any(|t| t == "*") {
            return true;
        }
        // Any token in XDG_CURRENT_DESKTOP matching any `de` entry.
        for want in &d.de {
            if caps
                .xdg_desktop
                .iter()
                .any(|t| t.eq_ignore_ascii_case(want))
            {
                return true;
            }
        }
        false
    };

    // Soft capability check: only warn on missing `requires`; don't veto
    // until the real wl_registry probe lands. This keeps Hyprland/Sway
    let mut best: Option<DisplayDef> = None;
    for d in all_defs {
        if !de_matches(&d) {
            continue;
        }
        // Skip Plasma-targeted backends here — the KDE branch above owns
        // those. Prevents `kde-plasma` from leaking into a non-KDE pick.
        if d.de.iter().any(|t| t.eq_ignore_ascii_case("kde")) && !d.de.iter().any(|t| t == "*") {
            continue;
        }
        if !d.requires.is_empty() && !caps.probed_globals.is_empty() {
            let ok = d
                .requires
                .iter()
                .all(|r| caps.probed_globals.iter().any(|g| g == r));
            if !ok {
                log::debug!(
                    "display backend {} skipped: unmet requires {:?}",
                    d.name,
                    d.requires
                );
                continue;
            }
        }
        match best {
            None => best = Some(d),
            Some(ref cur) if d.priority > cur.priority => best = Some(d),
            _ => {}
        }
    }

    match best {
        Some(def) => PickOutcome::Matched(def),
        None => PickOutcome::None,
    }
}

/// Convenience: log the outcome at info level with enough detail to
/// debug a mis-selection from `journalctl`.
pub fn log_outcome(outcome: &PickOutcome, caps: &DeCaps) {
    match outcome {
        PickOutcome::KdeHardMatch(def) => log::info!(
            "display backend selected: {} (KDE hard-rule, spawn={:?}, xdg_desktop={:?})",
            def.name,
            def.spawn,
            caps.xdg_desktop
        ),
        PickOutcome::Matched(def) => log::info!(
            "display backend selected: {} (spawn={:?}, priority={}, xdg_desktop={:?})",
            def.name,
            def.spawn,
            def.priority,
            caps.xdg_desktop
        ),
        PickOutcome::None => {
            if caps.is_kde() {
                log::warn!(
                    "no KDE display backend registered; install waywallen-kde or configure a manifest"
                );
            } else {
                log::warn!(
                    "no display backend matched xdg_desktop={:?}; daemon will run in pure external-consumer mode",
                    caps.xdg_desktop
                );
            }
        }
    }
}

/// Return `true` when the daemon should start a subprocess for this
/// outcome. `External` backends rely on the DE to launch them (e.g.
pub fn should_daemon_spawn(outcome: &PickOutcome) -> bool {
    match outcome {
        PickOutcome::KdeHardMatch(def) | PickOutcome::Matched(def) => {
            matches!(def.spawn, SpawnMode::Daemon)
        }
        PickOutcome::None => false,
    }
}

pub fn resolve_backend_bin(bin: &Path) -> Option<PathBuf> {
    if bin.as_os_str().is_empty() {
        return None;
    }

    if bin.is_absolute() || bin.components().count() > 1 {
        return bin.is_file().then(|| bin.to_path_buf());
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn preflight_daemon_backend(
    def: DisplayDef,
    caps: &DeCaps,
) -> (DisplayBackendStatus, Option<DisplayDef>) {
    if caps.is_flatpak() && requires_layer_shell(&def) {
        return (DisplayBackendStatus::flatpak_restricted(&def, caps), None);
    }
    let Some(resolved) = resolve_backend_bin(&def.bin) else {
        return (DisplayBackendStatus::binary_missing(&def, caps), None);
    };
    let mut def = def;
    def.bin = resolved;
    (DisplayBackendStatus::ready(&def, caps), Some(def))
}

fn requires_layer_shell(def: &DisplayDef) -> bool {
    def.requires.iter().any(|r| r == "wlr-layer-shell")
}

// ---------------------------------------------------------------------------
// Subprocess supervision

/// Initial restart delay after a backend exits unexpectedly. Kept
/// generous so a crashing backend (e.g. a protocol error from a bad
const RESTART_INITIAL: Duration = Duration::from_secs(2);
/// Upper bound on the exponential backoff.
const RESTART_MAX: Duration = Duration::from_secs(10);

/// Initial delay between backend re-detection attempts when the daemon
/// started before the session environment existed.
pub const DETECT_RETRY_INITIAL: Duration = Duration::from_secs(2);
/// Upper bound on the re-detection backoff.
pub const DETECT_RETRY_MAX: Duration = Duration::from_secs(15);
/// Give up on re-detection after this long; a session that has not
/// appeared by then is not going to.
pub const DETECT_RETRY_WINDOW: Duration = Duration::from_secs(300);

/// Supervise a daemon-spawned display backend for the lifetime of the
/// process. Exits cleanly when `shutdown_rx` flips to `true` (SIGTERMs
pub async fn run_backend(
    def: DisplayDef,
    socket: PathBuf,
    extra_env: Vec<(String, String)>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    if !matches!(def.spawn, SpawnMode::Daemon) {
        anyhow::bail!(
            "run_backend called on non-daemon backend '{}' (spawn={:?})",
            def.name,
            def.spawn
        );
    }
    if def.bin.as_os_str().is_empty() {
        anyhow::bail!(
            "display backend '{}' has empty bin; nothing to spawn",
            def.name
        );
    }

    let mut delay = RESTART_INITIAL;
    loop {
        if *shutdown_rx.borrow() {
            return Ok(());
        }
        log::info!(
            "starting display backend '{}' -> {} --socket {}",
            def.name,
            def.bin.display(),
            socket.display()
        );

        let mut cmd = Command::new(&def.bin);
        cmd.arg("--socket").arg(&socket);
        for extra in &def.extra_args {
            cmd.arg(extra);
        }
        cmd.envs(extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        // The compositor may not have been up when the daemon started.
        // When nothing provides WAYLAND_DISPLAY, re-scan per attempt so
        // the restart backoff self-heals once the socket appears.
        if std::env::var_os("WAYLAND_DISPLAY").is_none()
            && !extra_env.iter().any(|(k, _)| k == "WAYLAND_DISPLAY")
        {
            if let Some(display) = scan_wayland_socket() {
                log::info!(
                    "display backend '{}': discovered WAYLAND_DISPLAY={display}",
                    def.name
                );
                cmd.env("WAYLAND_DISPLAY", &display);
            }
        }
        cmd.env("WAYWALLEN_SOCKET", &socket);
        cmd.kill_on_drop(true)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        // Linux-only safety net: `kill_on_drop(true)` covers clean exits.
        // PDEATHSIG also handles abrupt parent death.
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                // prctl(PR_SET_PDEATHSIG, SIGTERM, 0, 0, 0)
                let rc = libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGTERM as libc::c_ulong,
                    0,
                    0,
                    0,
                );
                if rc == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                log::error!(
                    "spawn '{}' failed: {e}; backend will not run",
                    def.bin.display()
                );
                // Not recoverable: wrong path, permissions, etc. Don't
                // burn CPU on retries — a real fix needs config change.
                return Err(e.into());
            }
        };
        let pid = child.id();
        log::info!("display backend '{}' pid={:?}", def.name, pid);

        // Reset backoff on a successful spawn; re-apply it if the child
        // dies immediately (caught via the loop below).
        let status = tokio::select! {
            biased;
            _ = wait_shutdown(&mut shutdown_rx) => {
                log::info!("shutdown: stopping display backend '{}' (pid={pid:?})", def.name);
                // Send SIGKILL, then wait up to 2s so we leave no zombie.
                // If the child ignores or is stuck in uninterruptible
                let _ = child.start_kill();
                match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                    Ok(Ok(st)) => log::info!(
                        "display backend '{}' exited after shutdown: {st:?}",
                        def.name
                    ),
                    Ok(Err(e)) => log::warn!(
                        "display backend '{}' wait after shutdown failed: {e}",
                        def.name
                    ),
                    Err(_) => log::warn!(
                        "display backend '{}' did not exit within 2s of shutdown",
                        def.name
                    ),
                }
                return Ok(());
            }
            res = child.wait() => res,
        };

        match status {
            Ok(st) if st.success() => {
                log::info!(
                    "display backend '{}' exited cleanly ({:?}); not restarting",
                    def.name,
                    st.code()
                );
                return Ok(());
            }
            Ok(st) => {
                log::warn!(
                    "display backend '{}' exited {:?}; restarting in {:?}",
                    def.name,
                    st,
                    delay
                );
            }
            Err(e) => {
                log::warn!(
                    "display backend '{}' wait failed: {e}; restarting in {:?}",
                    def.name,
                    delay
                );
            }
        }

        // Race the backoff sleep against shutdown so Ctrl-C exits fast.
        tokio::select! {
            biased;
            _ = wait_shutdown(&mut shutdown_rx) => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
        delay = std::cmp::min(delay * 2, RESTART_MAX);
    }
}

async fn wait_shutdown(rx: &mut watch::Receiver<bool>) {
    // Already true → return immediately. Otherwise park until the flag
    // flips or the sender drops (treat drop as shutdown too).
    if *rx.borrow() {
        return;
    }
    let _ = rx.changed().await;
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::display_registry::{DisplayDef, DisplayRegistry, SpawnMode};
    use std::path::PathBuf;

    fn def(name: &str, de: &[&str], priority: i32, spawn: SpawnMode) -> DisplayDef {
        DisplayDef {
            name: name.to_string(),
            bin: PathBuf::from(format!("/usr/bin/{name}")),
            de: de.iter().map(|s| s.to_string()).collect(),
            priority,
            requires: Vec::new(),
            extra_args: Vec::new(),
            spawn,
        }
    }

    fn registry() -> DisplayRegistry {
        // Return an empty registry; pick_backend will use built-ins.
        DisplayRegistry::new()
    }

    #[test]
    fn kde_picks_builtin_kde() {
        let caps = DeCaps {
            xdg_desktop: vec!["kde".into()],
            ..Default::default()
        };
        let reg = registry();
        match pick_backend(&reg, &caps) {
            PickOutcome::KdeHardMatch(d) => assert_eq!(d.name, "kde-plasma"),
            other => panic!("expected KdeHardMatch, got {:?}", other),
        }
    }

    #[test]
    fn registry_overrides_builtin() {
        let caps = DeCaps {
            xdg_desktop: vec!["niri".into()],
            ..Default::default()
        };
        let mut reg = DisplayRegistry::new();
        // Higher priority than built-in layer-shell (50)
        reg.register(def("layer-shell", &["niri"], 60, SpawnMode::Daemon));

        match pick_backend(&reg, &caps) {
            PickOutcome::Matched(d) => {
                assert_eq!(d.priority, 60);
                assert_eq!(d.bin, PathBuf::from("/usr/bin/layer-shell"));
            }
            other => panic!("expected Matched, got {:?}", other),
        }
    }

    #[test]
    fn wlroots_desktops_pick_layer_shell() {
        for desktop in ["hyprland", "sway", "niri", "cosmic"] {
            let caps = DeCaps {
                xdg_desktop: vec![desktop.into()],
                ..Default::default()
            };
            match pick_backend(&registry(), &caps) {
                PickOutcome::Matched(d) => assert_eq!(d.name, "layer-shell"),
                other => panic!("expected Matched(layer-shell) for {desktop}, got {other:?}"),
            }
        }
    }

    #[test]
    fn should_daemon_spawn_respects_mode() {
        let caps_kde = DeCaps {
            xdg_desktop: vec!["kde".into()],
            ..Default::default()
        };
        let reg = registry();
        assert!(!should_daemon_spawn(&pick_backend(&reg, &caps_kde)));

        let caps_niri = DeCaps {
            xdg_desktop: vec!["niri".into()],
            ..Default::default()
        };
        assert!(should_daemon_spawn(&pick_backend(&reg, &caps_niri)));
    }

    #[test]
    fn preflight_missing_binary_reports_status_and_skips_backend() {
        let caps = DeCaps {
            xdg_desktop: vec!["hyprland".into()],
            ..Default::default()
        };
        let mut backend = def("layer-shell", &["hyprland"], 50, SpawnMode::Daemon);
        backend.bin = PathBuf::from("/__waywallen_missing_layer_shell_binary__");

        let (status, backend) = preflight_daemon_backend(backend, &caps);

        assert!(backend.is_none());
        assert_eq!(status.name, "layer-shell");
        assert_eq!(status.state, DISPLAY_BACKEND_STATE_BINARY_MISSING);
        assert_eq!(status.desktop, "hyprland");
        assert!(status.reason.contains("was not found"));
    }

    #[test]
    fn preflight_flatpak_layer_shell_reports_restricted_and_skips_backend() {
        let caps = DeCaps {
            xdg_desktop: vec!["sway".into()],
            flatpak_id: Some("org.waywallen.waywallen".into()),
            ..Default::default()
        };
        let mut backend = def("layer-shell", &["sway"], 50, SpawnMode::Daemon);
        backend.requires = vec!["wlr-layer-shell".into()];

        let (status, backend) = preflight_daemon_backend(backend, &caps);

        assert!(backend.is_none());
        assert_eq!(status.name, "layer-shell");
        assert_eq!(status.state, DISPLAY_BACKEND_STATE_FLATPAK_RESTRICTED);
        assert_eq!(status.desktop, "sway");
        assert_eq!(status.flatpak_id, "org.waywallen.waywallen");
        assert!(status.reason.contains("Flatpak"));
    }

    #[test]
    fn socket_scan_finds_lowest_socket_and_skips_locks() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::net::UnixListener::bind(dir.path().join("wayland-1")).unwrap();
        std::os::unix::net::UnixListener::bind(dir.path().join("wayland-0")).unwrap();
        std::fs::write(dir.path().join("wayland-0.lock"), b"").unwrap();
        // Plain file with a matching name must not count as a socket.
        std::fs::write(dir.path().join("wayland-"), b"").unwrap();

        assert_eq!(
            scan_wayland_socket_in(dir.path()).as_deref(),
            Some("wayland-0")
        );
    }

    #[test]
    fn socket_scan_empty_dir_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(scan_wayland_socket_in(dir.path()), None);
    }

    fn session(vars: &[(&str, &str)]) -> HashMap<String, String> {
        vars.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn merge_caps_process_env_wins() {
        let base = DeCaps {
            xdg_desktop: vec!["hyprland".into()],
            wayland_display: Some("wayland-1".into()),
            ..Default::default()
        };
        let sess = session(&[
            ("XDG_CURRENT_DESKTOP", "KDE"),
            ("WAYLAND_DISPLAY", "wayland-0"),
        ]);

        let (caps, overrides) = merge_caps(base, &sess, Some("wayland-9".into()));

        assert_eq!(caps.xdg_desktop, vec!["hyprland".to_string()]);
        assert_eq!(caps.wayland_display.as_deref(), Some("wayland-1"));
        assert!(overrides.is_empty());
    }

    #[test]
    fn merge_caps_fills_from_session_env() {
        let sess = session(&[
            ("XDG_CURRENT_DESKTOP", "Hyprland:wlroots"),
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-1"),
        ]);

        let (caps, overrides) = merge_caps(DeCaps::default(), &sess, None);

        assert_eq!(
            caps.xdg_desktop,
            vec!["hyprland".to_string(), "wlroots".to_string()]
        );
        assert!(caps.is_wayland_session);
        assert_eq!(caps.wayland_display.as_deref(), Some("wayland-1"));
        assert_eq!(
            overrides,
            vec![
                (
                    "XDG_CURRENT_DESKTOP".to_string(),
                    "Hyprland:wlroots".to_string()
                ),
                ("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string()),
            ]
        );
    }

    #[test]
    fn merge_caps_falls_back_to_socket_scan() {
        let (caps, overrides) =
            merge_caps(DeCaps::default(), &session(&[]), Some("wayland-0".into()));

        assert_eq!(caps.wayland_display.as_deref(), Some("wayland-0"));
        assert_eq!(
            overrides,
            vec![("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string())]
        );
    }
}
