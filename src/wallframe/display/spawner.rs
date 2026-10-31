use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::watch;

use crate::plugin::display_registry::{DisplayDef, DisplayRegistry, SpawnMode};

const CHILD_STDERR_REASON_BYTES: usize = 512;

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

fn desktop_tokens(current_desktop: Option<&str>, wayfire_socket: Option<&OsStr>) -> Vec<String> {
    let mut tokens = current_desktop
        .map(|s| {
            s.split(':')
                .filter(|p| !p.is_empty())
                .map(|p| p.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if tokens.iter().all(|token| token == "wayland")
        && wayfire_socket.is_some_and(|socket| !socket.is_empty())
    {
        tokens.push("wayfire".to_string());
    }

    tokens
}

/// Read environment to populate `DeCaps`. Never panics; unset values are
/// left at their defaults.
pub fn detect_de() -> DeCaps {
    let current_desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
    let wayfire_socket = std::env::var_os("WAYFIRE_SOCKET");
    let xdg_desktop = desktop_tokens(current_desktop.as_deref(), wayfire_socket.as_deref());
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

/// Select a backend from the registry for the current environment.
/// See module docs for rules.
pub fn pick_backend(reg: &DisplayRegistry, caps: &DeCaps) -> PickOutcome {
    // Hard rule: KDE sessions use their dedicated backend (usually
    // spawn=external) and never fall back.
    if caps.is_kde() {
        if let Some(def) = reg
            .all()
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
    for d in reg.all() {
        if !de_matches(&d) {
            continue;
        }
        // Skip Plasma-targeted backends here — the KDE branch above owns
        // those. Prevents `kde-plasma` from leaking into a non-KDE pick.
        if d.de.iter().any(|t| t.eq_ignore_ascii_case("kde")) && !d.de.iter().any(|t| t == "*") {
            continue;
        }
        // MATE, Cinnamon and Xfce keep their XDG_CURRENT_DESKTOP on Wayland.
        if requires_x11(d) && caps.is_wayland_session {
            log::debug!("display backend {} skipped: Wayland session", d.name);
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
            None => best = Some(d.clone()),
            Some(ref cur) if d.priority > cur.priority => best = Some(d.clone()),
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
                    target: "waywallen::display",
                    "no display backend detected for KDE; install waywallen-kde or configure a manifest"
                );
            } else {
                log::warn!(
                    target: "waywallen::display",
                    "no display backend / monitors path matched xdg_desktop={:?}; daemon will run in pure external-consumer mode",
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

fn requires_x11(def: &DisplayDef) -> bool {
    def.requires.iter().any(|r| r == "x11")
}

// ---------------------------------------------------------------------------
// Subprocess supervision

/// Initial restart delay after a backend exits unexpectedly. Kept
/// generous so a crashing backend (e.g. a protocol error from a bad
const RESTART_INITIAL: Duration = Duration::from_secs(2);
/// Upper bound on the exponential backoff.
const RESTART_MAX: Duration = Duration::from_secs(10);

/// Supervise a daemon-spawned display backend for the lifetime of the
/// process. Exits cleanly when `shutdown_rx` flips to `true` (SIGTERMs
pub async fn run_backend(
    def: DisplayDef,
    socket: PathBuf,
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
        cmd.env("WAYWALLEN_SOCKET", &socket);
        cmd.kill_on_drop(true)
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped());

        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
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
                    "display backend spawn failed: backend={} spawn={}: {e}; backend will not run",
                    def.name,
                    def.bin.display()
                );
                // Not recoverable: wrong path, permissions, etc. Don't
                // burn CPU on retries — a real fix needs config change.
                return Err(e.into());
            }
        };
        let stderr = child.stderr.take().map(|child_stderr| {
            crate::wallframe::process_stdio::ChildStderrCapture::spawn(
                child_stderr,
                format!("display-{}", def.name),
            )
        });
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
                if let Some(stderr) = &stderr {
                    let _ = stderr.drain(Duration::from_millis(250)).await;
                }
                return Ok(());
            }
            res = child.wait() => res,
        };

        let stderr = match &stderr {
            Some(stderr) => stderr
                .drain(Duration::from_millis(250))
                .await
                .last_line_limited(CHILD_STDERR_REASON_BYTES),
            None => None,
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
                if let Some(stderr) = &stderr {
                    log::warn!(
                        "display backend '{}' exited {:?}: {stderr}; restarting in {:?}",
                        def.name,
                        st,
                        delay
                    );
                } else {
                    log::warn!(
                        "display backend '{}' exited {:?}; restarting in {:?}",
                        def.name,
                        st,
                        delay
                    );
                }
            }
            Err(e) => {
                if let Some(stderr) = &stderr {
                    log::warn!(
                        "display backend '{}' wait failed: {e}: {stderr}; restarting in {:?}",
                        def.name,
                        delay
                    );
                } else {
                    log::warn!(
                        "display backend '{}' wait failed: {e}; restarting in {:?}",
                        def.name,
                        delay
                    );
                }
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
        DisplayRegistry::with_builtins()
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
        let mut reg = DisplayRegistry::with_builtins();
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
        for desktop in ["hyprland", "sway", "niri", "cosmic", "wayfire"] {
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
    fn x11_desktops_pick_x11_client_outside_wayland() {
        for desktop in ["mate", "x-cinnamon", "xfce"] {
            let mut caps = DeCaps {
                xdg_desktop: vec![desktop.into()],
                ..Default::default()
            };
            let outcome = pick_backend(&registry(), &caps);
            match &outcome {
                PickOutcome::Matched(d) => assert_eq!(d.name, "x11"),
                other => panic!("expected Matched(x11) for {desktop}, got {other:?}"),
            }
            assert!(should_daemon_spawn(&outcome));

            caps.is_wayland_session = true;
            assert!(matches!(
                pick_backend(&registry(), &caps),
                PickOutcome::None
            ));
        }
    }

    #[test]
    fn wayfire_socket_adds_wayfire_desktop_hint() {
        let socket = Some(OsStr::new("/run/user/1000/wayfire.sock"));
        assert_eq!(
            desktop_tokens(Some("wayland"), socket),
            vec!["wayland".to_string(), "wayfire".to_string()]
        );
        for desktop in [None, Some(""), Some(":")] {
            let caps = DeCaps {
                xdg_desktop: desktop_tokens(desktop, socket),
                ..Default::default()
            };
            match pick_backend(&registry(), &caps) {
                PickOutcome::Matched(d) => assert_eq!(d.name, "layer-shell"),
                other => panic!("expected Matched(layer-shell), got {other:?}"),
            }
        }
    }

    #[test]
    fn empty_or_unset_wayfire_socket_does_not_add_desktop_hint() {
        for socket in [None, Some(OsStr::new(""))] {
            assert!(desktop_tokens(None, socket).is_empty());
            assert_eq!(desktop_tokens(Some("wayland"), socket), ["wayland"]);
            assert_eq!(desktop_tokens(Some("Wayfire"), socket), ["wayfire"]);
        }
    }

    #[test]
    fn wayfire_socket_preserves_explicit_desktop_tokens() {
        let socket = Some(OsStr::new("/run/user/1000/wayfire.sock"));
        assert_eq!(desktop_tokens(Some("Wayfire"), socket), ["wayfire"]);
        for desktop in ["GNOME", "KDE", "sway", "wayland:GNOME"] {
            assert_eq!(
                desktop_tokens(Some(desktop), socket),
                desktop_tokens(Some(desktop), None)
            );
        }
    }

    #[test]
    fn gnome_picks_external_shell_backend() {
        let caps = DeCaps {
            xdg_desktop: vec!["gnome".into()],
            ..Default::default()
        };

        let outcome = pick_backend(&registry(), &caps);

        match &outcome {
            PickOutcome::Matched(def) => {
                assert_eq!(def.name, "gnome-shell");
                assert_eq!(def.spawn, SpawnMode::External);
            }
            other => panic!("expected Matched(gnome-shell), got {other:?}"),
        }
        assert!(!should_daemon_spawn(&outcome));
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
}
