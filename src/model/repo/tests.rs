use super::*;
use crate::model::connect_url;
use std::ffi::OsString;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct HomeGuard {
    old: Option<OsString>,
}

impl HomeGuard {
    fn set(home: &str) -> Self {
        let old = std::env::var_os("HOME");
        std::env::set_var("HOME", home);
        Self { old }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        if let Some(old) = &self.old {
            std::env::set_var("HOME", old);
        } else {
            std::env::remove_var("HOME");
        }
    }
}

async fn mem_db() -> DatabaseConnection {
    connect_url("sqlite::memory:").await.unwrap()
}

#[tokio::test]
async fn library_names_default_and_allow_duplicates() {
    let db = mem_db().await;
    let plugin = upsert_plugin(&db, "names", "1").await.unwrap();
    let a = add_library(&db, plugin.id, "/a/Wallpapers/").await.unwrap();
    assert_eq!(a.name, "Wallpapers");
    let b = add_named_library(&db, plugin.id, "/b", Some("Wallpapers-1"))
        .await
        .unwrap();
    let c = add_library(&db, plugin.id, "/c/Wallpapers").await.unwrap();
    assert_eq!(c.name, "Wallpapers-2");
    let duplicate = add_named_library(&db, plugin.id, "/d", Some(" Wallpapers "))
        .await
        .unwrap();
    assert_eq!(duplicate.name, a.name);
    let renamed = rename_library(&db, b.id, "Wallpapers").await.unwrap();
    assert_eq!(renamed.name, a.name);
    assert_eq!(renamed.id, b.id);
    assert_eq!(renamed.path, b.path);
    assert_eq!(renamed.plugin_id, b.plugin_id);
    assert_eq!(renamed.metadata, b.metadata);
    assert_eq!(
        find_library(&db, plugin.id, "/b")
            .await
            .unwrap()
            .unwrap()
            .name,
        a.name
    );
    let next = add_library(&db, plugin.id, "/next/Wallpapers")
        .await
        .unwrap();
    assert_eq!(next.name, "Wallpapers-1");
    assert!(matches!(
        rename_library(&db, a.id, " \n").await,
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        add_named_library(&db, plugin.id, "/empty", Some("")).await,
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        rename_library(&db, 999, "Missing").await,
        Err(Error::LibraryNotFound(999))
    ));
    assert!(add_library(&db, plugin.id, &a.path).await.is_err());
    assert_eq!(
        add_library(&db, plugin.id, "/壁纸").await.unwrap().name,
        "壁纸"
    );
    assert_eq!(add_library(&db, plugin.id, "/").await.unwrap().name, "/");
}

#[tokio::test]
async fn library_names_concurrent_defaults() {
    let db = mem_db().await;
    let plugin = upsert_plugin(&db, "names", "1").await.unwrap();
    let (a, b) = tokio::join!(
        add_library(&db, plugin.id, "/a/Wallpapers"),
        add_library(&db, plugin.id, "/b/Wallpapers"),
    );
    let names: HashSet<_> = [a.unwrap().name, b.unwrap().name].into_iter().collect();
    assert_eq!(
        names,
        HashSet::from(["Wallpapers".to_owned(), "Wallpapers-1".to_owned()])
    );
}

