use super::*;

#[test]
fn default_roundtrip() {
    let s: Settings = toml::from_str("").unwrap();
    assert!(s.global.last_wallpaper.is_none());
    assert_eq!(s.global.audio_fade_ms, DEFAULT_AUDIO_FADE_MS);
    assert!(!s.global.mute_when_other_audio);
    assert!(s.global.audio_capture_enabled);
    assert!(!s.global.manual_muted);
    assert!(s.global.plugin_update_notifications);
    assert!(!s.global.duplicate_renderers_for_same_wallpaper);
    assert!(s.global.pointer_forwarding_enabled);
    assert!(!s.global.autostart_enabled);
    assert!(s.plugins.is_empty());
}

#[test]
fn autostart_setting_roundtrip() {
    let src = r#"
[global]
autostart_enabled = true
"#;
    let s: Settings = toml::from_str(src).unwrap();
    assert!(s.global.autostart_enabled);
    assert!(toml::to_string(&s)
        .unwrap()
        .contains("autostart_enabled = true"));
}

#[test]
fn duplicate_renderers_setting_roundtrip() {
    let src = r#"
[global]
duplicate_renderers_for_same_wallpaper = true
"#;
    let s: Settings = toml::from_str(src).unwrap();
    assert!(s.global.duplicate_renderers_for_same_wallpaper);
    assert!(toml::to_string(&s)
        .unwrap()
        .contains("duplicate_renderers_for_same_wallpaper = true"));
}

#[test]
fn auto_attach_playlist_id_roundtrip() {
    let src = r#"
[global]
auto_attach_playlist_id = 42
"#;
    let s: Settings = toml::from_str(src).unwrap();
    assert_eq!(s.global.auto_attach_playlist_id, Some(42));
    assert!(toml::to_string(&s)
        .unwrap()
        .contains("auto_attach_playlist_id = 42"));
}

#[tokio::test]
async fn auto_attach_clears_to_none() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let store = SettingsStore::load_or_default(path).await;
    store.update(|s| {
        s.global.auto_attach_playlist_id = Some(7);
    });
    assert_eq!(store.global().auto_attach_playlist_id, Some(7));
    store.update(|s| {
        s.global.auto_attach_playlist_id = None;
    });
    assert_eq!(store.global().auto_attach_playlist_id, None);
}

#[test]
fn manual_muted_roundtrip() {
    let src = r#"
[global]
manual_muted = true
"#;
    let s: Settings = toml::from_str(src).unwrap();
    assert!(s.global.manual_muted);
    assert!(toml::to_string(&s).unwrap().contains("manual_muted = true"));
}

#[test]
fn audio_capture_setting_roundtrip() {
    let src = r#"
[global]
audio_capture_enabled = false
"#;
    let s: Settings = toml::from_str(src).unwrap();
    assert!(!s.global.audio_capture_enabled);
    assert!(toml::to_string(&s)
        .unwrap()
        .contains("audio_capture_enabled = false"));
}

#[test]
fn audio_fade_ms_is_clamped_for_runtime() {
    let mut g = GlobalSettings::default();
    g.audio_fade_ms = MAX_AUDIO_FADE_MS + 1;
    assert_eq!(g.effective_audio_fade_ms(), MAX_AUDIO_FADE_MS);
}

#[test]
fn layout_defaults_roundtrip() {
    let src = r#"
[global.layout]
fillmode = "preserve_aspect_crop"
align = "top_right"
location = { x = 25, y = 75 }
"#;
    let s: Settings = toml::from_str(src).unwrap();
    assert_eq!(s.global.layout.fillmode, FillMode::PreserveAspectCrop);
    assert_eq!(s.global.layout.align, Align::TopRight);
    assert_eq!(s.global.layout.location, Some(Location::new(25, 75)));
}

#[test]
fn display_override_parses_and_resolves() {
    let src = r#"
[global.layout]
fillmode = "stretched"
align = "center"

[display.HDMI-A-1]
fillmode = "preserve_aspect_fit"
"#;
    let s: Settings = toml::from_str(src).unwrap();
    let prefs = s.displays.get("HDMI-A-1").unwrap();
    assert_eq!(prefs.fillmode, Some(FillMode::PreserveAspectFit));
    assert_eq!(prefs.location, None); // inherits
    assert_eq!(prefs.align, None); // inherits
}

