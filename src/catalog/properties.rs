use std::collections::HashMap;

use crate::settings::ResolvedLayout;
use crate::wallframe::display::layout::{FillMode, Location, Rotation};
use serde::{Deserialize, Serialize};

const SCHEME_COLOR_KEY: &str = "waywallen.scheme_color";
const ENABLE_AUDIO_KEY: &str = "waywallen.enable_audio";
const PLAYBACK_SPEED_KEY: &str = "waywallen.playback_speed";
const MOUSE_PARALLAX_KEY: &str = "waywallen.mouse_parallax";
const FILL_MODE_KEY: &str = "waywallen.fill_mode";
const ROTATION_KEY: &str = "waywallen.rotation";
const LOCATION_X_KEY: &str = "waywallen.location_x";
const LOCATION_Y_KEY: &str = "waywallen.location_y";

const LEGACY_SCHEME_COLOR_KEY: &str = "schemecolor";

const DAEMON_LAYOUT_SCHEMA_KEYS: &[&str] =
    &[FILL_MODE_KEY, ROTATION_KEY, LOCATION_X_KEY, LOCATION_Y_KEY];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WallpaperLayoutOverride {
    pub fillmode: Option<FillMode>,
    pub location: Option<Location>,
    pub rotation: Option<Rotation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedWallpaperLayoutOverride {
    fillmode: FillMode,
    location: Location,
    rotation: Rotation,
}

impl WallpaperLayoutOverride {
    pub fn from_resolved(layout: ResolvedLayout) -> Self {
        Self {
            fillmode: Some(layout.fillmode),
            location: Some(layout.location),
            rotation: Some(layout.rotation),
        }
    }

    pub fn materialize(self) -> ResolvedLayout {
        self.apply_to(ResolvedLayout {
            fillmode: FillMode::default(),
            location: Location::default(),
            rotation: Rotation::default(),
        })
    }

    pub fn apply_to(self, base: ResolvedLayout) -> ResolvedLayout {
        ResolvedLayout {
            fillmode: self.fillmode.unwrap_or(base.fillmode),
            location: self.location.unwrap_or(base.location),
            rotation: self.rotation.unwrap_or(base.rotation),
        }
    }

    pub fn is_empty(self) -> bool {
        self.fillmode.is_none() && self.location.is_none() && self.rotation.is_none()
    }
}

pub fn wallpaper_layout_override_from_json(raw: &str) -> Option<WallpaperLayoutOverride> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(persisted) = serde_json::from_str::<PersistedWallpaperLayoutOverride>(raw) {
        return Some(WallpaperLayoutOverride::from_resolved(ResolvedLayout {
            fillmode: persisted.fillmode,
            location: persisted.location,
            rotation: persisted.rotation,
        }));
    }
    let parsed = serde_json::from_str::<WallpaperLayoutOverride>(raw).ok()?;
    (!parsed.is_empty()).then_some(parsed)
}

pub fn wallpaper_layout_override_to_json(
    layout: ResolvedLayout,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&PersistedWallpaperLayoutOverride {
        fillmode: layout.fillmode,
        location: layout.location,
        rotation: layout.rotation,
    })
}

pub fn is_daemon_display_property_key(key: &str) -> bool {
    matches!(
        key,
        FILL_MODE_KEY | ROTATION_KEY | LOCATION_X_KEY | LOCATION_Y_KEY
    )
}

pub fn is_daemon_predefined_property_key(key: &str) -> bool {
    matches!(
        canonical_user_property_key(key),
        SCHEME_COLOR_KEY | ENABLE_AUDIO_KEY | PLAYBACK_SPEED_KEY | MOUSE_PARALLAX_KEY
    )
}

pub fn canonical_user_property_key(key: &str) -> &str {
    match key {
        LEGACY_SCHEME_COLOR_KEY => SCHEME_COLOR_KEY,
        _ => key,
    }
}

