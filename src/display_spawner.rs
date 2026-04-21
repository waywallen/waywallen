//! Display-backend auto-selection and subprocess supervision.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::watch;

use crate::plugin::display_registry::{DisplayDef, DisplayRegistry, SpawnMode};

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
}

impl DeCaps {
    pub fn is_kde(&self) -> bool {
        self.xdg_desktop.iter().any(|t| t == "kde")
    }
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
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok().filter(|s| !s.is_empty());
    let is_wayland_session = std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false);
    DeCaps {
        xdg_desktop,
        wayland_display,
        is_wayland_session,
        probed_globals: Vec::new(),
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

    // waywallen-display-layer-shell — Wayland layer-shell wallpaper client.
    // We look for the binary in the same directory as the daemon.
    let mut layer_shell_bin = PathBuf::from("waywallen-display-layer-shell");
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("waywallen-display-layer-shell");
            if candidate.exists() {
                layer_shell_bin = candidate;
            }
        }
    }

    defs.push(DisplayDef {
        name: "layer-shell".to_string(),
        bin: layer_shell_bin,
        de: vec!["niri".to_string()],
        priority: 50,
        requires: vec![
            "wlr-layer-shell".to_string(),
            "linux-dmabuf-v4".to_string(),
        ],
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
            if caps.xdg_desktop.iter().any(|t| t.eq_ignore_ascii_case(want)) {
                return true;
            }
        }
        false
    };

    // Soft capability check: only warn on missing `requires`; don't veto
    // until the real wl_registry probe lands. This keeps Hyprland/Sway
    // working today without a Wayland dep in the daemon.
    let mut best: Option<DisplayDef> = None;
    for d in all_defs {
        if !de_matches(&d) {
            continue;
        }
        // Skip Plasma-targeted backends here — the KDE branch above owns
        // those. Prevents `kde-plasma` from leaking into a non-KDE pick.
        if d.de.iter().any(|t| t.eq_ignore_ascii_case("kde"))
            && !d.de.iter().any(|t| t == "*")
        {
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
/// Plasma kpackage), so the daemon stays out of their way.
pub fn should_daemon_spawn(outcome: &PickOutcome) -> bool {
    match outcome {
        PickOutcome::KdeHardMatch(def) | PickOutcome::Matched(def) => {
            matches!(def.spawn, SpawnMode::Daemon)
        }
        PickOutcome::None => false,
    }
}

// ---------------------------------------------------------------------------
// Subprocess supervision
// ---------------------------------------------------------------------------

/// Initial restart delay after a backend exits unexpectedly.
const RESTART_INITIAL: Duration = Duration::from_millis(250);
/// Upper bound on the exponential backoff.
const RESTART_MAX: Duration = Duration::from_secs(10);

/// Supervise a daemon-spawned display backend for the lifetime of the
/// process. Exits cleanly when `shutdown_rx` flips to `true` (SIGTERMs
/// the child via Tokio `kill_on_drop`). Unexpected child exits are
/// retried with exponential backoff; `exit=0` is treated as a graceful
/// completion (rare — usually only on --help or arg errors) and ends
/// the loop.
///
/// Only applicable when `def.spawn == SpawnMode::Daemon`. Callers
/// gate on `should_daemon_spawn` before invoking this.
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
            .stderr(Stdio::inherit());

        // Linux-only safety net: `kill_on_drop(true)` covers the happy
        // path (daemon exits cleanly, Tokio drops the Child handle), but
        // it does NOT cover the daemon getting SIGKILL'd mid-flight —
        // in that case the runtime is torn down without dropping anything
        // and the child becomes reparented to PID 1. PR_SET_PDEATHSIG
        // asks the kernel to SIGTERM the child as soon as its parent
        // thread-group-leader dies, regardless of how it died.
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
                // sleep we still return — the runtime teardown will
                // SIGKILL anything tokio still owns.
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
// ---------------------------------------------------------------------------

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
    fn hyprland_picks_none_if_no_match() {
        // Built-in layer-shell only matches "niri" in my new code (following TOML).
        // Hyprland doesn't match "kde" or "niri".
        let caps = DeCaps {
            xdg_desktop: vec!["hyprland".into()],
            ..Default::default()
        };
        assert!(matches!(pick_backend(&registry(), &caps), PickOutcome::None));
    }

    #[test]
    fn niri_picks_layer_shell() {
        let caps = DeCaps {
            xdg_desktop: vec!["niri".into()],
            ..Default::default()
        };
        match pick_backend(&registry(), &caps) {
            PickOutcome::Matched(d) => assert_eq!(d.name, "layer-shell"),
            other => panic!("expected Matched, got {:?}", other),
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
}