#[tokio::test]
async fn resolved_layout_falls_back_field_by_field() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let store = SettingsStore::load_or_default(path).await;

    // No per-display entry => pure global defaults.
    let r = store.resolved_layout("eDP-1");
    assert_eq!(r.fillmode, FillMode::default());
    assert_eq!(r.location, Location::from_align(Align::default()));

    // Set a partial override for "eDP-1" (only fillmode).
    store.update(|s| {
        s.global.layout.location = Some(Location::new(20, 80));
        s.displays.insert(
            "eDP-1".into(),
            DisplayPrefs {
                fillmode: Some(FillMode::PreserveAspectCrop),
                ..Default::default()
            },
        );
    });

    let r = store.resolved_layout("eDP-1");
    assert_eq!(r.fillmode, FillMode::PreserveAspectCrop); // override
    assert_eq!(r.location, Location::new(20, 80)); // global
}

#[test]
fn display_prefs_is_empty_tracks_last_wallpaper() {
    let mut p = DisplayPrefs::default();
    assert!(p.is_empty());
    p.last_wallpaper = Some("wp-1".into());
    assert!(!p.is_empty());
    p.last_wallpaper = None;
    assert!(p.is_empty());
    p.playlist_auto_attach_disabled = true;
    assert!(!p.is_empty());
}

#[test]
fn canvas_roundtrip_and_layout_resolution() {
    let input = r#"
[global.layout]
fillmode = "stretched"
rotation = "cw90"

[canvas.office]
name = "Office"
last_wallpaper = "42"

[canvas.office.members.display-a.rect]
x = -1920
y = 0
width = 1920
height = 1080

[canvas.office.members.display-b.rect]
x = 0
y = 0
width = 2560
height = 1440

[canvas.office.layout]
fillmode = "preserve_aspect_fit"
"#;
    let settings: Settings = toml::from_str(input).unwrap();
    let rect = settings.canvases["office"].members["display-a"].rect;
    assert_eq!(rect.x, -1920);
    assert_eq!(rect.width, 1920);
    assert_eq!(settings.canvases["office"].members.len(), 2);
    assert_eq!(
        settings.canvases["office"].last_wallpaper.as_deref(),
        Some("42")
    );
    let store = SettingsStore::from_test_settings(settings.clone());
    let resolved = store.resolved_canvas_layout("office", store.resolved_global_layout());
    assert_eq!(resolved.fillmode, FillMode::PreserveAspectFit);
    assert_eq!(resolved.rotation, Rotation::Cw90);

    let encoded = toml::to_string(&settings).unwrap();
    let decoded: Settings = toml::from_str(&encoded).unwrap();
    assert_eq!(decoded, settings);
}

#[test]
fn canvas_mutation_is_canonical_and_rejects_cross_canvas_membership() {
    let store = SettingsStore::from_test_settings(Settings::default());
    let first = store
        .create_canvas(
            CanvasDraft {
                name: "  Office  ".into(),
                members: HashMap::from([
                    (
                        "left".into(),
                        CanvasMemberPrefs {
                            rect: CanvasRect {
                                x: -200,
                                y: 40,
                                width: 100,
                                height: 80,
                            },
                            aspect_locked: true,
                        },
                    ),
                    (
                        "right".into(),
                        CanvasMemberPrefs {
                            rect: CanvasRect {
                                x: 20,
                                y: 0,
                                width: 120,
                                height: 100,
                            },
                            aspect_locked: true,
                        },
                    ),
                ]),
                layout: None,
            },
            &HashMap::new(),
        )
        .unwrap();
    assert_eq!(first.canvas.name, "Office");
    assert_eq!(first.canvas.members["left"].rect.x, 0);
    assert_eq!(first.canvas.members["left"].rect.y, 40);
    assert_eq!(first.canvas.members["right"].rect.x, 220);

    let conflict = store.create_canvas(
        CanvasDraft {
            name: "Mirror".into(),
            members: HashMap::from([(
                "left".into(),
                CanvasMemberPrefs {
                    rect: CanvasRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 80,
                    },
                    aspect_locked: true,
                },
            )]),
            layout: None,
        },
        &HashMap::new(),
    );
    assert!(matches!(
        conflict,
        Err(crate::error::Error::CanvasMemberConflict { display_key, .. })
            if display_key == "left"
    ));

    let stale = store.update_canvas(
        &first.canvas_id,
        first.revision - 1,
        CanvasDraft {
            name: "Stale".into(),
            members: HashMap::new(),
            layout: None,
        },
        &HashMap::new(),
    );
    assert!(matches!(
        stale,
        Err(crate::error::Error::CanvasRevisionConflict { .. })
    ));
}

