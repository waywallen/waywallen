//! `com.canonical.dbusmenu` served on `/MenuBar`.
//!
//! Hand-rolled against the canonical XML (GetLayout returns
//! `u(ia{sv}av)` where each `av` element is a `v<(ia{sv}av)>`).

use std::collections::HashMap;
use std::sync::Arc;

use zbus::{interface, zvariant};
use zvariant::{OwnedValue, StructureBuilder, Value};

use crate::control;
use crate::AppState;

const ID_ROOT: i32 = 0;
const ID_OPEN_UI: i32 = 1;
const ID_NEXT: i32 = 2;
const ID_PREV: i32 = 3;
const ID_SEP1: i32 = 4;
const ID_SHUFFLE: i32 = 11;
// Rotate submenu parent + radio leaves. Adding new presets means
// adding an id here and a row in `rotate_options`.
const ID_ROTATE: i32 = 20;
const ID_ROT_OFF: i32 = 21;
const ID_ROT_30S: i32 = 22;
const ID_ROT_1M: i32 = 23;
const ID_ROT_5M: i32 = 24;
const ID_ROT_15M: i32 = 25;
const ID_ROT_1H: i32 = 26;
const ID_SEP_PL: i32 = 12;
const ID_PAUSE: i32 = 5;
const ID_RESUME: i32 = 6;
const ID_SEP2: i32 = 7;
const ID_RESCAN: i32 = 8;
const ID_SEP3: i32 = 9;
const ID_QUIT: i32 = 10;

/// Rotate submenu presets in display order. The leaf id and the
/// matching interval-in-seconds for `set_rotation_interval`.
fn rotate_options() -> &'static [(i32, &'static str, u32)] {
    &[
        (ID_ROT_OFF, "Off", 0),
        (ID_ROT_30S, "30 seconds", 30),
        (ID_ROT_1M, "1 minute", 60),
        (ID_ROT_5M, "5 minutes", 300),
        (ID_ROT_15M, "15 minutes", 900),
        (ID_ROT_1H, "1 hour", 3600),
    ]
}

pub struct DBusMenu {
    app: Arc<AppState>,
}

impl DBusMenu {
    pub fn new(app: Arc<AppState>) -> Self {
        Self { app }
    }
}

type ItemStruct = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

