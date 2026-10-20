use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};

use crate::error::{Error, Result, ResultExt};
use crate::model::entities::{playlist, playlist_item};
use crate::model::playlist_source::PlaylistSource;
use crate::playback::Mode;

#[derive(Debug, Clone)]
pub struct Summary {
    pub id: i64,
    pub name: String,
    pub mode: Mode,
    pub interval_secs: u32,
    pub synchronized_selection: bool,
    pub item_count: u32,
    pub revision: i64,
    pub source: PlaylistSource,
}

#[derive(Debug, Clone)]
pub struct Detail {
    pub summary: Summary,
    pub entries: Vec<crate::catalog::entry::WallpaperEntry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdateChanges {
    pub name_changed: bool,
    pub mode_changed: bool,
    pub interval_changed: bool,
    pub synchronized_selection_changed: bool,
    pub items_changed: bool,
}

pub async fn create(
    db: &DatabaseConnection,
    name: &str,
    mode: Mode,
    interval_secs: u32,
    synchronized_selection: bool,
    now_ms: i64,
    entry_ids: &[i64],
) -> Result<i64> {
    create_with_source(
        db,
        name,
        mode,
        interval_secs,
        synchronized_selection,
        now_ms,
        &PlaylistSource::Curated(entry_ids.to_vec()),
    )
    .await
}

pub async fn create_with_source(
    db: &DatabaseConnection,
    name: &str,
    mode: Mode,
    interval_secs: u32,
    synchronized_selection: bool,
    now_ms: i64,
    source: &PlaylistSource,
) -> Result<i64> {
    let encoded = source.encode_selection()?;
    let txn = db.begin().await.context("begin playlist create")?;
    let pl = playlist::ActiveModel {
        name: Set(name.to_owned()),
        smart_selection: Set(encoded),
        mode: Set(mode.into()),
        interval_secs: Set(interval_secs as i64),
        synchronized_selection: Set(synchronized_selection),
        created_at: Set(now_ms),
        updated_at: Set(now_ms),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .context("insert playlist")?;
    if let PlaylistSource::Curated(ids) = source {
        insert_items(&txn, pl.id, ids).await?;
    }
    txn.commit().await.context("commit playlist create")?;
    Ok(pl.id)
}

async fn insert_items<C: sea_orm::ConnectionTrait>(
    conn: &C,
    playlist_id: i64,
    entry_ids: &[i64],
) -> Result<()> {
    for (pos, entry_id) in entry_ids.iter().enumerate() {
        playlist_item::ActiveModel {
            playlist_id: Set(playlist_id),
            entry_id: Set(*entry_id),
            position: Set(pos as i64),
            ..Default::default()
        }
        .insert(conn)
        .await
        .with_context(|| format!("insert playlist_item pl={playlist_id} entry={entry_id}"))?;
    }
    Ok(())
}

pub async fn list(db: &DatabaseConnection) -> Result<Vec<Summary>> {
    let txn = db.begin().await.context("begin playlist list")?;
    let pls = playlist::Entity::find()
        .order_by_desc(playlist::Column::Id)
        .all(&txn)
        .await
        .context("select playlists")?;
    let mut out = Vec::with_capacity(pls.len());
    for pl in pls {
        let source = read_source(&txn, &pl).await?;
        let count = match &source {
            PlaylistSource::Curated(ids) => ids.len(),
            PlaylistSource::Smart(selection) => {
                super::select_wallpapers(&txn, selection).await?.len()
            }
        };
        out.push(Summary {
            id: pl.id,
            name: pl.name,
            mode: pl.mode.into(),
            interval_secs: u32::try_from(pl.interval_secs).unwrap_or(0),
            synchronized_selection: pl.synchronized_selection,
            item_count: count as u32,
            revision: pl.updated_at,
            source,
        });
    }
    txn.commit().await.context("commit playlist list")?;
    Ok(out)
}

pub async fn detail(db: &DatabaseConnection, id: i64) -> Result<Detail> {
    let txn = db.begin().await.context("begin playlist detail")?;
    let playlist = require(&txn, id).await?;
    let source = read_source(&txn, &playlist).await?;
    let mut entries = match &source {
        PlaylistSource::Smart(selection) => super::select_wallpapers(&txn, selection).await?,
        PlaylistSource::Curated(ids) => {
            let entries = super::get_entries_ordered(&txn, ids).await?;
            if entries.len() != ids.len() {
                return Err(Error::PlaylistInvalid(format!(
                    "playlist {id} contains unavailable wallpapers"
                )));
            }
            entries
        }
    };
    if source.is_smart() {
        super::populate_entry_tags(&txn, &mut entries).await?;
    }
    let detail = Detail {
        summary: Summary {
            id: playlist.id,
            name: playlist.name,
            mode: playlist.mode.into(),
            interval_secs: u32::try_from(playlist.interval_secs).unwrap_or(0),
            synchronized_selection: playlist.synchronized_selection,
            item_count: entries.len() as u32,
            revision: playlist.updated_at,
            source,
        },
        entries,
    };
    txn.commit().await.context("commit playlist detail")?;
    Ok(detail)
}

pub async fn get(
    db: &DatabaseConnection,
    id: i64,
) -> Result<Option<crate::playback::playlist::Playlist>> {
    let model = playlist::Entity::find_by_id(id)
        .one(db)
        .await
        .with_context(|| format!("get playlist id={id}"))?;
    Ok(model.map(|model| crate::playback::playlist::Playlist {
        id: model.id,
        name: model.name,
        mode: model.mode.into(),
        interval_secs: u32::try_from(model.interval_secs).unwrap_or(0),
        synchronized_selection: model.synchronized_selection,
    }))
}

pub async fn delete(db: &DatabaseConnection, id: i64) -> Result<u64> {
    let res = playlist::Entity::delete_by_id(id)
        .exec(db)
        .await
        .with_context(|| format!("delete playlist id={id}"))?;
    Ok(res.rows_affected)
}

pub async fn update(
    db: &DatabaseConnection,
    id: i64,
    name: &str,
    mode: Mode,
    interval_secs: u32,
    synchronized_selection: bool,
    desired_entry_ids: &[i64],
    expected_revision: Option<i64>,
    now_ms: i64,
) -> Result<UpdateChanges> {
    update_with_source(
        db,
        id,
        name,
        mode,
        interval_secs,
        synchronized_selection,
        &PlaylistSource::Curated(desired_entry_ids.to_vec()),
        expected_revision,
        now_ms,
    )
    .await
}

pub async fn update_with_source(
    db: &DatabaseConnection,
    id: i64,
    name: &str,
    mode: Mode,
    interval_secs: u32,
    synchronized_selection: bool,
    source: &PlaylistSource,
    expected_revision: Option<i64>,
    now_ms: i64,
) -> Result<UpdateChanges> {
    let encoded = source.encode_selection()?;
    let txn = db.begin().await.context("begin update playlist")?;
    let playlist = require(&txn, id).await?;
    if playlist.smart_selection.is_some() != source.is_smart() {
        return Err(Error::PlaylistInvalid(
            "playlist source cannot be converted".into(),
        ));
    }
    if let Some(expected) = expected_revision {
        if expected != playlist.updated_at {
            return Err(Error::PlaylistRevisionConflict {
                expected,
                current: playlist.updated_at,
            });
        }
    }

    let mode = playlist::PlaylistMode::from(mode);
    let current_source = read_source(&txn, &playlist).await?;
    let changes = UpdateChanges {
        name_changed: playlist.name != name,
        mode_changed: playlist.mode != mode,
        interval_changed: playlist.interval_secs != i64::from(interval_secs),
        synchronized_selection_changed: playlist.synchronized_selection != synchronized_selection,
        items_changed: &current_source != source,
    };
    if changes == UpdateChanges::default() {
        txn.commit().await.context("commit update playlist")?;
        return Ok(changes);
    }

    let revision = next_revision(playlist.updated_at, now_ms);
    let mut active: playlist::ActiveModel = playlist.into();
    active.name = Set(name.to_owned());
    active.mode = Set(mode);
    active.interval_secs = Set(i64::from(interval_secs));
    active.synchronized_selection = Set(synchronized_selection);
    active.updated_at = Set(revision);
    active.smart_selection = Set(encoded);
    active.update(&txn).await.context("update playlist")?;
    if changes.items_changed {
        playlist_item::Entity::delete_many()
            .filter(playlist_item::Column::PlaylistId.eq(id))
            .exec(&txn)
            .await
            .context("clear playlist_item")?;
        if let PlaylistSource::Curated(ids) = source {
            insert_items(&txn, id, ids).await?;
        }
    }
    txn.commit().await.context("commit update playlist")?;
    Ok(changes)
}

pub async fn rename(db: &DatabaseConnection, id: i64, name: &str, now_ms: i64) -> Result<()> {
    let playlist = require(db, id).await?;
    let revision = next_revision(playlist.updated_at, now_ms);
    let mut am: playlist::ActiveModel = playlist.into();
    am.name = Set(name.to_owned());
    am.updated_at = Set(revision);
    am.update(db).await.context("rename playlist")?;
    Ok(())
}

pub async fn set_mode(db: &DatabaseConnection, id: i64, mode: Mode, now_ms: i64) -> Result<()> {
    let playlist = require(db, id).await?;
    let revision = next_revision(playlist.updated_at, now_ms);
    let mut am: playlist::ActiveModel = playlist.into();
    am.mode = Set(mode.into());
    am.updated_at = Set(revision);
    am.update(db).await.context("set playlist mode")?;
    Ok(())
}

pub async fn set_interval(db: &DatabaseConnection, id: i64, secs: u32, now_ms: i64) -> Result<()> {
    let playlist = require(db, id).await?;
    let revision = next_revision(playlist.updated_at, now_ms);
    let mut am: playlist::ActiveModel = playlist.into();
    am.interval_secs = Set(secs as i64);
    am.updated_at = Set(revision);
    am.update(db).await.context("set playlist interval")?;
    Ok(())
}

pub async fn set_items(
    db: &DatabaseConnection,
    id: i64,
    entry_ids: &[i64],
    expected_revision: Option<i64>,
    now_ms: i64,
) -> Result<()> {
    let txn = db.begin().await.context("begin set_items")?;
    let pl = require(&txn, id).await?;
    if pl.smart_selection.is_some() {
        return Err(Error::PlaylistInvalid(
            "smart playlists do not accept manual members".into(),
        ));
    }
    if let Some(expected) = expected_revision {
        if expected != pl.updated_at {
            return Err(Error::PlaylistRevisionConflict {
                expected,
                current: pl.updated_at,
            });
        }
    }
    playlist_item::Entity::delete_many()
        .filter(playlist_item::Column::PlaylistId.eq(id))
        .exec(&txn)
        .await
        .context("clear playlist_item")?;
    insert_items(&txn, id, entry_ids).await?;
    let revision = next_revision(pl.updated_at, now_ms);
    let mut am: playlist::ActiveModel = pl.into();
    am.updated_at = Set(revision);
    am.update(&txn).await.context("touch playlist")?;
    txn.commit().await.context("commit set_items")?;
    Ok(())
}

pub async fn entry_ids<C: sea_orm::ConnectionTrait>(db: &C, id: i64) -> Result<Vec<i64>> {
    let rows = playlist_item::Entity::find()
        .filter(playlist_item::Column::PlaylistId.eq(id))
        .order_by_asc(playlist_item::Column::Position)
        .all(db)
        .await
        .with_context(|| format!("list playlist_item pl={id}"))?;
    Ok(rows.into_iter().map(|r| r.entry_id).collect())
}

fn next_revision(current: i64, now_ms: i64) -> i64 {
    now_ms.max(current.saturating_add(1))
}

pub async fn revision(db: &DatabaseConnection, id: i64) -> Result<i64> {
    Ok(require(db, id).await?.updated_at)
}

async fn read_source<C: sea_orm::ConnectionTrait>(
    conn: &C,
    playlist: &playlist::Model,
) -> Result<PlaylistSource> {
    match &playlist.smart_selection {
        Some(encoded) => PlaylistSource::decode_selection(encoded),
        None => Ok(PlaylistSource::Curated(entry_ids(conn, playlist.id).await?)),
    }
}

async fn require<C: sea_orm::ConnectionTrait>(conn: &C, id: i64) -> Result<playlist::Model> {
    playlist::Entity::find_by_id(id)
        .one(conn)
        .await
        .with_context(|| format!("require playlist id={id}"))?
        .ok_or_else(|| crate::error::Error::PlaylistNotFound(id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::repo as model_repo;

    async fn mem_db() -> DatabaseConnection {
        crate::model::connect_url("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn smart_playlist_resolves_without_storing_members() {
        use crate::catalog::query::WallpaperSelection;
        let db = mem_db().await;
        let selection = PlaylistSource::Smart(WallpaperSelection::default());
        let id = create_with_source(&db, "smart", Mode::Sequential, 30, true, 1, &selection)
            .await
            .unwrap();
        assert_eq!(detail(&db, id).await.unwrap().summary.item_count, 0);
        let items = seed_items(&db, 2).await;
        let result = detail(&db, id).await.unwrap();
        assert_eq!(result.summary.item_count, 2);
        assert_eq!(result.summary.revision, 1);
        assert_eq!(result.summary.source, selection);
        assert!(entry_ids(&db, id).await.unwrap().is_empty());
        assert!(set_items(&db, id, &items, None, 2).await.is_err());
        assert!(
            update(&db, id, "bad", Mode::Random, 50, false, &items, None, 2)
                .await
                .is_err()
        );
        super::super::set_items_hidden(&db, &items, true)
            .await
            .unwrap();
        assert_eq!(detail(&db, id).await.unwrap().summary.item_count, 0);
        assert_eq!(detail(&db, id).await.unwrap().summary.revision, 1);
        rename(&db, id, "renamed", 2).await.unwrap();
        assert_eq!(detail(&db, id).await.unwrap().summary.name, "renamed");
        let mut changed = WallpaperSelection::default();
        changed.hidden = crate::catalog::query::HiddenFilter::Include;
        update_with_source(
            &db,
            id,
            "renamed",
            Mode::Sequential,
            30,
            true,
            &PlaylistSource::Smart(changed),
            Some(2),
            3,
        )
        .await
        .unwrap();
        assert_eq!(detail(&db, id).await.unwrap().summary.item_count, 2);
        let mut active: playlist::ActiveModel = require(&db, id).await.unwrap().into();
        active.smart_selection = Set(Some("{}".into()));
        active.update(&db).await.unwrap();
        assert!(detail(&db, id).await.is_err());
    }

    async fn seed_items(db: &DatabaseConnection, count: usize) -> Vec<i64> {
        let plugin = model_repo::upsert_plugin(db, "playlist-test", "")
            .await
            .unwrap();
        let library = model_repo::add_library(db, plugin.id, "/playlist-test")
            .await
            .unwrap();
        let mut ids = Vec::with_capacity(count);
        for idx in 0..count {
            let path = format!("item-{idx}");
            let item = model_repo::upsert_item(
                db,
                model_repo::ItemUpsertArgs {
                    plugin_id: plugin.id,
                    library_id: library.id,
                    path: &path,
                    ty: "image",
                    display_name: "",
                    preview_path: None,
                    description: None,
                    external_id: None,
                    web_url: None,
                    update_available: false,
                    size: None,
                    width: None,
                    height: None,
                    content_rating: None,
                },
            )
            .await
            .unwrap();
            ids.push(item.id);
        }
        ids
    }

    #[tokio::test]
    async fn create_list_delete_roundtrip() {
        let db = mem_db().await;
        let items = seed_items(&db, 1).await;
        let id = create(&db, "Nature", Mode::Shuffle, 300, true, 1, &items)
            .await
            .unwrap();
        let all = list(&db).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Nature");
        assert_eq!(all[0].mode, Mode::Shuffle);
        assert_eq!(all[0].interval_secs, 300);
        assert!(all[0].synchronized_selection);
        assert_eq!(all[0].item_count, 1);
        assert_eq!(delete(&db, id).await.unwrap(), 1);
        assert!(list(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_items_replaces_in_order() {
        let db = mem_db().await;
        let items = seed_items(&db, 3).await;
        let id = create(&db, "p", Mode::Sequential, 0, true, 1, &items[0..1])
            .await
            .unwrap();
        set_items(&db, id, &[items[1], items[2], items[0]], None, 2)
            .await
            .unwrap();
        assert_eq!(
            entry_ids(&db, id).await.unwrap(),
            vec![items[1], items[2], items[0]]
        );
        set_items(&db, id, &items[0..1], None, 3).await.unwrap();
        assert_eq!(entry_ids(&db, id).await.unwrap(), vec![items[0]]);
    }

    #[tokio::test]
    async fn delete_cascades_items() {
        let db = mem_db().await;
        let items = seed_items(&db, 2).await;
        let id = create(&db, "p", Mode::Sequential, 0, true, 1, &items)
            .await
            .unwrap();
        delete(&db, id).await.unwrap();
        assert!(entry_ids(&db, id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mutators_touch_fields() {
        let db = mem_db().await;
        let items = seed_items(&db, 1).await;
        let id = create(&db, "p", Mode::Sequential, 0, true, 1, &items)
            .await
            .unwrap();
        rename(&db, id, "Renamed", 5).await.unwrap();
        set_mode(&db, id, Mode::Random, 6).await.unwrap();
        set_interval(&db, id, 60, 7).await.unwrap();
        let s = &list(&db).await.unwrap()[0];
        assert_eq!(s.name, "Renamed");
        assert_eq!(s.mode, Mode::Random);
        assert_eq!(s.interval_secs, 60);
        assert_eq!(s.revision, 7);
    }

    #[tokio::test]
    async fn update_is_atomic_and_rejects_stale_revision() {
        let db = mem_db().await;
        let items = seed_items(&db, 2).await;
        let id = create(&db, "p", Mode::Sequential, 30, true, 5, &items[0..1])
            .await
            .unwrap();
        let changes = update(
            &db,
            id,
            "Renamed",
            Mode::Random,
            60,
            false,
            &items[1..2],
            Some(5),
            5,
        )
        .await
        .unwrap();
        assert_eq!(
            changes,
            UpdateChanges {
                name_changed: true,
                mode_changed: true,
                interval_changed: true,
                synchronized_selection_changed: true,
                items_changed: true,
            }
        );
        let updated = detail(&db, id).await.unwrap();
        assert_eq!(updated.summary.name, "Renamed");
        assert_eq!(updated.summary.mode, Mode::Random);
        assert_eq!(updated.summary.interval_secs, 60);
        assert!(!updated.summary.synchronized_selection);
        assert_eq!(updated.summary.revision, 6);
        assert_eq!(updated.entries[0].item_id, items[1]);

        let unchanged = update(
            &db,
            id,
            "Renamed",
            Mode::Random,
            60,
            false,
            &items[1..2],
            Some(6),
            6,
        )
        .await
        .unwrap();
        assert_eq!(unchanged, UpdateChanges::default());
        assert_eq!(detail(&db, id).await.unwrap().summary.revision, 6);

        let error = update(
            &db,
            id,
            "Stale",
            Mode::Shuffle,
            90,
            true,
            &items[0..1],
            Some(5),
            7,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            Error::PlaylistRevisionConflict {
                expected: 5,
                current: 6
            }
        ));
        let unchanged = detail(&db, id).await.unwrap();
        assert_eq!(unchanged.summary.name, "Renamed");
        assert_eq!(unchanged.summary.mode, Mode::Random);
        assert_eq!(unchanged.summary.interval_secs, 60);
        assert_eq!(unchanged.summary.revision, 6);
        assert_eq!(unchanged.entries[0].item_id, items[1]);
    }

    #[tokio::test]
    async fn detail_returns_ordered_entries_and_empty_playlists() {
        let db = mem_db().await;
        let items = seed_items(&db, 3).await;
        let id = create(
            &db,
            "ordered",
            Mode::Sequential,
            30,
            true,
            10,
            &[items[2], items[0], items[1]],
        )
        .await
        .unwrap();
        let ordered = detail(&db, id).await.unwrap();
        assert_eq!(ordered.summary.name, "ordered");
        assert_eq!(ordered.summary.revision, 10);
        assert_eq!(
            ordered
                .entries
                .iter()
                .map(|entry| entry.item_id)
                .collect::<Vec<_>>(),
            vec![items[2], items[0], items[1]]
        );

        let empty_id = create(&db, "empty", Mode::Sequential, 30, true, 11, &[])
            .await
            .unwrap();
        let empty = detail(&db, empty_id).await.unwrap();
        assert!(empty.entries.is_empty());
        assert_eq!(empty.summary.item_count, 0);
    }

    #[tokio::test]
    async fn set_items_rejects_stale_revision_without_writing() {
        let db = mem_db().await;
        let items = seed_items(&db, 2).await;
        let id = create(&db, "p", Mode::Sequential, 30, true, 5, &items[0..1])
            .await
            .unwrap();
        let error = set_items(&db, id, &items[1..2], Some(4), 6)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::PlaylistRevisionConflict {
                expected: 4,
                current: 5
            }
        ));
        assert_eq!(entry_ids(&db, id).await.unwrap(), vec![items[0]]);
        assert_eq!(detail(&db, id).await.unwrap().summary.revision, 5);

        set_items(&db, id, &items[1..2], Some(5), 5).await.unwrap();
        let updated = detail(&db, id).await.unwrap();
        assert_eq!(updated.summary.revision, 6);
        assert_eq!(updated.entries[0].item_id, items[1]);
    }
}