#[test]
fn canvas_runtime_layout_and_scale_to_reconciliation_do_not_change_topology_revision() {
    let store = SettingsStore::from_test_settings(Settings::default());
    let created = store
        .create_canvas(
            CanvasDraft {
                name: "Office".into(),
                members: HashMap::from([
                    (
                        "left".into(),
                        CanvasMemberPrefs {
                            rect: CanvasRect {
                                x: 0,
                                y: 0,
                                width: 100,
                                height: 80,
                            },
                            aspect_locked: true,
                        },
                    ),
                    (
                        "right".into(),
                        CanvasMemberPrefs {
                            rect: CanvasRect {
                                x: 100,
                                y: 20,
                                width: 120,
                                height: 90,
                            },
                            aspect_locked: true,
                        },
                    ),
                ]),
                layout: None,
            },
            &HashMap::new(),
        )
        .unwrap();
    let revision = created.revision;
    let layout = CanvasLayoutPrefs {
        fillmode: Some(FillMode::PreserveAspectFit),
        location: Some(Location::new(25, 75)),
        rotation: Some(Rotation::Cw90),
    };

    assert!(store
        .set_canvas_layout(&created.canvas_id, Some(layout))
        .unwrap());
    assert!(!store
        .set_canvas_layout(&created.canvas_id, Some(layout))
        .unwrap());
    assert_eq!(store.canvas_revision(), revision);

    assert_eq!(
        store
            .reconcile_canvas_member_runtime_size(
                "right",
                CanvasSize {
                    width: 150,
                    height: 120,
                },
            )
            .unwrap(),
        Some(created.canvas_id.clone())
    );
    assert_eq!(
        store
            .reconcile_canvas_member_runtime_size(
                "right",
                CanvasSize {
                    width: 130,
                    height: 200,
                },
            )
            .unwrap(),
        Some(created.canvas_id.clone())
    );
    assert_eq!(
        store
            .reconcile_canvas_member_runtime_size(
                "right",
                CanvasSize {
                    width: 100,
                    height: 100,
                },
            )
            .unwrap(),
        Some(created.canvas_id.clone())
    );
    assert_eq!(store.canvas_revision(), revision);
    let canvas = store.canvas(&created.canvas_id).unwrap();
    assert_eq!(canvas.members["right"].rect.x, 100);
    assert_eq!(canvas.members["right"].rect.y, 20);
    assert_eq!(canvas.members["right"].rect.width, 150);
    assert_eq!(canvas.members["right"].rect.height, 150);

    let updated = store
        .update_canvas(
            &created.canvas_id,
            revision,
            CanvasDraft {
                name: "Renamed".into(),
                members: canvas.members,
                layout: None,
            },
            &HashMap::new(),
        )
        .unwrap();
    assert_eq!(updated.canvas.layout, Some(layout));
}