#[tokio::test]
async fn library_visibility_excludes_only_hidden_ids() {
    let db = mem_db().await;
    let p1 = upsert_plugin(&db, "library-a", "1").await.unwrap();
    let p2 = upsert_plugin(&db, "library-b", "1").await.unwrap();
    let a = add_library(&db, p1.id, "/shared").await.unwrap();
    let b = add_library(&db, p2.id, "/shared").await.unwrap();
    let c = add_library(&db, p1.id, "/other").await.unwrap();
    for (plugin, library) in [(p1.id, a.id), (p2.id, b.id), (p1.id, c.id)] {
        upsert_item(&db, minimal_args(plugin, library, "same.png", "image"))
            .await
            .unwrap();
    }
    assert_eq!(
        load_entries_excluding_libraries(&db, &[])
            .await
            .unwrap()
            .len(),
        3
    );
    let entries = load_entries_excluding_libraries(&db, &[b.id, b.id])
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.plugin_name == "library-a"));
    assert_eq!(
        load_entries_excluding_libraries(&db, &[a.id, b.id])
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(load_entries_excluding_libraries(&db, &[a.id, b.id, c.id])
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        load_entries_excluding_libraries(&db, &[999])
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        load_entries_excluding_libraries(&db, &[999, a.id])
            .await
            .unwrap()
            .len(),
        2
    );
    for id in [0, -1] {
        assert!(matches!(
            load_entries_excluding_libraries(&db, &[id]).await,
            Err(Error::InvalidArgument(_))
        ));
    }
    remove_library(&db, c.id).await.unwrap();
    let replacement = add_library(&db, p1.id, "/other").await.unwrap();
    assert!(replacement.id > c.id);
    upsert_item(
        &db,
        minimal_args(p1.id, replacement.id, "same.png", "image"),
    )
    .await
    .unwrap();
    assert_eq!(
        load_entries_excluding_libraries(&db, &[c.id])
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        load_entries_excluding_libraries(&db, &[a.id, b.id, c.id])
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        load_entries_excluding_libraries(&db, &[])
            .await
            .unwrap()
            .len(),
        3
    );
}

fn minimal_args<'a>(
    plugin_id: i64,
    library_id: i64,
    path: &'a str,
    ty: &'a str,
) -> ItemUpsertArgs<'a> {
    ItemUpsertArgs {
        plugin_id,
        library_id,
        path,
        ty,
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
    }
}

