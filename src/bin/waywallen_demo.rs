//! waywallen_demo — one-shot launcher for the full C++ host → daemon →
//! display pipeline.
//!
//! Usage:
//!   waywallen_demo --scene <pkg> --assets <dir> [--width W] [--height H]
//!                  [--fps N]
//!
//! Required env:
//!   WAYWALLEN_RENDERER_BIN — path to the built C++ waywallen-renderer
//!   host binary. RendererManager picks it up via `new()`.
//!
//! This process does everything a user would otherwise do by hand:
//!   1. Builds a RendererManager and spawns the C++ host with the scene.
//!   2. Serves a display_endpoint on a throwaway Unix socket in TMPDIR.
//!   3. Exec's `waywallen_display_demo --display-sock <path> --renderer-id <id>`
//!      as a child process so a real winit window opens in this same
//!      terminal session.
//!   4. Waits for the display demo to exit, then tears everything down.
//!
//! I6 in plan.md. This is the smallest shippable path for a human to
//! watch a real Wallpaper Engine scene animate through the waywallen
//! pipeline.

use anyhow::{Context, Result};
use waywallen::renderer_manager::{RendererManager, SpawnRequest};
use waywallen::display_endpoint;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

struct Args {
    scene: String,
    assets: String,
    width: u32,
    height: u32,
    fps: u32,
    display_demo_bin: Option<PathBuf>,
}

// When both --scene and --assets are empty, the demo runs in a "no-scene"
// mode that's compatible with the Rust `waywallen_renderer` test producer
// (which draws cycling solid colours via vkCmdClearColorImage and ignores
// the scene/assets args). Useful for verifying the DMA-BUF + display path
// independently of the C++ scene renderer.

fn parse_args() -> Result<Args> {
    let mut scene = None;
    let mut assets = None;
    let mut width = 1280u32;
    let mut height = 720u32;
    let mut fps = 30u32;
    let mut display_demo_bin = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--scene" => scene = it.next(),
            "--assets" => assets = it.next(),
            "--width" => width = it.next().and_then(|s| s.parse().ok()).unwrap_or(1280),
            "--height" => height = it.next().and_then(|s| s.parse().ok()).unwrap_or(720),
            "--fps" => fps = it.next().and_then(|s| s.parse().ok()).unwrap_or(30),
            "--display-demo-bin" => display_demo_bin = it.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!(
                    "usage: waywallen_demo --scene <pkg> --assets <dir> \
                     [--width W] [--height H] [--fps N] [--display-demo-bin PATH]"
                );
                std::process::exit(0);
            }
            _ => {}
        }
    }
    // Either both --scene and --assets are provided (for the C++ scene
    // host), or neither is (test-producer mode). Mixing the two is a
    // user error.
    let scene = scene.unwrap_or_default();
    let assets = assets.unwrap_or_default();
    if scene.is_empty() != assets.is_empty() {
        anyhow::bail!("--scene and --assets must be provided together");
    }
    Ok(Args {
        scene,
        assets,
        width,
        height,
        fps,
        display_demo_bin,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();
    let args = parse_args()?;

    if std::env::var_os("WAYWALLEN_RENDERER_BIN").is_none() {
        anyhow::bail!(
            "WAYWALLEN_RENDERER_BIN must be set to the path of the built \
             C++ waywallen-renderer binary"
        );
    }

    // 1. Spin up manager + display_endpoint on a throwaway socket.
    let mgr = Arc::new(RendererManager::new());
    let display_sock = std::env::temp_dir().join(format!(
        "waywallen-demo-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&display_sock);
    let mgr_for_endpoint = Arc::clone(&mgr);
    let sock_for_endpoint = display_sock.clone();
    let endpoint = tokio::spawn(async move {
        if let Err(e) =
            display_endpoint::serve(&sock_for_endpoint, mgr_for_endpoint).await
        {
            eprintln!("[waywallen_demo] display_endpoint exited: {e:#}");
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 2. Spawn the C++ renderer with the real scene.
    log::info!(
        "spawning C++ renderer: scene={} assets={} {}x{}@{}fps",
        args.scene,
        args.assets,
        args.width,
        args.height,
        args.fps
    );
    let id = mgr
        .spawn(SpawnRequest {
            scene_pkg: args.scene,
            assets: args.assets,
            width: args.width,
            height: args.height,
            fps: args.fps,
            test_pattern: false,
        })
        .await
        .context("RendererManager::spawn")?;
    log::info!("spawned renderer id={id}");

    // 3. Wait for the daemon to cache BindBuffers before exec'ing the
    //    display demo — saves the demo from polling an empty snapshot.
    let handle = mgr
        .get(&id)
        .await
        .context("renderer vanished immediately")?;
    let bind_arc = handle.bind_snapshot();
    let bind_deadline =
        std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if bind_arc.lock().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() > bind_deadline {
            anyhow::bail!("BindBuffers never arrived within 15s");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    log::info!("BindBuffers cached; launching display demo window");

    // 4. Exec the display demo as a subprocess — blocking on its exit is
    //    the "run" phase of the demo.
    let display_demo_bin = args.display_demo_bin.clone().unwrap_or_else(|| {
        // Assume the display demo lives next to the waywallen_demo binary.
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("waywallen_display_demo")))
            .unwrap_or_else(|| PathBuf::from("waywallen_display_demo"))
    });
    log::info!("exec {display_demo_bin:?} --display-sock {display_sock:?} --renderer-id {id}");
    let display_status = tokio::task::spawn_blocking({
        let display_sock = display_sock.clone();
        let id = id.clone();
        move || -> std::io::Result<std::process::ExitStatus> {
            Command::new(&display_demo_bin)
                .arg("--display-sock")
                .arg(&display_sock)
                .arg("--renderer-id")
                .arg(&id)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
        }
    })
    .await
    .context("spawn_blocking display_demo")?
    .context("exec waywallen_display_demo")?;
    log::info!("display demo exited: {display_status}");

    // 5. Teardown.
    let _ = mgr.kill(&id).await;
    endpoint.abort();
    let _ = std::fs::remove_file(&display_sock);
    Ok(())
}