#[test]
fn canvas_member_config_is_canonical_and_persistent() {
    let store = SettingsStore::from_test_settings(Settings::default());
    let actual = CanvasSize {
        width: 1920,
        height: 1080,
    };
    let created = store
        .create_canvas(
            CanvasDraft {
                name: "Office".into(),
                members: HashMap::from([(
                    "display".into(),
                    CanvasMemberPrefs {
                        rect: CanvasRect {
                            x: 0,
                            y: 0,
                            width: 3840,
                            height: 2160,
                        },
                        aspect_locked: true,
                    },
                )]),
                layout: None,
            },
            &HashMap::from([("display".to_string(), actual)]),
        )
        .unwrap();

    let resized = store
        .configure_canvas_member(
            &created.canvas_id,
            created.revision,
            "display",
            CanvasMemberConfig {
                size: Some(CanvasMemberSizeChange::Height(1200)),
                aspect_locked: None,
            },
            Some(actual),
        )
        .unwrap();
    assert_eq!(resized.canvas.members["display"].rect.width, 2133);
    assert_eq!(resized.canvas.members["display"].rect.height, 1200);
    assert!(resized.canvas.members["display"].aspect_locked);

    let unlocked = store
        .configure_canvas_member(
            &created.canvas_id,
            resized.revision,
            "display",
            CanvasMemberConfig {
                size: None,
                aspect_locked: Some(false),
            },
            Some(actual),
        )
        .unwrap();
    assert!(!unlocked.canvas.members["display"].aspect_locked);

    let independent = store
        .configure_canvas_member(
            &created.canvas_id,
            unlocked.revision,
            "display",
            CanvasMemberConfig {
                size: Some(CanvasMemberSizeChange::Width(2000)),
                aspect_locked: None,
            },
            Some(actual),
        )
        .unwrap();
    assert_eq!(independent.canvas.members["display"].rect.width, 2000);
    assert_eq!(independent.canvas.members["display"].rect.height, 1200);

    assert_eq!(
        store
            .reconcile_canvas_member_runtime_size(
                "display",
                CanvasSize {
                    width: 1600,
                    height: 900,
                },
            )
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .reconcile_canvas_member_runtime_size(
                "display",
                CanvasSize {
                    width: 2400,
                    height: 1300,
                },
            )
            .unwrap(),
        Some(created.canvas_id.clone())
    );
    let canvas = store.canvas(&created.canvas_id).unwrap();
    assert_eq!(canvas.members["display"].rect.width, 2400);
    assert_eq!(canvas.members["display"].rect.height, 1300);

    let reset = store
        .configure_canvas_member(
            &created.canvas_id,
            independent.revision,
            "display",
            CanvasMemberConfig {
                size: Some(CanvasMemberSizeChange::Reset),
                aspect_locked: None,
            },
            Some(actual),
        )
        .unwrap();
    assert_eq!(reset.canvas.members["display"].rect.size(), actual);
    assert!(!reset.canvas.members["display"].aspect_locked);
}

