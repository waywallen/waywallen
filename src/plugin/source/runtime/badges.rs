use super::*;

impl LuaPluginRuntime {
    pub(super) fn discover_badge_definitions(table: &LuaTable) -> Result<Vec<DiscoverBadgeDef>> {
        let context = "info().capabilities.discover";
        let Some(table) = Self::optional_table(table, "badges", context)? else {
            return Ok(Vec::new());
        };
        let mut definitions = Vec::new();
        let mut ids = HashSet::new();
        for (index, badge) in table.sequence_values::<LuaTable>().enumerate() {
            let context = format!("{context}.badges[{}]", index + 1);
            if index >= 8 {
                return Err(Error::InvalidArgument(
                    "too many discover badge definitions".into(),
                ));
            }
            let badge = badge?;
            let id = Self::require_string(&badge, "id", &context)?;
            if id.is_empty() || id.len() > 64 || !ids.insert(id.clone()) {
                return Err(Error::InvalidArgument(format!(
                    "{context}.id must be non-empty and unique"
                )));
            }
            definitions.push(DiscoverBadgeDef {
                id,
                label: Self::require_localized_text(&badge, "label", &context)?,
            });
        }
        Ok(definitions)
    }

    pub(super) fn discover_item_badges(
        table: &LuaTable,
        definitions: &[DiscoverBadgeDef],
        context: &str,
    ) -> Result<Vec<DiscoverBadge>> {
        let Some(table) = Self::optional_table(table, "badges", context)? else {
            return Ok(Vec::new());
        };
        let mut badges = Vec::new();
        let mut ids = HashSet::new();
        for (index, badge) in table.sequence_values::<LuaTable>().enumerate() {
            let context = format!("{context}.badges[{}]", index + 1);
            if index >= 8 {
                return Err(Error::InvalidArgument(
                    "too many discover item badges".into(),
                ));
            }
            let badge = badge?;
            let id = Self::require_string(&badge, "id", &context)?;
            if !definitions.iter().any(|definition| definition.id == id) || !ids.insert(id.clone())
            {
                return Err(Error::InvalidArgument(format!(
                    "{context}.id must be declared and unique"
                )));
            }
            let text = Self::require_string(&badge, "text", &context)?;
            if text.trim().is_empty() || text.len() > 64 || text.chars().any(char::is_control) {
                return Err(Error::InvalidArgument(format!(
                    "{context}.text must be a short single-line label"
                )));
            }
            badges.push(DiscoverBadge { id, text });
        }
        Ok(badges)
    }
}