pub fn dedupe_predefined_schema(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };
    let Some(map) = value.as_object_mut() else {
        return raw.to_string();
    };
    let mut remapped = serde_json::Map::new();
    let old = std::mem::take(map);
    for (key, value) in old {
        let canonical = canonical_user_property_key(&key);
        if key == canonical || !remapped.contains_key(canonical) {
            remapped.insert(canonical.to_string(), value);
        }
    }
    remapped.retain(|key, _| {
        is_daemon_predefined_property_key(key) || !DAEMON_LAYOUT_SCHEMA_KEYS.contains(&key.as_str())
    });
    *map = remapped;
    if map.is_empty() {
        String::new()
    } else {
        serde_json::to_string(map).unwrap_or_else(|_| raw.to_string())
    }
}

pub fn user_property_default_wire_value(raw_schema: &str, key: &str) -> Option<String> {
    let raw_schema = raw_schema.trim();
    if raw_schema.is_empty() {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(raw_schema).ok()?;
    let map = value.as_object()?;
    let canonical = canonical_user_property_key(key);
    let prop = map.get(canonical).or_else(|| {
        map.iter()
            .find(|(k, _)| canonical_user_property_key(k) == canonical)
            .map(|(_, v)| v)
    })?;
    let prop = prop.as_object()?;
    let ty = prop
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let default = prop.get("value")?;
    coerce_default_wire_value(default, &ty)
}

fn coerce_default_wire_value(value: &serde_json::Value, ty: &str) -> Option<String> {
    match ty {
        "color" => match value {
            serde_json::Value::Array(values) => values
                .iter()
                .map(|v| v.as_f64().map(|n| format!("{n:.4}")))
                .collect::<Option<Vec<_>>>()
                .map(|v| v.join(" ")),
            serde_json::Value::String(value) => Some(value.clone()),
            _ => json_value_to_wire_string(value),
        },
        "bool" => value
            .as_bool()
            .map(|v| if v { "true" } else { "false" }.to_string())
            .or_else(|| json_value_to_wire_string(value)),
        "slider" => json_value_to_wire_string(value),
        "combo" => json_value_to_wire_string(value),
        _ => json_value_to_wire_string(value),
    }
}

fn json_value_to_wire_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Bool(value) => Some(if *value { "true" } else { "false" }.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|v| v.as_f64().map(|n| format!("{n:.4}")))
            .collect::<Option<Vec<_>>>()
            .map(|v| v.join(" ")),
        _ => None,
    }
}

pub fn normalize_user_property_overrides(map: HashMap<String, String>) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(map.len());
    for (key, value) in map {
        let canonical = canonical_user_property_key(&key);
        if key == canonical || !out.contains_key(canonical) {
            out.insert(canonical.to_string(), value);
        }
    }
    out
}

pub fn normalize_renderer_user_properties(map: HashMap<String, String>) -> HashMap<String, String> {
    let mut out = normalize_user_property_overrides(map);
    out.retain(|key, _| !is_daemon_display_property_key(key));
    out
}

pub fn merge_renderer_user_properties(
    defaults: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
) -> String {
    let mut merged = normalize_renderer_user_properties(defaults.clone());
    merged.extend(normalize_renderer_user_properties(overrides.clone()));
    if merged.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&merged).unwrap_or_default()
    }
}

pub fn normalize_user_property_overrides_json(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };
    let Some(map) = value.as_object_mut() else {
        return raw.to_string();
    };

    let mut remapped = serde_json::Map::new();
    let old = std::mem::take(map);
    for (key, value) in old {
        let canonical = canonical_user_property_key(&key);
        if key == canonical || !remapped.contains_key(canonical) {
            remapped.insert(canonical.to_string(), value);
        }
    }
    *map = remapped;
    serde_json::to_string(&value).unwrap_or_else(|_| raw.to_string())
}

pub fn split_renderer_properties(
    map: HashMap<String, String>,
) -> (HashMap<String, String>, WallpaperLayoutOverride) {
    let mut renderer = HashMap::new();
    let mut fillmode = None;
    let mut rotation = None;
    let mut location_x = None;
    let mut location_y = None;

    for (key, value) in map {
        match key.as_str() {
            FILL_MODE_KEY => fillmode = parse_fillmode(&value),
            ROTATION_KEY => rotation = parse_rotation(&value),
            LOCATION_X_KEY => location_x = parse_percent(&value),
            LOCATION_Y_KEY => location_y = parse_percent(&value),
            _ => {
                renderer.insert(key, value);
            }
        }
    }

    let location = if location_x.is_some() || location_y.is_some() {
        Some(Location::new(
            location_x.unwrap_or(50),
            location_y.unwrap_or(50),
        ))
    } else {
        None
    };
    let layout = WallpaperLayoutOverride {
        fillmode,
        location,
        rotation,
    };
    (renderer, layout)
}