#[test]
fn canvas_member_aspect_lock_defaults_on_for_old_settings() {
    let member: CanvasMemberPrefs =
        toml::from_str(r#"rect = { x = 0, y = 0, width = 1920, height = 1080 }"#).unwrap();
    assert!(member.aspect_locked);

    let member = CanvasMemberPrefs {
        rect: CanvasRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        aspect_locked: false,
    };
    let encoded = toml::to_string(&member).unwrap();
    let decoded: CanvasMemberPrefs = toml::from_str(&encoded).unwrap();
    assert_eq!(decoded, member);
}

#[test]
fn canvas_scale_to_clamps_live_and_rejects_offline_shrink() {
    let store = SettingsStore::from_test_settings(Settings::default());
    let live_minimums = HashMap::from([(
        "display".to_string(),
        CanvasSize {
            width: 1920,
            height: 1080,
        },
    )]);
    let draft = |width, height| CanvasDraft {
        name: "Office".into(),
        members: HashMap::from([(
            "display".into(),
            CanvasMemberPrefs {
                rect: CanvasRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                aspect_locked: true,
            },
        )]),
        layout: None,
    };

    let clamped = store
        .create_canvas(draft(1919, 1080), &live_minimums)
        .unwrap();
    assert_eq!(clamped.canvas.members["display"].rect.width, 1920);
    assert_eq!(clamped.canvas.members["display"].rect.height, 1080);
    store
        .delete_canvas(&clamped.canvas_id, clamped.revision)
        .unwrap();

    let created = store
        .create_canvas(draft(3840, 2160), &live_minimums)
        .unwrap();
    let shrunk_online = store
        .update_canvas(
            &created.canvas_id,
            created.revision,
            draft(2560, 1440),
            &live_minimums,
        )
        .unwrap();
    assert_eq!(shrunk_online.canvas.members["display"].rect.width, 2560);
    assert_eq!(shrunk_online.canvas.members["display"].rect.height, 1440);

    let shrunk_offline = store.update_canvas(
        &created.canvas_id,
        shrunk_online.revision,
        draft(1920, 1080),
        &HashMap::new(),
    );
    assert!(matches!(
        shrunk_offline,
        Err(crate::error::Error::CanvasInvalid(message))
            if message.contains("smaller than its minimum")
    ));

    let grown_offline = store
        .update_canvas(
            &created.canvas_id,
            shrunk_online.revision,
            draft(4096, 2160),
            &HashMap::new(),
        )
        .unwrap();
    assert_eq!(grown_offline.canvas.members["display"].rect.width, 4096);
    assert_eq!(grown_offline.canvas.members["display"].rect.height, 2160);
}

#[test]
fn empty_canvas_survives_update_until_explicit_delete() {
    let store = SettingsStore::from_test_settings(Settings::default());
    let created = store
        .create_canvas(
            CanvasDraft {
                name: "Empty".into(),
                members: HashMap::new(),
                layout: None,
            },
            &HashMap::new(),
        )
        .unwrap();
    let updated = store
        .update_canvas(
            &created.canvas_id,
            created.revision,
            CanvasDraft {
                name: "Still empty".into(),
                members: HashMap::new(),
                layout: None,
            },
            &HashMap::new(),
        )
        .unwrap();
    assert!(store.canvas(&created.canvas_id).unwrap().members.is_empty());
    store
        .delete_canvas(&created.canvas_id, updated.revision)
        .unwrap();
    assert!(store.canvas(&created.canvas_id).is_none());
}

#[test]
fn auto_replay_default_actions() {
    let policy = AutoReplayPolicy::default();
    assert_eq!(policy.fullscreen, AutoAction::Pause);
    assert_eq!(policy.session_locked, AutoAction::Stop);
    assert_eq!(policy.session_inactive, AutoAction::Stop);
    assert_eq!(policy.resume_delay_ms, DEFAULT_AUTO_REPLAY_RESUME_DELAY_MS);
}

#[test]
fn pause_effect_defaults_to_none_and_clamps_blur_radius() {
    let config = PauseEffectConfig::default();
    assert_eq!(config.kind, PauseEffectKind::None);
    assert_eq!(config.blur.effective_radius(), DEFAULT_BLUR_EFFECT_RADIUS);

    assert_eq!(
        BlurEffectConfig { radius: 0 }.effective_radius(),
        MIN_BLUR_EFFECT_RADIUS
    );
    assert_eq!(
        BlurEffectConfig { radius: 100 }.effective_radius(),
        MAX_BLUR_EFFECT_RADIUS
    );
}

#[test]
fn transition_defaults_to_none_and_clamps() {
    let config = TransitionConfig::default();
    assert_eq!(config.kind, TransitionKind::None);
    assert_eq!(config.effective(), config);

    let clamped = TransitionConfig {
        kind: TransitionKind::Grow,
        duration_ms: 5,
        angle: 725,
        origin: TransitionOrigin { x: 101, y: 400 },
    }
    .effective();
    assert_eq!(clamped.duration_ms, MIN_TRANSITION_DURATION_MS);
    assert_eq!(clamped.angle, 5);
    assert_eq!(clamped.origin, TransitionOrigin { x: 100, y: 100 });
}

#[test]
fn transition_setting_roundtrip() {
    let src = r#"
[global.transition]
kind = "wipe"
duration_ms = 800
angle = 90
"#;
    let s: Settings = toml::from_str(src).unwrap();
    assert_eq!(s.global.transition.kind, TransitionKind::Wipe);
    assert_eq!(s.global.transition.duration_ms, 800);
    assert_eq!(s.global.transition.angle, 90);
    assert_eq!(s.global.transition.origin, TransitionOrigin::default());
    let serialized = toml::to_string(&s).unwrap();
    let reparsed: Settings = toml::from_str(&serialized).unwrap();
    assert_eq!(reparsed.global.transition, s.global.transition);
}

#[tokio::test]
async fn resolved_last_wallpaper_prefers_per_display_then_global() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let store = SettingsStore::load_or_default(path).await;

    // Neither global nor per-display set => None.
    assert_eq!(store.resolved_last_wallpaper("HDMI-A-1"), None);

    // Only global set => returned for any key.
    store.update(|s| s.global.last_wallpaper = Some("wp-global".into()));
    assert_eq!(
        store.resolved_last_wallpaper("HDMI-A-1").as_deref(),
        Some("wp-global"),
    );

    // Per-display override wins; other displays keep falling back.
    store.update(|s| {
        s.displays.insert(
            "HDMI-A-1".into(),
            DisplayPrefs {
                last_wallpaper: Some("wp-a".into()),
                ..Default::default()
            },
        );
    });
    assert_eq!(
        store.resolved_last_wallpaper("HDMI-A-1").as_deref(),
        Some("wp-a"),
    );
    assert_eq!(
        store.resolved_last_wallpaper("DP-2").as_deref(),
        Some("wp-global"),
    );
}

