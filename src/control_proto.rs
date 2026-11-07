include!(concat!(env!("OUT_DIR"), "/waywallen.control.v1.rs"));

use crate::plugin::renderer_registry::{SettingDef, SettingType};

pub fn plugin_message_to_proto(
    value: &crate::plugin::i18n::LocalizedText,
) -> Option<PluginMessage> {
    value.message().map(|value| PluginMessage {
        plugin_id: value.plugin_id.clone(),
        msgid: value.msgid.clone(),
        arguments: value.arguments.clone(),
    })
}

/// Stringify a `toml::Value` for the wire `default_value` / `min` /
/// `max` / `step` fields.
fn toml_value_to_wire(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        // Arrays/tables aren't valid setting scalars; fall back to the
        // TOML repr so the UI sees a deterministic fallback string.
        other => other.to_string(),
    }
}

fn setting_type_to_proto(ty: SettingType) -> i32 {
    match ty {
        SettingType::U32 => SettingValueType::U32 as i32,
        SettingType::I32 => SettingValueType::I32 as i32,
        SettingType::F32 => SettingValueType::F32 as i32,
        SettingType::String => SettingValueType::String as i32,
        SettingType::Bool => SettingValueType::Bool as i32,
    }
}

/// Convert a source plugin's Lua-declared `SourceSetting` into the same
/// `SettingSchema` wire shape, so `SourcePluginInfo.settings` renders through
/// the identical UI as renderer settings.
pub fn source_setting_to_proto(s: &crate::plugin::source::SourceSetting) -> SettingSchema {
    let ty = match s.ty.as_str() {
        "u32" => SettingValueType::U32,
        "i32" => SettingValueType::I32,
        "f32" => SettingValueType::F32,
        "bool" => SettingValueType::Bool,
        _ => SettingValueType::String,
    };
    SettingSchema {
        key: s.key.clone(),
        r#type: ty as i32,
        default_value: s.default.clone(),
        identity: false,
        label_key: s.label.text().to_string(),
        description_key: s.description.text().to_string(),
        min: String::new(),
        max: String::new(),
        step: String::new(),
        choices: s.choices.clone(),
        choice_labels: s
            .choice_labels
            .iter()
            .map(|label| plugin_message_to_proto(label).unwrap_or_default())
            .collect(),
        group: s.group.clone(),
        order: s.order,
        label: plugin_message_to_proto(&s.label),
        description: plugin_message_to_proto(&s.description),
        group_label: plugin_message_to_proto(&s.group_label),
    }
}

pub fn source_action_to_proto(a: &crate::plugin::source::SourceAction) -> PluginActionDef {
    use crate::plugin::source::SourceActionKind;
    PluginActionDef {
        id: a.id.clone(),
        label: a.label.text().to_string(),
        description: a.description.text().to_string(),
        browse_description: a.browse_description.text().to_string(),
        browse_button_label: a.browse_button_label.text().to_string(),
        group: a.group.clone(),
        order: a.order,
        kind: match a.kind {
            SourceActionKind::Invoke => PluginActionKind::Invoke,
            SourceActionKind::QrLogin => PluginActionKind::QrLogin,
            SourceActionKind::Form => PluginActionKind::Form,
        } as i32,
        visible: a.visible,
        enabled: a.enabled,
        fields: a
            .fields
            .iter()
            .map(|field| PluginActionField {
                key: field.key.clone(),
                label: field.label.text().to_string(),
                description: field.description.text().to_string(),
                placeholder: field.placeholder.text().to_string(),
                secret: field.secret,
                required: field.required,
                label_text: plugin_message_to_proto(&field.label),
                description_text: plugin_message_to_proto(&field.description),
                placeholder_text: plugin_message_to_proto(&field.placeholder),
            })
            .collect(),
        required_for_browsing: a.required_for_browsing,
        label_text: plugin_message_to_proto(&a.label),
        description_text: plugin_message_to_proto(&a.description),
        browse_description_text: plugin_message_to_proto(&a.browse_description),
        browse_button_label_text: plugin_message_to_proto(&a.browse_button_label),
        group_label: plugin_message_to_proto(&a.group_label),
    }
}

pub fn source_status_to_proto(s: &crate::plugin::source::SourceStatus) -> PluginStatusRow {
    PluginStatusRow {
        id: s.id.clone(),
        label: s.label.text().to_string(),
        group: s.group.clone(),
        order: s.order,
        value: s.value.text().to_string(),
        label_text: plugin_message_to_proto(&s.label),
        group_label: plugin_message_to_proto(&s.group_label),
        value_text: plugin_message_to_proto(&s.value),
    }
}

/// Convert one manifest `SettingDef` into the `SettingSchema` wire
/// shape consumed by `RendererPluginInfo.settings`.
pub fn setting_def_to_proto(key: &str, def: &SettingDef, plugin_id: &str) -> SettingSchema {
    let label = def
        .label
        .as_ref()
        .map(|value| value.resolve(plugin_id))
        .unwrap_or_else(|| {
            crate::plugin::i18n::LocalizedText::raw(
                def.label_key.clone().unwrap_or_else(|| key.to_string()),
            )
        });
    let description = def
        .description
        .as_ref()
        .map(|value| value.resolve(plugin_id))
        .unwrap_or_else(|| {
            crate::plugin::i18n::LocalizedText::raw(def.description_key.clone().unwrap_or_default())
        });
    let group_label = def
        .group_label
        .as_ref()
        .map(|value| value.resolve(plugin_id))
        .unwrap_or_else(|| {
            crate::plugin::i18n::LocalizedText::raw(def.group.clone().unwrap_or_default())
        });
    SettingSchema {
        key: key.to_string(),
        r#type: setting_type_to_proto(def.ty),
        default_value: toml_value_to_wire(&def.default),
        identity: def.identity,
        label_key: label.text().to_string(),
        description_key: description.text().to_string(),
        min: def.min.as_ref().map(toml_value_to_wire).unwrap_or_default(),
        max: def.max.as_ref().map(toml_value_to_wire).unwrap_or_default(),
        step: def
            .step
            .as_ref()
            .map(toml_value_to_wire)
            .unwrap_or_default(),
        choices: def.choices.clone().unwrap_or_default(),
        choice_labels: Vec::new(),
        group: def.group.clone().unwrap_or_default(),
        order: def.order.unwrap_or(0),
        label: plugin_message_to_proto(&label),
        description: plugin_message_to_proto(&description),
        group_label: plugin_message_to_proto(&group_label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_item_dimensions_round_trip() {
        use prost::Message;

        let legacy = RemoteItem {
            id: "legacy".into(),
            ..Default::default()
        };
        let decoded = RemoteItem::decode(legacy.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.width, None);
        assert_eq!(decoded.height, None);

        let sized = RemoteItem {
            width: Some(2250),
            height: Some(4000),
            ..legacy
        };
        let decoded = RemoteItem::decode(sized.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.width, Some(2250));
        assert_eq!(decoded.height, Some(4000));
    }

    #[test]
    fn plugin_message_mapping_preserves_namespace_and_raw_branch() {
        let translated = crate::plugin::i18n::LocalizedText::translated("org.test.one", "Status");
        let message = plugin_message_to_proto(&translated).unwrap();
        assert_eq!(message.plugin_id, "org.test.one");
        assert_eq!(message.msgid, "Status");

        let raw = crate::plugin::i18n::LocalizedText::raw("Status");
        assert!(plugin_message_to_proto(&raw).is_none());
    }
}
