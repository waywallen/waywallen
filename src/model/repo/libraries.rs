use super::*;

// library

pub fn normalize_library_ids(ids: &[i64]) -> Result<Vec<i64>> {
    if ids.iter().any(|id| *id <= 0) {
        return Err(Error::InvalidArgument(
            "library IDs must be positive".into(),
        ));
    }
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

pub fn expand_home_path(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_owned();
    };
    if !rest.is_empty() && !rest.starts_with('/') {
        return path.to_owned();
    }
    let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) else {
        return path.to_owned();
    };
    let mut expanded = std::path::PathBuf::from(home);
    if let Some(rest) = rest.strip_prefix('/') {
        if !rest.is_empty() {
            expanded.push(rest);
        }
    }
    expanded.to_string_lossy().into_owned()
}

pub fn library_path_key(path: &str) -> std::path::PathBuf {
    let path = expand_home_path(path);
    // Keep unavailable libraries addressable without changing their stored paths.
    std::fs::canonicalize(&path).unwrap_or_else(|_| std::path::PathBuf::from(path))
}

pub async fn add_library(
    db: &DatabaseConnection,
    plugin_id: i64,
    path: &str,
) -> Result<library::Model> {
    add_named_library(db, plugin_id, path, None).await
}

pub async fn add_named_library(
    db: &DatabaseConnection,
    plugin_id: i64,
    path: &str,
    name: Option<&str>,
) -> Result<library::Model> {
    let path = expand_home_path(path);
    let txn = db.begin().await.context("begin library create")?;
    if let Some(existing) = find_library(&txn, plugin_id, &path).await? {
        return Err(Error::InvalidArgument(format!(
            "Library folder is already registered: {}",
            existing.path
        )));
    }
    let name = match name {
        Some(name) => validate_library_name(name)?.to_owned(),
        None => {
            let used: HashSet<String> = library::Entity::find()
                .select_only()
                .column(library::Column::Name)
                .into_tuple::<String>()
                .all(&txn)
                .await
                .context("select library names")?
                .into_iter()
                .collect();
            crate::model::library_name::default_name(&path, &used)
        }
    };
    let am = library::ActiveModel {
        plugin_id: Set(plugin_id),
        path: Set(path.clone()),
        name: Set(name),
        ..Default::default()
    };
    let library = am
        .insert(&txn)
        .await
        .with_context(|| format!("insert library plugin={plugin_id} path={path}"))?;
    txn.commit().await.context("commit library create")?;
    Ok(library)
}

fn validate_library_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::InvalidArgument(
            "library name must not be empty".into(),
        ));
    }
    Ok(name)
}

pub async fn rename_library(
    db: &DatabaseConnection,
    id: i64,
    name: &str,
) -> Result<library::Model> {
    let name = validate_library_name(name)?;
    let existing = library::Entity::find_by_id(id)
        .one(db)
        .await
        .context("select library to rename")?
        .ok_or(Error::LibraryNotFound(id))?;
    let mut am: library::ActiveModel = existing.into();
    am.name = Set(name.to_owned());
    am.update(db).await.context("rename library")
}

pub async fn find_library(
    db: &impl ConnectionTrait,
    plugin_id: i64,
    path: &str,
) -> Result<Option<library::Model>> {
    let path = expand_home_path(path);
    let key = library_path_key(&path);
    let libraries = library::Entity::find()
        .filter(library::Column::PluginId.eq(plugin_id))
        .order_by_asc(library::Column::Id)
        .all(db)
        .await
        .with_context(|| format!("select library plugin={plugin_id} path={path}"))?;
    Ok(libraries
        .into_iter()
        .find(|library| library.path == path || library_path_key(&library.path) == key))
}

pub async fn list_libraries_by_plugin(
    db: &DatabaseConnection,
    plugin_id: i64,
) -> Result<Vec<library::Model>> {
    library::Entity::find()
        .filter(library::Column::PluginId.eq(plugin_id))
        .order_by_asc(library::Column::Path)
        .all(db)
        .await
        .with_context(|| format!("select libraries plugin={plugin_id}"))
}

pub async fn list_libraries(db: &DatabaseConnection) -> Result<Vec<library::Model>> {
    library::Entity::find()
        .order_by_asc(library::Column::Id)
        .all(db)
        .await
        .context("select libraries")
}

pub async fn remove_library(db: &DatabaseConnection, id: i64) -> Result<u64> {
    let res = library::Entity::delete_by_id(id)
        .exec(db)
        .await
        .with_context(|| format!("delete library id={id}"))?;
    Ok(res.rows_affected)
}

/// Decode the JSON blob in `library.metadata` into a flat string map.
/// Invalid or empty JSON falls back to an empty map so callers never
pub async fn get_library_metadata(
    db: &DatabaseConnection,
    library_id: i64,
) -> Result<HashMap<String, String>> {
    let row = library::Entity::find_by_id(library_id)
        .one(db)
        .await
        .with_context(|| format!("select library id={library_id} for metadata"))?
        .ok_or(Error::LibraryNotFound(library_id))?;
    Ok(decode_library_metadata(&row.metadata))
}

pub async fn get_library_metadata_value(
    db: &DatabaseConnection,
    library_id: i64,
    key: &str,
) -> Result<Option<String>> {
    Ok(get_library_metadata(db, library_id).await?.remove(key))
}

/// Read-modify-write a single key in `library.metadata`. Pass
/// `value = None` to delete the key. Other keys survive.
pub async fn set_library_metadata_value(
    db: &DatabaseConnection,
    library_id: i64,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    let existing = library::Entity::find_by_id(library_id)
        .one(db)
        .await
        .with_context(|| format!("reload library id={library_id} for metadata write"))?
        .ok_or(Error::LibraryNotFound(library_id))?;
    let mut map = decode_library_metadata(&existing.metadata);
    match value {
        Some(v) => {
            map.insert(key.to_owned(), v.to_owned());
        }
        None => {
            map.remove(key);
        }
    }
    let encoded = serde_json::to_string(&map).context("encode library metadata")?;
    let mut am: library::ActiveModel = existing.into();
    am.metadata = Set(encoded);
    am.update(db)
        .await
        .with_context(|| format!("update library metadata id={library_id}"))?;
    Ok(())
}

/// Replace the full metadata map atomically.
pub async fn replace_library_metadata(
    db: &DatabaseConnection,
    library_id: i64,
    kv: &HashMap<String, String>,
) -> Result<()> {
    let existing = library::Entity::find_by_id(library_id)
        .one(db)
        .await
        .with_context(|| format!("reload library id={library_id} for metadata write"))?
        .ok_or(Error::LibraryNotFound(library_id))?;
    let encoded = serde_json::to_string(kv).context("encode library metadata")?;
    let mut am: library::ActiveModel = existing.into();
    am.metadata = Set(encoded);
    am.update(db)
        .await
        .with_context(|| format!("update library metadata id={library_id}"))?;
    Ok(())
}

fn decode_library_metadata(raw: &str) -> HashMap<String, String> {
    if raw.is_empty() {
        return HashMap::new();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

pub async fn delete_libraries_missing(
    db: &DatabaseConnection,
    plugin_id: i64,
    keep: &HashSet<String>,
) -> Result<u64> {
    let mut q = library::Entity::delete_many().filter(library::Column::PluginId.eq(plugin_id));
    if !keep.is_empty() {
        q = q.filter(library::Column::Path.is_not_in(keep.iter().cloned()));
    }
    let res = q
        .exec(db)
        .await
        .with_context(|| format!("delete missing libraries plugin={plugin_id}"))?;
    Ok(res.rows_affected)
}

// ---------------------------------------------------------------------------