#[tokio::test]
async fn resolved_playlist_prefers_display_and_honors_auto_attach_override() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let store = SettingsStore::load_or_default(path).await;

    store.update(|settings| settings.global.auto_attach_playlist_id = Some(9));
    assert_eq!(store.resolved_playlist_id("HDMI-A-1"), Some(9));

    store.update(|settings| {
        settings.displays.insert(
            "HDMI-A-1".into(),
            DisplayPrefs {
                playlist_auto_attach_disabled: true,
                ..Default::default()
            },
        );
    });
    assert_eq!(store.resolved_playlist_id("HDMI-A-1"), None);
    assert_eq!(store.resolved_playlist_id("DP-2"), Some(9));
    let encoded = toml::to_string(&store.snapshot()).unwrap();
    let decoded: Settings = toml::from_str(&encoded).unwrap();
    assert!(decoded.displays["HDMI-A-1"].playlist_auto_attach_disabled);

    store.update(|settings| {
        settings
            .displays
            .get_mut("HDMI-A-1")
            .unwrap()
            .active_playlist_id = Some(3);
    });
    assert_eq!(store.resolved_playlist_id("HDMI-A-1"), Some(3));
}

#[test]
fn plugin_section_preserved() {
    let src = r#"
[plugin.wescene]
foo = "bar"
baz = "7"
"#;
    let s: Settings = toml::from_str(src).unwrap();
    let wescene = s.plugins.get("wescene").expect("wescene section");
    assert_eq!(wescene.get("foo").map(String::as_str), Some("bar"));
    assert_eq!(wescene.get("baz").map(String::as_str), Some("7"));
}

#[tokio::test]
async fn debounced_write_roundtrips() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let store = SettingsStore::load_or_default(path.clone()).await;
    assert_eq!(store.global().rotation_secs, 0);

    store.update(|s| s.global.rotation_secs = 30);
    // Wait past the debounce window.
    tokio::time::sleep(DEBOUNCE_WRITE + Duration::from_millis(500)).await;

    let written = tokio::fs::read_to_string(&path).await.unwrap();
    let parsed: Settings = toml::from_str(&written).unwrap();
    assert_eq!(parsed.global.rotation_secs, 30);
}

// --- reconcile() tests --------------------------------------------

use crate::plugin::renderer_registry::{RendererDef, RendererRegistry, SettingDef, SettingType};
use std::path::PathBuf;

fn schema_setting(ty: SettingType, default: toml::Value, identity: bool) -> SettingDef {
    SettingDef {
        ty,
        default,
        identity,
        label_key: None,
        description_key: None,
        label: None,
        description: None,
        group_label: None,
        min: None,
        max: None,
        step: None,
        choices: None,
        group: None,
        order: None,
    }
}

