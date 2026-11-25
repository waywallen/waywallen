use std::collections::HashMap;

use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, Set};
use waywallen::control_proto::ErrorCode;
use waywallen::model::{
    connect_url,
    entities::{item, wallpaper_property_preset as preset},
    repo,
};

async fn seed() -> DatabaseConnection {
    let db = connect_url("sqlite::memory:").await.unwrap();
    for sql in [
        "INSERT INTO source_plugin (id, name) VALUES (1, 'local')",
        "INSERT INTO library (id, plugin_id, path) VALUES (1, 1, '/first'), (2, 1, '/second')",
        "INSERT INTO item (id, plugin_id, library_id, path, type) VALUES
            (1, 1, 1, 'one.png', 'image'), (2, 1, 2, 'one.png', 'image'),
            (3, 1, 1, 'two.png', 'image')",
    ] {
        db.execute_unprepared(sql).await.unwrap();
    }
    db
}

fn overrides<const N: usize>(entries: [(&str, &str); N]) -> HashMap<String, String> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

#[tokio::test]
async fn apply_replaces_renderer_overrides_without_changing_layout() {
    let db = seed().await;
    const LAYOUT: &str = r#" { "fillmode": "fit", "rotation": 180 } "#;
    item::Entity::update(item::ActiveModel {
        id: Set(1),
        user_property_overrides: Set(Some(
            r#"{"schemecolor":"saved","strength":"0.6","waywallen.rotation":"90","waywallen.location_x":"20"}"#.into(),
        )),
        wallpaper_layout_override: Set(Some(LAYOUT.into())),
        ..Default::default()
    })
    .exec(&db)
    .await
    .unwrap();
    repo::create_wallpaper_preset(&db, 1, "Saved")
        .await
        .unwrap();
    let snapshot = repo::list_wallpaper_presets(&db, 1)
        .await
        .unwrap()
        .remove(0);
    let saved = overrides([("waywallen.scheme_color", "saved"), ("strength", "0.6")]);
    assert_eq!(
        serde_json::from_str::<HashMap<String, String>>(&snapshot.user_property_overrides).unwrap(),
        saved
    );

    item::Entity::update(item::ActiveModel {
        id: Set(1),
        user_property_overrides: Set(Some(
            r#"{"schemecolor":"current","discard":"yes","waywallen.rotation":"45","waywallen.location_y":"unknown"}"#.into(),
        )),
        ..Default::default()
    })
    .exec(&db)
    .await
    .unwrap();
    let change = repo::apply_wallpaper_preset(&db, 1, snapshot.id, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        change.before,
        overrides([("waywallen.scheme_color", "current"), ("discard", "yes")])
    );
    assert_eq!(change.after, saved);
    let legacy = overrides([
        ("waywallen.rotation", "45"),
        ("waywallen.location_y", "unknown"),
    ]);
    let mut expected = legacy.clone();
    expected.extend(saved);
    assert_eq!(
        repo::get_user_property_overrides(&db, 1).await.unwrap(),
        expected
    );

    for key in ["strength", "waywallen.scheme_color"] {
        repo::set_user_property_override(&db, 1, key, None)
            .await
            .unwrap();
    }
    repo::update_wallpaper_preset(&db, 1, snapshot.id)
        .await
        .unwrap();
    let updated = repo::list_wallpaper_presets(&db, 1)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(updated.id, snapshot.id);
    assert_eq!(updated.name, snapshot.name);
    assert_eq!(updated.user_property_overrides, "{}");
    repo::set_user_property_override(&db, 1, "strength", Some("0.2"))
        .await
        .unwrap();
    let change = repo::apply_wallpaper_preset(&db, 1, updated.id, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(change.before, overrides([("strength", "0.2")]));
    assert!(change.after.is_empty());
    let row = item::Entity::find_by_id(1).one(&db).await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<HashMap<String, String>>(&row.user_property_overrides.unwrap())
            .unwrap(),
        legacy
    );
    assert_eq!(row.wallpaper_layout_override.as_deref(), Some(LAYOUT));
}

