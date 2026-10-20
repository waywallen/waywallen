use super::WallpaperEntry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StringMatch {
    Is,
    IsNot,
    Contains,
    ContainsNot,
}

impl StringMatch {
    pub fn from_code(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Is),
            2 => Some(Self::IsNot),
            3 => Some(Self::Contains),
            4 => Some(Self::ContainsNot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntMatch {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl IntMatch {
    pub fn from_code(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Equal),
            2 => Some(Self::NotEqual),
            3 => Some(Self::Less),
            4 => Some(Self::LessEqual),
            5 => Some(Self::Greater),
            6 => Some(Self::GreaterEqual),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterPredicate {
    Name {
        value: String,
        condition: StringMatch,
    },
    WallpaperType {
        value: String,
        condition: StringMatch,
    },
    Library {
        value: i64,
        exclude: bool,
    },
    Width {
        value: i64,
        condition: IntMatch,
    },
    Height {
        value: i64,
        condition: IntMatch,
    },
    Size {
        value: i64,
        condition: IntMatch,
    },
    ContentRating {
        value: String,
        condition: StringMatch,
    },
    Tags {
        values: Vec<String>,
        condition: StringMatch,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterRule {
    pub group: i32,
    pub predicate: FilterPredicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogicOperator {
    And,
    Or,
}

impl LogicOperator {
    pub fn from_code(value: i32) -> Self {
        match value {
            2 => Self::Or,
            _ => Self::And,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterLogic {
    pub operator: LogicOperator,
    pub group_a: i32,
    pub group_b: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    Name,
    WallpaperType,
    Size,
    LastModified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortRule {
    pub key: SortKey,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogQuery {
    pub filters: Vec<FilterRule>,
    pub logics: Vec<FilterLogic>,
    pub sorts: Vec<SortRule>,
    pub search_text: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HiddenFilter {
    #[default]
    Exclude,
    Include,
    Only,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperSelection {
    pub filters: Vec<FilterRule>,
    pub logics: Vec<FilterLogic>,
    pub sorts: Vec<SortRule>,
    pub search_text: String,
    pub wp_type: String,
    pub skip_types: Vec<String>,
    pub filter_tags: Vec<String>,
    pub skip_content_ratings: Vec<String>,
    pub hidden: HiddenFilter,
    pub hidden_library_ids: Vec<i64>,
}

impl WallpaperSelection {
    pub fn validate(&self) -> crate::error::Result<()> {
        use crate::error::Error;
        if self.hidden_library_ids.iter().any(|id| *id <= 0)
            || self.filters.iter().any(|rule| matches!(rule.predicate, FilterPredicate::Library { value, .. } if value <= 0))
        {
            return Err(Error::InvalidArgument(
                "library IDs must be positive".into(),
            ));
        }
        let groups: std::collections::HashSet<_> = self.filters.iter().map(|f| f.group).collect();
        if groups.iter().any(|g| *g < 0)
            || self
                .logics
                .iter()
                .any(|logic| !groups.contains(&logic.group_a) || !groups.contains(&logic.group_b))
        {
            return Err(Error::InvalidArgument(
                "invalid filter group reference".into(),
            ));
        }
        Ok(())
    }
}

pub fn sort_entries(entries: &mut [&WallpaperEntry], sorts: &[SortRule]) {
    for rule in sorts.iter().rev() {
        entries.sort_by(|a, b| {
            let ordering = match rule.key {
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::WallpaperType => a.wp_type.cmp(&b.wp_type),
                SortKey::Size => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
                SortKey::LastModified => last_modified_key(a).cmp(&last_modified_key(b)),
            };
            if rule.direction == SortDirection::Descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }

    fn last_modified_key(entry: &WallpaperEntry) -> i64 {
        entry.modified_at.unwrap_or(entry.create_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, modified_at: Option<i64>, create_at: i64) -> WallpaperEntry {
        WallpaperEntry {
            item_id: 0,
            name: name.to_string(),
            wp_type: "image".to_string(),
            resource: name.to_string(),
            preview: None,
            description: None,
            tags: Vec::new(),
            external_id: None,
            web_url: None,
            update_available: false,
            size: None,
            width: None,
            height: None,
            content_rating: None,
            modified_at,
            create_at,
            plugin_name: String::new(),
            library_root: String::new(),
            hidden: false,
        }
    }

    #[test]
    fn last_modified_sort_falls_back_to_create_at() {
        let newer_created = entry("created-newer", None, 30);
        let older_modified = entry("modified-older", Some(20), 40);
        let older_created = entry("created-older", None, 10);
        let mut entries = vec![&newer_created, &older_modified, &older_created];

        sort_entries(
            &mut entries,
            &[SortRule {
                key: SortKey::LastModified,
                direction: SortDirection::Ascending,
            }],
        );

        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["created-older", "modified-older", "created-newer"]
        );
    }
}