fn registry_with_video() -> RendererRegistry {
    let mut r = RendererRegistry::new();
    let mut s: HashMap<String, SettingDef> = HashMap::new();
    s.insert(
        "loop_file".into(),
        schema_setting(
            SettingType::String,
            toml::Value::String("inf".into()),
            false,
        ),
    );
    s.insert(
        "volume".into(),
        SettingDef {
            min: Some(toml::Value::Integer(0)),
            max: Some(toml::Value::Integer(100)),
            ..schema_setting(SettingType::U32, toml::Value::Integer(100), false)
        },
    );
    s.insert(
        RENDERER_ENABLE_AUDIO_KEY.into(),
        schema_setting(SettingType::Bool, toml::Value::Boolean(true), false),
    );
    r.register(RendererDef {
        name: "waywallen-video".into(),
        plugin_id: "test.plugin".to_string(),
        plugin_version: "0.0.0".to_string(),
        plugin_system: false,
        bin: PathBuf::from("/dev/null"),
        types: vec!["video".into()],
        priority: 100,
        activity: crate::plugin::renderer_registry::RendererActivityMode::Continuous,
        spawn_version: Some(1),
        extras: Vec::new(),
        settings: s,
        legacy_events: None,
    });
    r
}

fn make_store_with(plugins: HashMap<String, HashMap<String, String>>) -> Arc<SettingsStore> {
    SettingsStore::from_test_settings(Settings {
        global: GlobalSettings::default(),
        plugins,
        displays: HashMap::new(),
        canvases: HashMap::new(),
    })
}

#[test]
fn reconcile_fills_missing_defaults() {
    let store = make_store_with(HashMap::new());
    let changed = store.reconcile(&registry_with_video());
    assert!(changed, "expected reconcile to fill defaults");
    let snap = store.snapshot();
    let video = snap.plugins.get("waywallen-video").expect("video table");
    assert_eq!(video.get("loop_file").map(String::as_str), Some("inf"));
    assert_eq!(video.get("volume").map(String::as_str), Some("100"));
    assert_eq!(
        video.get(RENDERER_ENABLE_AUDIO_KEY).map(String::as_str),
        Some("true")
    );
}

#[test]
fn reconcile_drops_unknown_keys() {
    let mut plugins = HashMap::new();
    let mut video = HashMap::new();
    video.insert("loop_file".into(), "inf".into());
    video.insert("volume".into(), "50".into());
    video.insert("ghost".into(), "should-disappear".into());
    plugins.insert("waywallen-video".into(), video);

    let store = make_store_with(plugins);
    let changed = store.reconcile(&registry_with_video());
    assert!(changed);
    let snap = store.snapshot();
    let video = snap.plugins.get("waywallen-video").unwrap();
    assert!(!video.contains_key("ghost"), "unknown key must be dropped");
    assert_eq!(video.get("volume").map(String::as_str), Some("50"));
}

#[test]
fn reconcile_resets_out_of_range_to_default() {
    let mut plugins = HashMap::new();
    let mut video = HashMap::new();
    video.insert("loop_file".into(), "inf".into());
    video.insert("volume".into(), "999".into());
    plugins.insert("waywallen-video".into(), video);

    let store = make_store_with(plugins);
    let changed = store.reconcile(&registry_with_video());
    assert!(changed);
    let snap = store.snapshot();
    let video = snap.plugins.get("waywallen-video").unwrap();
    assert_eq!(video.get("volume").map(String::as_str), Some("100"));
}

#[test]
fn reconcile_no_change_returns_false() {
    let mut plugins = HashMap::new();
    let mut video = HashMap::new();
    video.insert("loop_file".into(), "inf".into());
    video.insert("volume".into(), "100".into());
    video.insert(RENDERER_ENABLE_AUDIO_KEY.into(), "true".into());
    plugins.insert("waywallen-video".into(), video);

    let store = make_store_with(plugins);
    let changed = store.reconcile(&registry_with_video());
    assert!(!changed, "all keys present and valid → no change");
}