#[tokio::test]
async fn quick_apply_requires_confirmation_only_for_unsaved_values() {
    let db = seed().await;
    repo::create_wallpaper_preset(&db, 1, "Defaults")
        .await
        .unwrap();
    repo::set_user_property_override(&db, 1, "text", Some(""))
        .await
        .unwrap();
    repo::create_wallpaper_preset(&db, 1, "Empty text")
        .await
        .unwrap();
    let presets = repo::list_wallpaper_presets(&db, 1).await.unwrap();
    let default_id = presets.iter().find(|p| p.name == "Defaults").unwrap().id;
    let empty_id = presets.iter().find(|p| p.name == "Empty text").unwrap().id;

    assert!(repo::apply_wallpaper_preset(&db, 1, default_id, true)
        .await
        .unwrap()
        .is_some());
    assert!(repo::apply_wallpaper_preset(&db, 1, empty_id, true)
        .await
        .unwrap()
        .is_some());

    repo::set_user_property_override(&db, 1, "text", Some("draft"))
        .await
        .unwrap();
    assert!(repo::apply_wallpaper_preset(&db, 1, default_id, true)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo::get_user_property_overrides(&db, 1).await.unwrap(),
        overrides([("text", "draft")])
    );
    assert!(repo::apply_wallpaper_preset(&db, 1, default_id, false)
        .await
        .unwrap()
        .is_some());
    assert!(repo::get_user_property_overrides(&db, 1)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn presets_follow_library_and_path_across_item_recreation() {
    let db = seed().await;
    for item_id in [1, 2, 3] {
        repo::create_wallpaper_preset(&db, item_id, "  Base  ")
            .await
            .unwrap();
    }
    let original = repo::list_wallpaper_presets(&db, 1)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(original.name, "Base");
    for item_id in [2, 3] {
        let other = repo::list_wallpaper_presets(&db, item_id).await.unwrap();
        assert_eq!(other.len(), 1);
        assert_ne!(other[0].id, original.id);
        assert!(
            repo::apply_wallpaper_preset(&db, item_id, original.id, false)
                .await
                .is_err()
        );
        assert!(repo::update_wallpaper_preset(&db, item_id, original.id)
            .await
            .is_err());
        assert!(
            repo::rename_wallpaper_preset(&db, item_id, original.id, "Wrong")
                .await
                .is_err()
        );
        assert!(repo::delete_wallpaper_preset(&db, item_id, original.id)
            .await
            .is_err());
    }

    assert_eq!(repo::delete_item(&db, 1).await.unwrap(), 1);
    assert_eq!(
        repo::list_wallpaper_presets(&db, 1)
            .await
            .unwrap_err()
            .error_code(),
        ErrorCode::WallpaperNotFound
    );
    db.execute_unprepared(
        "INSERT INTO item (id, plugin_id, library_id, path, type) VALUES (4, 1, 1, 'one.png', 'image')",
    )
    .await
    .unwrap();
    assert_eq!(
        repo::list_wallpaper_presets(&db, 4).await.unwrap(),
        [original.clone()]
    );
    repo::rename_wallpaper_preset(&db, 4, original.id, "Rescanned")
        .await
        .unwrap();
    assert_eq!(
        repo::list_wallpaper_presets(&db, 4).await.unwrap()[0].name,
        "Rescanned"
    );
    repo::delete_wallpaper_preset(&db, 4, original.id)
        .await
        .unwrap();
    assert!(repo::list_wallpaper_presets(&db, 4)
        .await
        .unwrap()
        .is_empty());

    repo::create_wallpaper_preset(&db, 4, "Orphan")
        .await
        .unwrap();
    let orphan = repo::list_wallpaper_presets(&db, 4)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(repo::delete_item(&db, 4).await.unwrap(), 1);
    assert_eq!(repo::remove_library(&db, 1).await.unwrap(), 1);
    assert!(preset::Entity::find_by_id(orphan.id)
        .one(&db)
        .await
        .unwrap()
        .is_none());
    assert_eq!(repo::list_wallpaper_presets(&db, 2).await.unwrap().len(), 1);
}

#[tokio::test]
async fn preset_names_are_validated_and_unique_in_their_scope() {
    let db = seed().await;
    let name = "\u{754c}".repeat(128);
    repo::create_wallpaper_preset(&db, 1, &format!(" {name} "))
        .await
        .unwrap();
    let original = repo::list_wallpaper_presets(&db, 1)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(original.name, name);

    for invalid in [" ".to_owned(), "a\u{7f}b".into(), format!("{name}x")] {
        let create = repo::create_wallpaper_preset(&db, 1, &invalid)
            .await
            .unwrap_err();
        assert_eq!(create.error_code(), ErrorCode::InvalidArgument);
        let rename = repo::rename_wallpaper_preset(&db, 1, original.id, &invalid)
            .await
            .unwrap_err();
        assert_eq!(rename.error_code(), ErrorCode::InvalidArgument);
    }
    for other in ["Duplicate", "duplicate"] {
        repo::create_wallpaper_preset(&db, 1, other).await.unwrap();
    }
    let duplicate = repo::create_wallpaper_preset(&db, 1, " Duplicate ")
        .await
        .unwrap_err();
    assert_eq!(duplicate.error_code(), ErrorCode::InvalidArgument);
    let duplicate = repo::rename_wallpaper_preset(&db, 1, original.id, "Duplicate")
        .await
        .unwrap_err();
    assert_eq!(duplicate.error_code(), ErrorCode::InvalidArgument);
    assert_eq!(
        preset::Entity::find_by_id(original.id)
            .one(&db)
            .await
            .unwrap(),
        Some(original)
    );
}