fn parse_fillmode(value: &str) -> Option<FillMode> {
    match value {
        "stretched" => Some(FillMode::Stretched),
        "preserve_aspect_fit" => Some(FillMode::PreserveAspectFit),
        "preserve_aspect_crop" => Some(FillMode::PreserveAspectCrop),
        "centered" => Some(FillMode::Centered),
        _ => None,
    }
}

fn parse_rotation(value: &str) -> Option<Rotation> {
    match value {
        "normal" => Some(Rotation::Normal),
        "cw_90" => Some(Rotation::Cw90),
        "cw_180" => Some(Rotation::Cw180),
        "cw_270" => Some(Rotation::Cw270),
        _ => None,
    }
}

fn parse_percent(value: &str) -> Option<u8> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let parsed = value.parse::<f32>().ok()?;
    if !parsed.is_finite() {
        return None;
    }
    Some(parsed.round().clamp(0.0, 100.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_daemon_display_properties_from_renderer_properties() {
        let raw = r#"{
            "waywallen.fill_mode": "centered",
            "waywallen.location_x": "25",
            "waywallen.location_y": "75",
            "speed": "2"
        }"#;
        let properties = serde_json::from_str(raw).unwrap();
        let (renderer, layout) = split_renderer_properties(properties);
        assert_eq!(layout.fillmode, Some(FillMode::Centered));
        assert_eq!(layout.location, Some(Location::new(25, 75)));
        assert_eq!(renderer.get("speed").map(String::as_str), Some("2"));
    }

    #[test]
    fn keeps_scheme_color_default_when_filtering_schema() {
        let raw = r#"{
            "waywallen.scheme_color": { "type": "color", "value": [0.1, 0.2, 0.3] },
            "waywallen.enable_audio": { "type": "bool", "value": true },
            "waywallen.playback_speed": { "type": "slider", "value": 100 },
            "ui_browse_properties_scheme_color": { "type": "color" },
            "schemecolor": { "type": "color", "value": [0.9, 0.8, 0.7] },
            "waywallen.fill_mode": { "type": "combo" },
            "speed": { "type": "slider" }
        }"#;
        let filtered = dedupe_predefined_schema(raw);
        let value: serde_json::Value = serde_json::from_str(&filtered).unwrap();
        let obj = value.as_object().unwrap();
        assert!(obj.contains_key("waywallen.scheme_color"));
        assert!(obj.contains_key("waywallen.enable_audio"));
        assert!(obj.contains_key("waywallen.playback_speed"));
        assert!(!obj.contains_key("schemecolor"));
        assert!(!obj.contains_key("waywallen.fill_mode"));
        assert!(obj.contains_key("ui_browse_properties_scheme_color"));
        assert!(obj.contains_key("speed"));
        assert_eq!(
            obj.get("waywallen.scheme_color")
                .and_then(|v| v.as_object())
                .and_then(|v| v.get("value")),
            Some(&serde_json::json!([0.1, 0.2, 0.3]))
        );
    }

    #[test]
    fn classifies_enable_audio_as_predefined_renderer_property() {
        assert!(is_daemon_predefined_property_key("waywallen.enable_audio"));
        assert!(is_daemon_predefined_property_key(
            "waywallen.playback_speed"
        ));
        assert!(is_daemon_predefined_property_key(
            "waywallen.mouse_parallax"
        ));
        assert!(!is_daemon_display_property_key("waywallen.enable_audio"));
        assert!(!is_daemon_display_property_key("waywallen.playback_speed"));
        assert!(!is_daemon_display_property_key("waywallen.mouse_parallax"));

        let raw = r#"{
            "waywallen.enable_audio": "false",
            "waywallen.fill_mode": "centered"
        }"#;
        let properties = serde_json::from_str(raw).unwrap();
        let (renderer, layout) = split_renderer_properties(properties);
        assert_eq!(layout.fillmode, Some(FillMode::Centered));
        assert_eq!(
            renderer.get("waywallen.enable_audio").map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn normalizes_legacy_scheme_color_override_key() {
        let raw = r#"{
            "schemecolor": "0.1 0.2 0.3",
            "speed": "2"
        }"#;
        let normalized = normalize_user_property_overrides_json(raw);
        let value: serde_json::Value = serde_json::from_str(&normalized).unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(
            obj.get("waywallen.scheme_color").and_then(|v| v.as_str()),
            Some("0.1 0.2 0.3")
        );
        assert!(!obj.contains_key("schemecolor"));
        assert_eq!(obj.get("speed").and_then(|v| v.as_str()), Some("2"));
    }

    #[test]
    fn canonical_scheme_color_override_wins_over_legacy_alias() {
        let map = HashMap::from([
            (
                "waywallen.scheme_color".to_string(),
                "0.4 0.5 0.6".to_string(),
            ),
            ("schemecolor".to_string(), "0.1 0.2 0.3".to_string()),
        ]);
        let normalized = normalize_user_property_overrides(map);
        assert_eq!(
            normalized.get("waywallen.scheme_color").map(String::as_str),
            Some("0.4 0.5 0.6")
        );
        assert!(!normalized.contains_key("schemecolor"));
    }

    #[test]
    fn merges_authored_defaults_with_persisted_overrides() {
        let defaults = HashMap::from([
            ("schemecolor".to_string(), "0.1 0.2 0.3".to_string()),
            ("speed".to_string(), "1".to_string()),
            ("waywallen.fill_mode".to_string(), "centered".to_string()),
        ]);
        let merged = merge_renderer_user_properties(
            &defaults,
            &HashMap::from([
                (
                    "waywallen.scheme_color".to_string(),
                    "0.8 0.7 0.6".to_string(),
                ),
                ("enabled".to_string(), "true".to_string()),
            ]),
        );
        let value: HashMap<String, String> = serde_json::from_str(&merged).unwrap();
        assert_eq!(
            value.get("waywallen.scheme_color").map(String::as_str),
            Some("0.8 0.7 0.6")
        );
        assert_eq!(value.get("speed").map(String::as_str), Some("1"));
        assert_eq!(value.get("enabled").map(String::as_str), Some("true"));
        assert!(!value.contains_key("waywallen.fill_mode"));
    }

    #[test]
    fn reads_default_wire_values_from_property_schema() {
        let raw = r#"{
            "waywallen.scheme_color": { "type": "color", "value": [0.1, 0.2, 0.3, 1.0] },
            "waywallen.playback_speed": { "type": "slider", "value": 100 },
            "speed": { "type": "slider", "value": 1.5 },
            "enabled": { "type": "bool", "value": true },
            "mode": { "type": "combo", "value": "pulse" },
            "text": { "type": "textinput", "value": "" }
        }"#;
        assert_eq!(
            user_property_default_wire_value(raw, "waywallen.scheme_color").as_deref(),
            Some("0.1000 0.2000 0.3000 1.0000")
        );
        assert_eq!(
            user_property_default_wire_value(raw, "waywallen.playback_speed").as_deref(),
            Some("100")
        );
        assert_eq!(
            user_property_default_wire_value(raw, "speed").as_deref(),
            Some("1.5")
        );
        assert_eq!(
            user_property_default_wire_value(raw, "enabled").as_deref(),
            Some("true")
        );
        assert_eq!(
            user_property_default_wire_value(raw, "mode").as_deref(),
            Some("pulse")
        );
        assert_eq!(
            user_property_default_wire_value(raw, "text").as_deref(),
            Some("")
        );
    }

    #[test]
    fn reads_default_wire_value_by_canonical_key() {
        let raw = r#"{
            "schemecolor": { "type": "color", "value": "0.4 0.5 0.6" }
        }"#;
        assert_eq!(
            user_property_default_wire_value(raw, "waywallen.scheme_color").as_deref(),
            Some("0.4 0.5 0.6")
        );
    }
}
