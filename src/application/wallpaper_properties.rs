use std::collections::HashMap;
use std::sync::Arc;

use super::WallpaperEntry;
use crate::catalog::properties::user_property_default_wire_value;
use crate::daemon::DaemonContext;
use crate::error::Result;
use crate::wallframe::ipc::proto::ControlMsg;

pub(crate) fn override_changes(
    before: &HashMap<String, String>,
    after: &HashMap<String, String>,
) -> Vec<(String, Option<String>)> {
    let mut changes: Vec<_> = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, value)| (key.clone(), Some(value.clone())))
        .chain(
            before
                .keys()
                .filter(|key| !after.contains_key(*key))
                .map(|key| (key.clone(), None)),
        )
        .collect();
    changes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    changes
}

pub(crate) async fn push_changes(
    app: &Arc<DaemonContext>,
    entry: &WallpaperEntry,
    changes: &[(String, Option<String>)],
) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    let wallpaper_id = entry.item_id.to_string();
    let active = app
        .router
        .update_wallpaper_assignment_properties(&wallpaper_id, changes)
        .await;
    let mut schema = None;
    let mut failure = None;
    for target in active {
        let id = &target.renderer_id;
        let Some(handle) = app
            .renderer_manager
            .get(id)
            .await
            .filter(|handle| handle.process_generation == target.process_generation)
        else {
            continue;
        };
        let mut settings = Vec::with_capacity(changes.len());
        for (key, value) in changes {
            let value = if let Some(value) =
                value.clone().or_else(|| handle.default_user_property(key))
            {
                value
            } else {
                if schema.is_none() {
                    schema = Some(
                        app.source_manager
                            .call_properties(&entry.plugin_name, entry)
                            .await
                            .ok()
                            .flatten()
                            .unwrap_or_default(),
                    );
                }
                schema
                    .as_deref()
                    .and_then(|schema| user_property_default_wire_value(schema, key))
                    .unwrap_or_else(|| {
                        log::warn!("reset {key} on wallpaper {wallpaper_id} has no default value");
                        String::new()
                    })
            };
            settings.push((key.clone(), value));
        }
        if let Err(error) = app
            .renderer_manager
            .send_control_to_handle(id, &handle, ControlMsg::SettingChanged { settings })
            .await
        {
            log::warn!("could not update renderer {id} for wallpaper {wallpaper_id}: {error}");
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_reset_omitted_properties_and_keep_empty_values() {
        let before = HashMap::from([
            ("removed".into(), "1".into()),
            ("unchanged".into(), "true".into()),
            ("updated".into(), "hello".into()),
        ]);
        let after = HashMap::from([
            ("added".into(), "0".into()),
            ("unchanged".into(), "true".into()),
            ("updated".into(), String::new()),
        ]);
        assert_eq!(
            override_changes(&before, &after),
            vec![
                ("added".into(), Some("0".into())),
                ("removed".into(), None),
                ("updated".into(), Some(String::new())),
            ]
        );
    }
}