#[interface(name = "com.canonical.dbusmenu")]
impl DBusMenu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    /// Return the whole tree; `parent_id` and `recursion_depth` are
    /// honoured loosely — the menu is tiny so we always serve the root.
    /// Async so we can read live shuffle/rotation state under the
    /// `tokio::Mutex` for checkmark/radio rendering.
    async fn get_layout(
        &self,
        parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, ItemStruct)> {
        let revision = 1;
        let menu = snapshot_menu_state(&self.app).await;
        if parent_id == ID_ROOT {
            Ok((revision, build_root(&menu)))
        } else if parent_id == ID_ROTATE {
            Ok((revision, build_rotate_submenu(&menu)))
        } else {
            let props = props_for(parent_id, &menu)
                .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown id {parent_id}")))?;
            Ok((revision, (parent_id, props, Vec::new())))
        }
    }

    async fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let menu = snapshot_menu_state(&self.app).await;
        ids.into_iter()
            .filter_map(|id| props_for(id, &menu).map(|p| (id, p)))
            .collect()
    }

    async fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let menu = snapshot_menu_state(&self.app).await;
        props_for(id, &menu)
            .and_then(|mut p| p.remove(&name))
            .ok_or_else(|| zbus::fdo::Error::Failed(format!("no such property {name}/{id}")))
    }

    async fn event(
        &self,
        id: i32,
        event_id: String,
        _data: OwnedValue,
        _timestamp: u32,
    ) -> zbus::fdo::Result<()> {
        if event_id != "clicked" {
            return Ok(());
        }
        let app = self.app.clone();
        match id {
            ID_OPEN_UI => {
                if !crate::spawn_ui(&app) {
                    log::warn!("tray: open_ui failed");
                }
            }
            ID_NEXT => {
                if let Err(e) = control::step(&app, 1).await {
                    log::warn!("tray next: {e}");
                }
            }
            ID_PREV => {
                if let Err(e) = control::step(&app, -1).await {
                    log::warn!("tray previous: {e}");
                }
            }
            ID_PAUSE => {
                if let Err(e) = control::pause_all(&app).await {
                    log::warn!("tray pause: {e}");
                }
            }
            ID_RESUME => {
                if let Err(e) = control::resume_all(&app).await {
                    log::warn!("tray resume: {e}");
                }
            }
            ID_RESCAN => {
                if let Err(e) = control::rescan(&app).await {
                    log::warn!("tray rescan: {e}");
                }
            }
            ID_SHUFFLE => {
                let was_on = matches!(
                    app.playlist.lock().await.mode,
                    crate::playlist::Mode::Shuffle
                );
                control::set_shuffle(&app, !was_on).await;
            }
            ID_ROT_OFF | ID_ROT_30S | ID_ROT_1M | ID_ROT_5M | ID_ROT_15M | ID_ROT_1H => {
                if let Some((_, _, secs)) =
                    rotate_options().iter().copied().find(|(rid, _, _)| *rid == id)
                {
                    control::set_rotation_interval(&app, secs).await;
                }
            }
            ID_QUIT => {
                app.shutdown_now();
            }
            _ => {}
        }
        Ok(())
    }

    fn event_group(
        &self,
        events: Vec<(i32, String, OwnedValue, u32)>,
    ) -> zbus::fdo::Result<Vec<i32>> {
        // Acknowledge every event synchronously; actual dispatch happens
        // via `event` for hosts that send them individually. Hosts that
        // only use EventGroup are rare but we still accept the call.
        let app = self.app.clone();
        for (id, kind, _, _) in events.iter() {
            if kind != "clicked" {
                continue;
            }
            let id_copy = *id;
            let app = app.clone();
            tokio::spawn(async move {
                let _ = dispatch_click(&app, id_copy).await;
            });
        }
        Ok(Vec::new())
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(signal)]
    pub async fn items_properties_updated(
        ctxt: &zbus::SignalContext<'_>,
        updated: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn layout_updated(
        ctxt: &zbus::SignalContext<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn item_activation_requested(
        ctxt: &zbus::SignalContext<'_>,
        id: i32,
        timestamp: u32,
    ) -> zbus::Result<()>;
}

// ---------------------------------------------------------------------------
// Live menu state
// ---------------------------------------------------------------------------

/// Snapshot of mutable state we need at menu-render time. Captured
/// synchronously per request so we don't hold any locks across the
/// `OwnedValue` construction below (which can otherwise blow up the
/// future size).
struct MenuState {
    is_shuffle: bool,
    rotation_secs: u32,
}

async fn snapshot_menu_state(app: &Arc<AppState>) -> MenuState {
    let mode = app.playlist.lock().await.mode;
    MenuState {
        is_shuffle: matches!(mode, crate::playlist::Mode::Shuffle),
        rotation_secs: app.rotation.interval(),
    }
}

// `LayoutUpdated` signal emission is intentionally omitted — KDE
// Plasma's tray re-fetches `GetLayout` every time the menu is
// re-opened, so the checkmark / radio state catches up on the next
// click anyway. Wiring the signal would require stashing the zbus
// `Connection` on `AppState`, which is more plumbing than the UX
// improvement merits at this point.

// ---------------------------------------------------------------------------
// Static menu tree
// ---------------------------------------------------------------------------

fn build_root(menu: &MenuState) -> ItemStruct {
    let children: Vec<OwnedValue> = vec![
        item_to_value(make_leaf(ID_OPEN_UI, "Open UI", None)),
        item_to_value(make_leaf(ID_NEXT, "Next", None)),
        item_to_value(make_leaf(ID_PREV, "Previous", None)),
        item_to_value(make_leaf(ID_SEP1, "", Some("separator"))),
        item_to_value(make_checkmark(ID_SHUFFLE, "Shuffle", menu.is_shuffle)),
        item_to_value(make_submenu_parent(ID_ROTATE, "Rotate")),
        item_to_value(make_leaf(ID_SEP_PL, "", Some("separator"))),
        item_to_value(make_leaf(ID_PAUSE, "Pause", None)),
        item_to_value(make_leaf(ID_RESUME, "Resume", None)),
        item_to_value(make_leaf(ID_SEP2, "", Some("separator"))),
        item_to_value(make_leaf(ID_RESCAN, "Rescan wallpapers", None)),
        item_to_value(make_leaf(ID_SEP3, "", Some("separator"))),
        item_to_value(make_leaf(ID_QUIT, "Quit", None)),
    ];
    (ID_ROOT, root_props(), children)
}

fn build_rotate_submenu(menu: &MenuState) -> ItemStruct {
    let children: Vec<OwnedValue> = rotate_options()
        .iter()
        .map(|(id, label, secs)| {
            item_to_value(make_radio(*id, label, menu.rotation_secs == *secs))
        })
        .collect();
    let mut props = HashMap::new();
    props.insert(
        "label".into(),
        OwnedValue::try_from(Value::from("Rotate")).unwrap(),
    );
    props.insert(
        "children-display".into(),
        OwnedValue::try_from(Value::from("submenu")).unwrap(),
    );
    props.insert(
        "enabled".into(),
        OwnedValue::try_from(Value::from(true)).unwrap(),
    );
    props.insert(
        "visible".into(),
        OwnedValue::try_from(Value::from(true)).unwrap(),
    );
    (ID_ROTATE, props, children)
}

fn root_props() -> HashMap<String, OwnedValue> {
    let mut m = HashMap::new();
    m.insert(
        "children-display".into(),
        OwnedValue::try_from(Value::from("submenu")).unwrap(),
    );
    m
}

fn make_leaf(id: i32, label: &str, kind: Option<&str>) -> ItemStruct {
    let mut p = HashMap::new();
    if let Some(k) = kind {
        p.insert("type".into(), OwnedValue::try_from(Value::from(k)).unwrap());
    }
    if !label.is_empty() {
        p.insert(
            "label".into(),
            OwnedValue::try_from(Value::from(label)).unwrap(),
        );
    }
    p.insert(
        "enabled".into(),
        OwnedValue::try_from(Value::from(true)).unwrap(),
    );
    p.insert(
        "visible".into(),
        OwnedValue::try_from(Value::from(true)).unwrap(),
    );
    (id, p, Vec::new())
}

fn make_checkmark(id: i32, label: &str, on: bool) -> ItemStruct {
    let mut item = make_leaf(id, label, None);
    item.1.insert(
        "toggle-type".into(),
        OwnedValue::try_from(Value::from("checkmark")).unwrap(),
    );
    item.1.insert(
        "toggle-state".into(),
        OwnedValue::try_from(Value::from(if on { 1i32 } else { 0i32 })).unwrap(),
    );
    item
}

fn make_radio(id: i32, label: &str, on: bool) -> ItemStruct {
    let mut item = make_leaf(id, label, None);
    item.1.insert(
        "toggle-type".into(),
        OwnedValue::try_from(Value::from("radio")).unwrap(),
    );
    item.1.insert(
        "toggle-state".into(),
        OwnedValue::try_from(Value::from(if on { 1i32 } else { 0i32 })).unwrap(),
    );
    item
}

fn make_submenu_parent(id: i32, label: &str) -> ItemStruct {
    let mut item = make_leaf(id, label, None);
    item.1.insert(
        "children-display".into(),
        OwnedValue::try_from(Value::from("submenu")).unwrap(),
    );
    item
}

fn props_for(id: i32, menu: &MenuState) -> Option<HashMap<String, OwnedValue>> {
    match id {
        ID_ROOT => Some(root_props()),
        ID_OPEN_UI => Some(make_leaf(id, "Open UI", None).1),
        ID_NEXT => Some(make_leaf(id, "Next", None).1),
        ID_PREV => Some(make_leaf(id, "Previous", None).1),
        ID_SEP1 | ID_SEP2 | ID_SEP3 | ID_SEP_PL => {
            Some(make_leaf(id, "", Some("separator")).1)
        }
        ID_SHUFFLE => Some(make_checkmark(id, "Shuffle", menu.is_shuffle).1),
        ID_ROTATE => Some(make_submenu_parent(id, "Rotate").1),
        ID_ROT_OFF | ID_ROT_30S | ID_ROT_1M | ID_ROT_5M | ID_ROT_15M | ID_ROT_1H => {
            let (_, label, secs) = rotate_options()
                .iter()
                .copied()
                .find(|(rid, _, _)| *rid == id)?;
            Some(make_radio(id, label, menu.rotation_secs == secs).1)
        }
        ID_PAUSE => Some(make_leaf(id, "Pause", None).1),
        ID_RESUME => Some(make_leaf(id, "Resume", None).1),
        ID_RESCAN => Some(make_leaf(id, "Rescan wallpapers", None).1),
        ID_QUIT => Some(make_leaf(id, "Quit", None).1),
        _ => None,
    }
}

fn item_to_value(item: ItemStruct) -> OwnedValue {
    let s = StructureBuilder::new()
        .add_field(item.0)
        .add_field(item.1)
        .add_field(item.2)
        .build();
    OwnedValue::try_from(Value::from(s)).unwrap()
}

async fn dispatch_click(app: &Arc<AppState>, id: i32) -> zbus::fdo::Result<()> {
    match id {
        ID_OPEN_UI => {
            let _ = crate::spawn_ui(app);
        }
        ID_NEXT => {
            let _ = control::step(app, 1).await;
        }
        ID_PREV => {
            let _ = control::step(app, -1).await;
        }
        ID_PAUSE => {
            let _ = control::pause_all(app).await;
        }
        ID_RESUME => {
            let _ = control::resume_all(app).await;
        }
        ID_RESCAN => {
            let _ = control::rescan(app).await;
        }
        ID_SHUFFLE => {
            let was_on = matches!(
                app.playlist.lock().await.mode,
                crate::playlist::Mode::Shuffle
            );
            control::set_shuffle(app, !was_on).await;
        }
        ID_ROT_OFF | ID_ROT_30S | ID_ROT_1M | ID_ROT_5M | ID_ROT_15M | ID_ROT_1H => {
            if let Some((_, _, secs)) =
                rotate_options().iter().copied().find(|(rid, _, _)| *rid == id)
            {
                control::set_rotation_interval(app, secs).await;
            }
        }
        ID_QUIT => {
            app.shutdown_now();
        }
        _ => {}
    }
    Ok(())
}