#[tokio::test]
async fn item_hidden_batch_validates_all_ids_and_preserves_scan_state() {
    let db = mem_db().await;
    let plugin = upsert_plugin(&db, "hidden-test", "1").await.unwrap();
    let library = add_library(&db, plugin.id, "/hidden-test").await.unwrap();
    let a = upsert_item(&db, minimal_args(plugin.id, library.id, "a.png", "image"))
        .await
        .unwrap();
    let b = upsert_item(&db, minimal_args(plugin.id, library.id, "b.png", "image"))
        .await
        .unwrap();
    assert!(!a.hidden && !b.hidden);
    assert_eq!(
        set_items_hidden(&db, &[b.id, a.id, a.id], true)
            .await
            .unwrap(),
        vec![a.id, b.id]
    );
    item::Entity::update_many()
        .col_expr(item::Column::UpdateAt, Expr::value(123_i64))
        .filter(item::Column::Id.eq(a.id))
        .exec(&db)
        .await
        .unwrap();
    let saved = item::Entity::find_by_id(a.id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(set_items_hidden(&db, &[a.id], true)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        item::Entity::find_by_id(a.id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .update_at,
        saved.update_at
    );
    let invalid = b.id + 1;
    assert!(
        matches!(set_items_hidden(&db, &[a.id, invalid], false).await,
        Err(Error::WallpaperNotFound(id)) if id == invalid.to_string())
    );
    assert!(get_entry(&db, a.id).await.unwrap().unwrap().hidden);

    let rescanned = upsert_item(&db, minimal_args(plugin.id, library.id, "a.png", "image"))
        .await
        .unwrap();
    assert!(rescanned.hidden);
    assert_eq!(
        set_items_hidden(&db, &[a.id], false).await.unwrap(),
        vec![a.id]
    );
    assert_eq!(
        set_items_hidden(&db, &[a.id, b.id], true).await.unwrap(),
        vec![a.id]
    );
    assert!(set_items_hidden(&db, &[], true).await.unwrap().is_empty());
}

#[tokio::test]
async fn item_hidden_batch_rolls_back_all_chunks_on_failure() {
    let db = mem_db().await;
    let plugin = upsert_plugin(&db, "hidden-test", "1").await.unwrap();
    let library = add_library(&db, plugin.id, "/hidden-test").await.unwrap();
    let mut ids = Vec::new();
    for n in 0..501 {
        ids.push(
            upsert_item(
                &db,
                minimal_args(plugin.id, library.id, &format!("{n}.png"), "image"),
            )
            .await
            .unwrap()
            .id,
        );
    }
    let mut invalid = ids.clone();
    invalid.push(ids.last().unwrap() + 1);
    assert!(set_items_hidden(&db, &invalid, true).await.is_err());
    assert_eq!(
        item::Entity::find()
            .filter(item::Column::Hidden.eq(true))
            .count(&db)
            .await
            .unwrap(),
        0
    );

    db.execute(sea_orm::Statement::from_string(db.get_database_backend(), format!(
        "CREATE TRIGGER reject_hidden BEFORE UPDATE OF hidden ON item WHEN NEW.id = {} BEGIN SELECT RAISE(ABORT, 'test failure'); END", ids.last().unwrap()
    ))).await.unwrap();
    assert!(set_items_hidden(&db, &ids, true).await.is_err());
    assert_eq!(
        item::Entity::find()
            .filter(item::Column::Hidden.eq(true))
            .count(&db)
            .await
            .unwrap(),
        0
    );
    db.execute(sea_orm::Statement::from_string(
        db.get_database_backend(),
        "DROP TRIGGER reject_hidden".to_owned(),
    ))
    .await
    .unwrap();
    assert_eq!(set_items_hidden(&db, &ids, true).await.unwrap(), ids);
}

#[tokio::test]
async fn upsert_plugin_inserts_then_updates_version() {
    let db = mem_db().await;
    let p1 = upsert_plugin(&db, "wescene", "1.0").await.unwrap();
    let p2 = upsert_plugin(&db, "wescene", "1.1").await.unwrap();
    assert_eq!(p2.id, p1.id);
    assert_eq!(p2.version, "1.1");
}

#[tokio::test]
async fn library_path_scoped_per_plugin() {
    let db = mem_db().await;
    let a = upsert_plugin(&db, "a", "").await.unwrap();
    let b = upsert_plugin(&db, "b", "").await.unwrap();
    add_library(&db, a.id, "/shared").await.unwrap();
    add_library(&db, b.id, "/shared").await.unwrap();
    assert!(add_library(&db, a.id, "/shared").await.is_err());
}

#[tokio::test]
async fn add_library_expands_home_prefix() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _home = HomeGuard::set("/tmp/waywallen-home");
    let db = mem_db().await;
    let p = upsert_plugin(&db, "p", "").await.unwrap();

    let root = add_library(&db, p.id, "~").await.unwrap();
    let pictures = add_library(&db, p.id, "~/Pictures").await.unwrap();

    assert_eq!(root.path, "/tmp/waywallen-home");
    assert_eq!(pictures.path, "/tmp/waywallen-home/Pictures");
    assert!(find_library(&db, p.id, "~/Pictures")
        .await
        .unwrap()
        .is_some());
    assert_eq!(expand_home_path("~other/Pictures"), "~other/Pictures");
}

#[tokio::test]
async fn upsert_item_refreshes_every_column_on_conflict() {
    let db = mem_db().await;
    let p = upsert_plugin(&db, "p", "").await.unwrap();
    let lib = add_library(&db, p.id, "/root").await.unwrap();
    upsert_item(
        &db,
        ItemUpsertArgs {
            plugin_id: p.id,
            library_id: lib.id,
            path: "a.png",
            ty: "image",
            display_name: "Old",
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
    let updated = upsert_item(
        &db,
        ItemUpsertArgs {
            plugin_id: p.id,
            library_id: lib.id,
            path: "a.png",
            ty: "GIF",
            display_name: "New",
            preview_path: Some("new/preview.png"),
            description: Some("now animated"),
            external_id: Some("ext-42"),
            web_url: Some("https://example.invalid/item/ext-42"),
            update_available: false,
            size: None,
            width: None,
            height: None,
            content_rating: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.ty, "gif");
    assert_eq!(updated.display_name, "New");
    assert_eq!(updated.preview_path.as_deref(), Some("new/preview.png"));
    assert_eq!(updated.description.as_deref(), Some("now animated"));
    assert_eq!(updated.external_id.as_deref(), Some("ext-42"));
    assert_eq!(
        updated.web_url.as_deref(),
        Some("https://example.invalid/item/ext-42")
    );
}

#[tokio::test]
async fn upsert_item_persists_media_meta() {
    let db = mem_db().await;
    let p = upsert_plugin(&db, "p", "").await.unwrap();
    let lib = add_library(&db, p.id, "/root").await.unwrap();
    let first = upsert_item(
        &db,
        ItemUpsertArgs {
            plugin_id: p.id,
            library_id: lib.id,
            path: "video.mkv",
            ty: "video",
            display_name: "v",
            preview_path: None,
            description: None,
            external_id: None,
            web_url: None,
            update_available: false,
            size: Some(123_456),
            width: Some(1920),
            height: Some(1080),
            content_rating: Some("Everyone"),
        },
    )
    .await
    .unwrap();
    assert_eq!(first.size, Some(123_456));
    assert_eq!(first.width, Some(1920));
    assert_eq!(first.height, Some(1080));
    assert_eq!(first.content_rating.as_deref(), Some("Everyone"));

    // Re-upserting with None must preserve the prior probe-filled
    // values — otherwise plugin re-scans clobber size/width/height
    let second = upsert_item(
        &db,
        ItemUpsertArgs {
            plugin_id: p.id,
            library_id: lib.id,
            path: "video.mkv",
            ty: "video",
            display_name: "v",
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
    assert_eq!(second.size, Some(123_456));
    assert_eq!(second.width, Some(1920));
    assert_eq!(second.height, Some(1080));
    assert_eq!(second.content_rating.as_deref(), Some("Everyone"));
}

#[tokio::test]
async fn upsert_tags_dedupes_case_insensitively() {
    let db = mem_db().await;
    let tags = upsert_tags(
        &db,
        &[
            "Anime".into(),
            "anime".into(),
            "Landscape".into(),
            "ANIME".into(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(tags.len(), 2);
    let all = list_tags(&db).await.unwrap();
    let names: Vec<_> = all.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Anime", "Landscape"]);
}

#[tokio::test]
async fn upsert_tags_skips_whitespace_entries() {
    let db = mem_db().await;
    let tags = upsert_tags(&db, &["  ".into(), "".into(), " Anime ".into()])
        .await
        .unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "Anime");
}

#[tokio::test]
async fn replace_item_tags_idempotent_and_atomic() {
    let db = mem_db().await;
    let p = upsert_plugin(&db, "p", "").await.unwrap();
    let lib = add_library(&db, p.id, "/r").await.unwrap();
    let item = upsert_item(&db, minimal_args(p.id, lib.id, "a.png", "image"))
        .await
        .unwrap();
    let tags = upsert_tags(&db, &["Anime".into(), "Nature".into(), "Game".into()])
        .await
        .unwrap();
    let ids: HashMap<&str, i64> = tags.iter().map(|t| (t.name.as_str(), t.id)).collect();

    replace_item_tags(&db, item.id, &[ids["Anime"], ids["Nature"]])
        .await
        .unwrap();
    assert_eq!(list_tags_of_item(&db, item.id).await.unwrap().len(), 2);

    replace_item_tags(&db, item.id, &[ids["Game"]])
        .await
        .unwrap();
    let after = list_tags_of_item(&db, item.id).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].name, "Game");
}

#[tokio::test]
async fn list_items_by_tag_crosses_libraries() {
    let db = mem_db().await;
    let p = upsert_plugin(&db, "p", "").await.unwrap();
    let l1 = add_library(&db, p.id, "/one").await.unwrap();
    let l2 = add_library(&db, p.id, "/two").await.unwrap();
    let i1 = upsert_item(&db, minimal_args(p.id, l1.id, "a", "image"))
        .await
        .unwrap();
    let i2 = upsert_item(&db, minimal_args(p.id, l2.id, "b", "image"))
        .await
        .unwrap();
    let tags = upsert_tags(&db, &["Shared".into()]).await.unwrap();
    replace_item_tags(&db, i1.id, &[tags[0].id]).await.unwrap();
    replace_item_tags(&db, i2.id, &[tags[0].id]).await.unwrap();
    assert_eq!(list_items_by_tag(&db, tags[0].id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn item_delete_cascades_item_tag() {
    let db = mem_db().await;
    let p = upsert_plugin(&db, "p", "").await.unwrap();
    let lib = add_library(&db, p.id, "/r").await.unwrap();
    let item = upsert_item(&db, minimal_args(p.id, lib.id, "a", "image"))
        .await
        .unwrap();
    let tags = upsert_tags(&db, &["Anime".into()]).await.unwrap();
    replace_item_tags(&db, item.id, &[tags[0].id])
        .await
        .unwrap();

    remove_library(&db, lib.id).await.unwrap();
    assert!(list_items_by_tag(&db, tags[0].id).await.unwrap().is_empty());
    assert_eq!(list_tags(&db).await.unwrap().len(), 1);
}

#[tokio::test]
async fn remove_plugin_cascades_everything_including_item_tag() {
    let db = mem_db().await;
    let p = upsert_plugin(&db, "doomed", "").await.unwrap();
    let lib = add_library(&db, p.id, "/x").await.unwrap();
    let it = upsert_item(&db, minimal_args(p.id, lib.id, "a", "image"))
        .await
        .unwrap();
    let tags = upsert_tags(&db, &["T".into()]).await.unwrap();
    replace_item_tags(&db, it.id, &[tags[0].id]).await.unwrap();

    remove_plugin(&db, p.id).await.unwrap();
    assert!(list_plugins(&db).await.unwrap().is_empty());
    assert!(list_items_by_plugin(&db, p.id).await.unwrap().is_empty());
    assert!(list_items_by_tag(&db, tags[0].id).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_items_synced_before_sweeps_only_scoped_and_stale() {
    let db = mem_db().await;
    let p = upsert_plugin(&db, "p", "").await.unwrap();
    let l1 = add_library(&db, p.id, "/one").await.unwrap();
    let l2 = add_library(&db, p.id, "/two").await.unwrap();
    // Seed three items in l1 and one in l2 (all stamped "old").
    for rel in ["a", "b", "c"] {
        upsert_item(&db, minimal_args(p.id, l1.id, rel, "image"))
            .await
            .unwrap();
    }
    upsert_item(&db, minimal_args(p.id, l2.id, "z", "image"))
        .await
        .unwrap();
    // Advance the clock, then re-see only l1/a — it gets a fresh
    // sync_at; the cutoff sits between the two timestamps.
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let cutoff = crate::tasks::now_ms();
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    upsert_item(&db, minimal_args(p.id, l1.id, "a", "image"))
        .await
        .unwrap();
    // Sweep l1 only: stale b/c go, fresh a stays; l2 untouched
    // because it isn't in the scoped set.
    let deleted = delete_items_synced_before(&db, &[l1.id], cutoff)
        .await
        .unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(list_items_by_library(&db, l1.id).await.unwrap().len(), 1);
    assert_eq!(list_items_by_library(&db, l2.id).await.unwrap().len(), 1);
}

async fn seed_queue_db() -> (DatabaseConnection, Vec<i64>) {
    let db = mem_db().await;
    let plug = upsert_plugin(&db, "p", "").await.unwrap();
    let lib = add_library(&db, plug.id, "/lib").await.unwrap();
    let mut ids = Vec::new();
    for path in ["a.png", "b.png", "c.png"] {
        let it = upsert_item(&db, minimal_args(plug.id, lib.id, path, "image"))
            .await
            .unwrap();
        ids.push(it.id);
    }
    (db, ids)
}

#[tokio::test]
async fn random_item_by_filter_skips_excluded_when_pool_has_others() {
    let (db, ids) = seed_queue_db().await;
    for _ in 0..16 {
        let row = random_item_by_filter(&db, &[], &[], Some(ids[0]))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(row.item_id, ids[0], "exclude_id must never come back");
    }
}

#[tokio::test]
async fn random_item_by_filter_returns_only_when_pool_is_singleton() {
    let (db, ids) = seed_queue_db().await;
    let plugin = find_plugin_by_name(&db, "p").await.unwrap().unwrap();
    let library = find_library(&db, plugin.id, "/lib").await.unwrap().unwrap();
    let mut args = minimal_args(plugin.id, library.id, "a.png", "image");
    args.width = Some(640);
    upsert_item(&db, args).await.unwrap();
    let filter = crate::catalog::FilterRule {
        group: 0,
        predicate: crate::catalog::query::FilterPredicate::Width {
            value: 640,
            condition: crate::catalog::query::IntMatch::Equal,
        },
    };

    let row = random_item_by_filter(&db, &[filter], &[], Some(ids[0]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.item_id, ids[0]);
}

#[tokio::test]
async fn list_item_ids_by_filter_returns_stable_order() {
    let (db, ids) = seed_queue_db().await;
    let listed = list_item_ids_by_filter(&db, &[], &[]).await.unwrap();
    assert_eq!(listed, ids);
}

#[tokio::test]
async fn count_items_by_filter_with_no_filter_counts_all() {
    let (db, _) = seed_queue_db().await;
    assert_eq!(count_items_by_filter(&db, &[], &[]).await.unwrap(), 3);
}

#[tokio::test]
async fn user_property_override_distinguishes_empty_value_from_reset() {
    let (db, ids) = seed_queue_db().await;

    set_user_property_override(&db, ids[0], "text", Some(""))
        .await
        .unwrap();
    let overrides = get_user_property_overrides(&db, ids[0]).await.unwrap();
    assert_eq!(overrides.get("text").map(String::as_str), Some(""));

    set_user_property_override(&db, ids[0], "text", None)
        .await
        .unwrap();
    let overrides = get_user_property_overrides(&db, ids[0]).await.unwrap();
    assert!(!overrides.contains_key("text"));
}

#[tokio::test]
async fn find_item_by_library_path_round_trip() {
    let (db, ids) = seed_queue_db().await;
    let it = find_item_by_library_path(&db, "/lib", "b.png")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(it.id, ids[1]);
    assert!(find_item_by_library_path(&db, "/lib", "missing.png")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_libraries_missing_drops_absent_and_cascades_items() {
    let db = mem_db().await;
    let p = upsert_plugin(&db, "p", "").await.unwrap();
    let keep_lib = add_library(&db, p.id, "/keep").await.unwrap();
    let drop_lib = add_library(&db, p.id, "/drop").await.unwrap();
    upsert_item(&db, minimal_args(p.id, drop_lib.id, "x", "image"))
        .await
        .unwrap();
    let keep_set: HashSet<String> = ["/keep".to_owned()].into_iter().collect();
    let deleted = delete_libraries_missing(&db, p.id, &keep_set)
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    let remaining = list_libraries_by_plugin(&db, p.id).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, keep_lib.id);
    assert_eq!(list_items_by_plugin(&db, p.id).await.unwrap().len(), 0);
}