#[test]
fn reconcile_keeps_unknown_plugin_section() {
    // A plugin we don't know about should stay untouched (might
    // be a renamed/missing manifest the user'll re-add).
    let mut plugins = HashMap::new();
    let mut wescene = HashMap::new();
    wescene.insert("foo".into(), "bar".into());
    plugins.insert("waywallen-wescene".into(), wescene);

    let store = make_store_with(plugins);
    store.reconcile(&registry_with_video());
    let snap = store.snapshot();
    assert!(snap.plugins.contains_key("waywallen-wescene"));
    assert_eq!(
        snap.plugins
            .get("waywallen-wescene")
            .and_then(|m| m.get("foo"))
            .map(String::as_str),
        Some("bar")
    );
}

#[test]
fn renderer_audio_settings_compose_without_mutating_plugin_values() {
    let registry = registry_with_video();
    let renderer = registry.resolve_by_name("waywallen-video").unwrap();
    let mut settings = Settings::default();
    settings.plugins.insert(
        renderer.name.clone(),
        HashMap::from([
            (RENDERER_ENABLE_AUDIO_KEY.to_string(), "true".to_string()),
            (RENDERER_VOLUME_KEY.to_string(), "75".to_string()),
        ]),
    );

    assert_eq!(
        settings
            .resolved_renderer_settings(renderer)
            .get(RENDERER_ENABLE_AUDIO_KEY)
            .map(String::as_str),
        Some("true")
    );

    settings.global.renderer.enable_audio = false;
    settings.global.renderer.volume = 50;
    let effective = settings.resolved_renderer_settings(renderer);
    assert_eq!(
        effective.get(RENDERER_ENABLE_AUDIO_KEY).map(String::as_str),
        Some("false")
    );
    assert_eq!(
        effective.get(RENDERER_VOLUME_KEY).map(String::as_str),
        Some("38")
    );

    settings.global.renderer.enable_audio = true;
    settings.global.renderer.volume = MAX_RENDERER_VOLUME;
    let restored = settings.resolved_renderer_settings(renderer);
    assert_eq!(
        restored.get(RENDERER_ENABLE_AUDIO_KEY).map(String::as_str),
        Some("true")
    );
    assert_eq!(
        restored.get(RENDERER_VOLUME_KEY).map(String::as_str),
        Some("75")
    );
    assert_eq!(
        settings.plugins[&renderer.name][RENDERER_ENABLE_AUDIO_KEY],
        "true"
    );
    assert_eq!(settings.plugins[&renderer.name][RENDERER_VOLUME_KEY], "75");
}

#[test]
fn renderer_global_settings_roundtrip_toml() {
    let settings: Settings = toml::from_str(
        r#"
[global.renderer]
enable_audio = false
volume = 35
"#,
    )
    .unwrap();
    assert!(!settings.global.renderer.enable_audio);
    assert_eq!(settings.global.renderer.volume, 35);

    let encoded = toml::to_string(&settings).unwrap();
    let decoded: Settings = toml::from_str(&encoded).unwrap();
    assert_eq!(decoded.global.renderer, settings.global.renderer);

    let legacy: Settings = toml::from_str("[global]\nrotation_secs = 10\n").unwrap();
    assert_eq!(legacy.global.renderer, GlobalRendererSettings::default());
}

#[test]
fn persisted_catalog_query_round_trips_through_domain_types() {
    let state = WallpaperFilterState {
        filters: vec![WallpaperFilterRuleState {
            r#type: 9,
            group: 3,
            tag_filter: Some(WallpaperTagFilterState {
                values: vec!["Nature".into(), "Video".into()],
                condition: 2,
            }),
            ..Default::default()
        }],
        filter_logics: vec![FilterLogicState {
            op: 2,
            group_a: 1,
            group_b: 3,
        }],
    };
    let (filters, logics) = state.to_catalog();
    assert_eq!(WallpaperFilterState::from_catalog(&filters, &logics), state);

    let sorts = vec![WallpaperSortRuleState {
        key: 4,
        direction: 2,
    }];
    let domain = WallpaperSortRuleState::vec_to_catalog(&sorts);
    assert_eq!(WallpaperSortRuleState::vec_from_catalog(&domain), sorts);
}
