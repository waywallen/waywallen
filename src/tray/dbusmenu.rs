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
const ID_PAUSE: i32 = 5;
const ID_RESUME: i32 = 6;
const ID_SEP2: i32 = 7;
const ID_RESCAN: i32 = 8;
const ID_SEP3: i32 = 9;
const ID_QUIT: i32 = 10;

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
    fn get_layout(
        &self,
        parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, ItemStruct)> {
        let revision = 1;
        if parent_id == ID_ROOT {
            Ok((revision, build_root()))
        } else {
            // All non-root items in our tree are leaves.
            let props = props_for(parent_id)
                .ok_or_else(|| zbus::fdo::Error::Failed(format!("unknown id {parent_id}")))?;
            Ok((revision, (parent_id, props, Vec::new())))
        }
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        ids.into_iter()
            .filter_map(|id| props_for(id).map(|p| (id, p)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        props_for(id)
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
            ID_OPEN_UI
                if !crate::spawn_ui(&app) => {
                    log::warn!("tray: open_ui failed");
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
// Static menu tree
// ---------------------------------------------------------------------------

fn build_root() -> ItemStruct {
    let children = [
        (ID_OPEN_UI, "Open UI", None),
        (ID_NEXT, "Next", None),
        (ID_PREV, "Previous", None),
        (ID_SEP1, "", Some("separator")),
        (ID_PAUSE, "Pause", None),
        (ID_RESUME, "Resume", None),
        (ID_SEP2, "", Some("separator")),
        (ID_RESCAN, "Rescan wallpapers", None),
        (ID_SEP3, "", Some("separator")),
        (ID_QUIT, "Quit", None),
    ];
    let children_v: Vec<OwnedValue> = children
        .into_iter()
        .map(|(id, label, kind)| item_to_value(make_leaf(id, label, kind)))
        .collect();
    (ID_ROOT, root_props(), children_v)
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

fn props_for(id: i32) -> Option<HashMap<String, OwnedValue>> {
    match id {
        ID_ROOT => Some(root_props()),
        ID_OPEN_UI => Some(make_leaf(id, "Open UI", None).1),
        ID_NEXT => Some(make_leaf(id, "Next", None).1),
        ID_PREV => Some(make_leaf(id, "Previous", None).1),
        ID_SEP1 | ID_SEP2 | ID_SEP3 => Some(make_leaf(id, "", Some("separator")).1),
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
        ID_QUIT => {
            app.shutdown_now();
        }
        _ => {}
    }
    Ok(())
}
