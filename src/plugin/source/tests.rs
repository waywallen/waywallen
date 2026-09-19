use super::parsing::redact_secrets;
use super::*;
use crate::plugin::i18n::LocalizedText;
use crate::probe::media::{MediaMeta, MediaProbe};
use std::io::Write;
use std::time::Duration;

struct FakeProbe {
    meta: MediaMeta,
}

#[test]
fn remote_errors_redact_credentials() {
    let redacted = redact_secrets(
            "access_token=access-secret&refresh_token=refresh-secret\nAuthorization: Bearer bearer-secret\nCookie: session-secret\n{\"access_token\": \"json-secret\"}",
        );
    assert!(!redacted.contains("access-secret"));
    assert!(!redacted.contains("refresh-secret"));
    assert!(!redacted.contains("bearer-secret"));
    assert!(!redacted.contains("session-secret"));
    assert!(!redacted.contains("json-secret"));
}

#[test]
fn discover_callback_errors_do_not_expose_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("redaction.lua");
    std::fs::write(
        &entry,
        r#"
local M = {}
function M.info()
    return { name = "redaction", capabilities = { discover = { search = true } } }
end
M.discover = {}
function M.discover.search(ctx, params)
    error("request failed: access_token=access-secret&refresh_token=refresh-secret")
end
return M
"#,
    )
    .unwrap();
    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&entry, "org.redaction", "1", ENTRY_VERSION_V3)
        .unwrap();
    let error = block_value(async { manager.call_discover("redaction", "", "", 1, &[]).await })
        .unwrap_err()
        .to_string();
    assert!(!error.contains("access-secret"));
    assert!(!error.contains("refresh-secret"));
    assert!(error.contains("[REDACTED]"));
}

#[test]
fn remote_settings_patch_uses_source_schema() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("settings.lua");
    std::fs::write(
        &entry,
        r#"
local M = {}
function M.info()
    return {
        name = "settings_remote",
        settings = {
            { key = "count", type = "u32", default = 1 },
            { key = "enabled", type = "bool", default = false },
            { key = "mode", type = "string", default = "one", choices = { "one", "two" } },
        },
        capabilities = { discover = { search = true } },
    }
end
M.discover = {}
function M.discover.search(ctx, params)
    return { items = {}, has_more = false }
end
return M
"#,
    )
    .unwrap();
    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&entry, "org.settings", "1", ENTRY_VERSION_V3)
        .unwrap();

    let values = HashMap::from([
        ("count".to_string(), "007".to_string()),
        ("enabled".to_string(), "true".to_string()),
        ("mode".to_string(), "two".to_string()),
    ]);
    let validated = manager
        .validate_remote_settings_patch("settings_remote", values)
        .unwrap();
    assert_eq!(validated.get("count").map(String::as_str), Some("7"));
    assert_eq!(validated.get("enabled").map(String::as_str), Some("true"));
    assert_eq!(validated.get("mode").map(String::as_str), Some("two"));

    for values in [
        HashMap::from([("missing".to_string(), "value".to_string())]),
        HashMap::from([("enabled".to_string(), "yes".to_string())]),
        HashMap::from([("mode".to_string(), "three".to_string())]),
    ] {
        assert!(manager
            .validate_remote_settings_patch("settings_remote", values)
            .is_err());
    }
}

#[test]
fn source_settings_reject_invalid_schema() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("settings.lua");
    let script = |settings: &str| {
        format!(
            r#"
local M = {{}}
function M.info()
    return {{
        name = "invalid_settings",
        settings = {{ {settings} }},
        capabilities = {{ discover = {{ search = true }} }},
    }}
end
M.discover = {{}}
function M.discover.search(ctx, params)
    return {{ items = {{}}, has_more = false }}
end
return M
"#,
        )
    };

    std::fs::write(&entry, script(r#"{ key = "same" }, { key = "same" }"#)).unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&entry, "org.invalid", "1", ENTRY_VERSION_V3)
        .is_err());

    std::fs::write(&entry, script(r#"{ key = "value", type = "object" }"#)).unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&entry, "org.invalid", "1", ENTRY_VERSION_V3)
        .is_err());
}

impl MediaProbe for FakeProbe {
    fn probe_media(&self, _path: &str) -> MediaMeta {
        self.meta.clone()
    }
}

/// Drive an async scan from a sync `#[test]` — these tests don't
/// touch the DB so a single-thread runtime is fine.
fn block(fut: impl std::future::Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

fn block_value<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

#[test]
fn ctx_probe_callable_from_lua() {
    let probe = Arc::new(FakeProbe {
        meta: MediaMeta {
            width: Some(1920),
            height: Some(1080),
        },
    });
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("probe_test.lua");
    let mut f = std::fs::File::create(&plugin_path).unwrap();
    write!(
        f,
        r#"
local M = {{}}
function M.info()
    return {{
        name = "probe_test",
        capabilities = {{
            source = {{ types = {{"video"}}, scan = true }},
        }},
    }}
end
M.source = {{}}
function M.source.scan(ctx)
    local m = ctx.probe("/fake/path/video.mp4")
    if m == nil then error("probe returned nil") end
    return {{
        {{
            id = "v1",
            name = "Video",
            wp_type = "video",
            resource = "/lib/v1.mp4",
            library_root = "/lib",
            metadata = {{}},
            _probe_size = m.size,
            _probe_width = m.width,
            _probe_height = m.height,
        }},
    }}
end
return M
"#
    )
    .unwrap();

    let mgr = SourceManager::with_probe(probe as Arc<dyn MediaProbe>).unwrap();
    mgr.load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    block(async { mgr.scan_all(&HashMap::new()).await.unwrap() });

    let entries = mgr.list();
    assert_eq!(entries.len(), 1);
    // The Lua plugin called ctx.probe successfully (it would error() otherwise).
    // Verify the entry was emitted correctly.
    assert_eq!(entries[0].resource, "/lib/v1.mp4");
}

#[test]
fn v3_source_context_has_grouped_interfaces_and_v2_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("grouped_ctx.lua");
    std::fs::write(
        &plugin_path,
        r#"
local M = {}
function M.info()
    return {
        name = "grouped_ctx",
        capabilities = { source = { types = {"image"}, scan = true } },
    }
end
M.source = {}
function M.source.scan(ctx)
    assert(type(ctx.fs) == "table" and type(ctx.fs.exists) == "function")
    assert(type(ctx.config) == "table" and type(ctx.config.get) == "function")
    assert(type(ctx.json) == "table" and type(ctx.json.parse) == "function")
    assert(type(ctx.file_exists) == "function")
    assert(type(ctx.plugin_config) == "function")
    assert(type(ctx.json_parse) == "function")
    return {}
end
return M
"#,
    )
    .unwrap();

    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&plugin_path, "org.grouped", "1", ENTRY_VERSION_V3)
        .unwrap();
    block_value(async { manager.scan_all(&HashMap::new()).await }).unwrap();
}

#[test]
fn fs_read_bytes_returns_a_binary_window_and_refuses_oversized_requests() {
    let dir = tempfile::tempdir().unwrap();
    let blob_path = dir.path().join("container.bin");
    // A non-UTF-8 byte in the middle: `read` would refuse this file, the
    // window must hand it back untouched.
    std::fs::write(&blob_path, b"HEAD\xffTAIL").unwrap();

    let plugin_path = dir.path().join("read_bytes.lua");
    std::fs::write(
        &plugin_path,
        format!(
            r#"
local M = {{}}
function M.info()
    return {{
        name = "read_bytes",
        capabilities = {{ source = {{ types = {{"image"}}, scan = true }} }},
    }}
end
M.source = {{}}
function M.source.scan(ctx)
    local path = "{path}"
    assert(ctx.fs.read_bytes(path, 0, 4) == "HEAD", "head window")
    assert(ctx.fs.read_bytes(path, 4, 1) == "\xff", "binary byte survives")
    assert(ctx.fs.read_bytes(path, 5, 64) == "TAIL", "short read at EOF")
    assert(ctx.fs.read_bytes(path, 9, 4) == nil, "read past EOF")
    assert(ctx.fs.read_bytes(path, -1, 4) == nil, "negative offset")
    assert(ctx.fs.read_bytes(path, 0, 0) == nil, "empty window")
    assert(ctx.fs.read_bytes(path, 0, 1048577) == nil, "over the cap")
    assert(ctx.fs.read_bytes(path .. ".missing", 0, 4) == nil, "missing file")
    return {{}}
end
return M
"#,
            path = blob_path.display()
        ),
    )
    .unwrap();

    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&plugin_path, "org.read_bytes", "1", ENTRY_VERSION_V4)
        .unwrap();
    block_value(async { manager.scan_all(&HashMap::new()).await }).unwrap();
}

#[test]
fn scan_all_reports_a_failing_plugin_instead_of_reporting_success() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("failing_source.lua");
    let mut f = std::fs::File::create(&plugin_path).unwrap();
    write!(
        f,
        r#"
local M = {{}}
function M.info()
    return {{
        name = "failing",
        capabilities = {{
            source = {{ types = {{"image"}}, scan = true }},
        }},
    }}
end
M.source = {{}}
function M.source.scan(ctx)
    error("library root is unreadable")
end
return M
"#
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    mgr.load_plugin(&plugin_path, "failing.plugin", "1.0", ENTRY_VERSION)
        .unwrap();

    let result = block_value(async { mgr.scan_all(&HashMap::new()).await });

    let err = result.expect_err("a failing source scan must not report success");
    assert!(
        err.to_string().contains("failing"),
        "error should name the plugin that failed, got: {err}"
    );
    assert!(mgr.list().is_empty());
}

#[test]
fn test_load_and_scan_plugin() {
    let dir = tempfile::tempdir().unwrap();

    // Write a minimal source plugin
    let plugin_path = dir.path().join("test_source.lua");
    let mut f = std::fs::File::create(&plugin_path).unwrap();
    write!(
        f,
        r#"
local M = {{}}
function M.info()
    return {{
        name = "test",
        capabilities = {{
            source = {{ types = {{"image"}}, scan = true }},
        }},
    }}
end
M.source = {{}}
function M.source.scan(ctx)
    return {{
        {{ id = "w1", name = "Test Wallpaper", wp_type = "image",
           resource = "/tmp/test.png", metadata = {{}} }},
    }}
end
return M
"#
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    let name = mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    assert_eq!(name, "test");

    block(async { mgr.scan_all(&HashMap::new()).await.unwrap() });
    assert_eq!(mgr.list().len(), 1);
    assert_eq!(mgr.list()[0].name, "Test Wallpaper");
    assert_eq!(mgr.list()[0].wp_type, "image");
    assert_eq!(mgr.list()[0].plugin_name, "test");

    let by_type = mgr.list_by_type("image");
    assert_eq!(by_type.len(), 1);

    let by_type_empty = mgr.list_by_type("video");
    assert!(by_type_empty.is_empty());

    // Identity is the DB item.id, assigned at sync time; this
    // scan-only test leaves it at 0, so look up by "0".
    let found = mgr.get("0");
    assert!(found.is_some());

    let plugins = mgr.plugins().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].name, "test");
    assert_eq!(plugins[0].version, "1.0");
}

#[test]
fn plugin_import_loads_plugin_local_modules() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("helpers")).unwrap();
    std::fs::write(
        dir.path().join("helpers/names.lua"),
        r#"
local M = {}
function M.name()
    return "Imported"
end
return M
"#,
    )
    .unwrap();
    let plugin_path = dir.path().join("main.lua");
    std::fs::write(
        &plugin_path,
        r#"
local names = import("helpers.names")
local M = {}
function M.info()
    return {
        name = "imported",
        capabilities = {
            source = { types = {"image"}, scan = true },
            discover = { search = true, details = true, download = true },
        },
    }
end
M.source = {}
function M.source.scan(ctx)
    return {
        { name = names.name(), wp_type = "image", resource = "/tmp/imported.png" },
    }
end
M.discover = {}
function M.discover.search(ctx, params)
    return {
        items = {
            { id = "abc", title = names.name(), preview_url = "", author = "" },
        },
        has_more = false,
    }
end
function M.discover.details(ctx, id)
    return {
        author = "Imported Author",
        description = names.name(),
        size = "42",
        width = 10,
        height = 20,
        tags = {"tag"},
        web_url = "https://example.invalid/item/" .. id,
    }
end
function M.discover.download(ctx, id)
    return {
        wp_type = "image",
        url = "https://example.invalid/" .. id,
        filename = id .. ".jpg",
        title = names.name(),
        tags = {"tag"},
        external_id = id,
        web_url = "https://example.invalid/item/" .. id,
        size = 42,
        width = 10,
        height = 20,
        content_rating = "Everyone",
    }
end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    let name = mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    assert_eq!(name, "imported");
    block(async { mgr.scan_all(&HashMap::new()).await.unwrap() });
    assert_eq!(mgr.list()[0].name, "Imported");

    let search =
        block_value(async { mgr.call_discover("imported", "", "", 1, &[]).await.unwrap() });
    assert_eq!(search.items[0].wp_type, "image");

    let dl = block_value(async { mgr.call_download("imported", "abc").await.unwrap() });
    assert_eq!(dl.wp_type, "image");
    assert_eq!(dl.filename, "abc.jpg");
    assert_eq!(dl.title, "Imported");
    assert_eq!(dl.tags, vec!["tag"]);
    assert_eq!(dl.external_id, "abc");
    assert_eq!(dl.web_url, "https://example.invalid/item/abc");
    assert_eq!(dl.size, Some(42));
    let detail = block_value(async { mgr.call_details("imported", "abc").await.unwrap() });
    assert_eq!(detail.width, Some(10));
    assert_eq!(detail.height, Some(20));
    assert_eq!(detail.web_url, "https://example.invalid/item/abc");
    assert_eq!(detail.author, "Imported Author");
}

#[test]
fn localized_text_keeps_owning_plugin_across_imports_and_properties() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("helpers")).unwrap();
    std::fs::write(
        dir.path().join("helpers/text.lua"),
        r#"
local M = {}
function M.library_label()
    return tr("Test Library")
end
return M
"#,
    )
    .unwrap();
    let plugin_path = dir.path().join("main.lua");
    std::fs::write(
        &plugin_path,
        r#"
local text = import("helpers.text")
local M = {}
function M.info()
    return {
        name = "localized",
        settings = {
            {
                key = "enabled",
                type = "bool",
                default = true,
                label = tr("Enabled"),
                group = "general",
                group_label = tr("General"),
            },
        },
        capabilities = {
            source = {
                types = {"image"},
                scan = true,
                library_label = text.library_label(),
                library_hint = tr("Request failed with HTTP %1", 503),
            },
            wallpaper = { properties = true },
        },
    }
end
M.source = {}
function M.source.scan(ctx)
    return {}
end
M.wallpaper = {}
function M.wallpaper.properties()
    return {
        mode = {
            text = tr("Mode"),
            type = "combo",
            value = "one",
            options = {
                { label = tr("One"), value = "one" },
            },
        },
    }
end
return M
"#,
    )
    .unwrap();

    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&plugin_path, "org.test.localized", "1.0", ENTRY_VERSION_V3)
        .unwrap();

    let plugins = manager.plugins().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].library_label.text(), "Test Library");
    assert_eq!(
        plugins[0].library_label.message().unwrap().plugin_id,
        "org.test.localized"
    );
    assert_eq!(
        plugins[0].library_hint.text(),
        "Request failed with HTTP %1"
    );
    assert_eq!(
        plugins[0]
            .library_hint
            .message()
            .unwrap()
            .arguments
            .as_slice(),
        ["503"]
    );
    assert_eq!(plugins[0].settings[0].label.text(), "Enabled");
    assert_eq!(
        plugins[0].settings[0].label.message().unwrap().plugin_id,
        "org.test.localized"
    );
    assert_eq!(plugins[0].settings[0].group, "general");
    assert_eq!(plugins[0].settings[0].group_label.text(), "General");
    assert_eq!(
        plugins[0].settings[0]
            .group_label
            .message()
            .unwrap()
            .plugin_id,
        "org.test.localized"
    );

    let entry: WallpaperEntry = serde_json::from_value(serde_json::json!({
        "name": "Wallpaper",
        "wp_type": "image",
        "resource": "/tmp/wallpaper.png",
        "preview": null
    }))
    .unwrap();
    let json = block_value(async { manager.call_properties("localized", &entry).await })
        .unwrap()
        .unwrap();
    let properties: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(properties["mode"]["text"], "Mode");
    assert_eq!(
        properties["mode"]["localized_text"]["pluginId"],
        "org.test.localized"
    );
    assert_eq!(properties["mode"]["localized_text"]["msgid"], "Mode");
    assert_eq!(properties["mode"]["options"][0]["label"], "One");
    assert_eq!(
        properties["mode"]["options"][0]["localized_label"]["msgid"],
        "One"
    );
}

#[test]
fn tr_rejects_invalid_runtime_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("main.lua");
    for expression in [r#"tr("Message", {})"#, r#"tr()"#, r#"tr({})"#, r#"tr("")"#] {
        std::fs::write(
            &plugin_path,
            format!(
                r#"
local M = {{}}
function M.info()
    return {{
        name = "invalid_tr",
        display_name = {expression},
        capabilities = {{}},
    }}
end
return M
"#
            ),
        )
        .unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&plugin_path, "org.test.invalid-tr", "1.0", ENTRY_VERSION_V3,)
            .is_err());
    }
}

#[test]
fn plugin_messages_are_namespaced_by_the_owning_vm() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SourceManager::new().unwrap();
    for (name, plugin_id) in [("first", "org.test.first"), ("second", "org.test.second")] {
        let root = dir.path().join(name);
        std::fs::create_dir(&root).unwrap();
        let entry = root.join("main.lua");
        std::fs::write(
            &entry,
            format!(
                r#"
local M = {{}}
function M.info()
    return {{
        name = "{name}",
        capabilities = {{
            source = {{
                types = {{"image"}},
                scan = true,
                library_label = tr("Status"),
            }},
        }},
    }}
end
M.source = {{}}
function M.source.scan(ctx) return {{}} end
return M
"#
            ),
        )
        .unwrap();
        manager
            .load_plugin(&entry, plugin_id, "1.0", ENTRY_VERSION_V3)
            .unwrap();
    }

    let mut plugins = manager.plugins().unwrap();
    plugins.sort_by(|left, right| left.name.cmp(&right.name));
    let first = plugins[0].library_label.message().unwrap();
    let second = plugins[1].library_label.message().unwrap();
    assert_eq!(first.msgid, "Status");
    assert_eq!(second.msgid, "Status");
    assert_eq!(first.plugin_id, "org.test.first");
    assert_eq!(second.plugin_id, "org.test.second");
}

#[test]
fn source_item_remove_works_without_scan_capability() {
    let dir = tempfile::tempdir().unwrap();
    let item_path = dir.path().join("wallpaper.png");
    std::fs::write(&item_path, b"image").unwrap();
    let plugin_path = dir.path().join("remove.lua");
    std::fs::write(
        &plugin_path,
        r#"
local M = {}
function M.info()
    return {
        name = "remove_only",
        capabilities = {},
    }
end
M.source = {}
function M.source.remove(ctx, item)
    if item.path ~= item.resource then error("path/resource mismatch") end
    if item.relative_path ~= "wallpaper.png" then error("wrong relative path") end
    if item.external_id ~= "ext-1" then error("missing external id") end
    ctx.remove_file(item.path)
end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    let name = mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    assert_eq!(name, "remove_only");
    assert!(mgr.supports_item_remove("remove_only"));

    let entry = WallpaperEntry {
        item_id: 42,
        name: "Wallpaper".to_string(),
        wp_type: "image".to_string(),
        resource: item_path.to_string_lossy().to_string(),
        preview: None,
        plugin_name: "remove_only".to_string(),
        library_root: dir.path().to_string_lossy().to_string(),
        description: None,
        tags: vec!["tag".to_string()],
        external_id: Some("ext-1".to_string()),
        web_url: None,
        size: None,
        width: None,
        height: None,
        content_rating: None,
        modified_at: None,
        create_at: 0,
    };
    let libraries = vec![entry.library_root.clone()];
    block_value(async { mgr.remove_item("remove_only", &entry, &libraries).await }).unwrap();
    assert!(!item_path.exists());
}

#[test]
fn source_item_remove_rejects_plugins_without_remove() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("no_remove.lua");
    std::fs::write(
        &plugin_path,
        r#"
local M = {}
function M.info()
    return {
        name = "no_remove",
        capabilities = {},
    }
end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    let name = mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    assert_eq!(name, "no_remove");
    assert!(!mgr.supports_item_remove("no_remove"));

    let entry = WallpaperEntry {
        item_id: 7,
        name: "Wallpaper".to_string(),
        wp_type: "image".to_string(),
        resource: "/tmp/wallpaper.png".to_string(),
        preview: None,
        plugin_name: "no_remove".to_string(),
        library_root: "/tmp".to_string(),
        description: None,
        tags: Vec::new(),
        external_id: None,
        web_url: None,
        size: None,
        width: None,
        height: None,
        content_rating: None,
        modified_at: None,
        create_at: 0,
    };
    let err = block_value(async { mgr.remove_item("no_remove", &entry, &[]).await }).unwrap_err();
    assert!(matches!(
        err,
        Error::SourceItemRemoveUnsupported(plugin) if plugin == "no_remove"
    ));
}

#[test]
fn plugin_import_rejects_path_escape() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("main.lua");
    std::fs::write(
        &plugin_path,
        r#"
local bad = import("../outside")
return bad
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    assert!(mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
        .is_err());
}

#[test]
fn entry_versions_v3_and_v4_are_supported_and_other_versions_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("main.lua");
    std::fs::write(
        &plugin_path,
        r#"
local M = {}
function M.info()
    return {
        name = "too_new",
        capabilities = {
            discover = { search = true },
        },
    }
end
M.discover = {}
function M.discover.search(ctx, params)
    return { items = {}, has_more = false }
end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    assert!(mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION_V3 - 1)
        .is_err());
    let mgr = SourceManager::new().unwrap();
    assert!(mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION_V3)
        .is_ok());
    let mgr = SourceManager::new().unwrap();
    assert!(mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION_V4)
        .is_ok());
    let mgr = SourceManager::new().unwrap();
    assert!(mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", LATEST_ENTRY_VERSION + 1,)
        .is_err());
}

#[test]
fn wallhaven_plugin_supports_optional_api_key_login() {
    let plugin_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("plugins/org.waywallen.wallhaven/main.lua");

    let mgr = SourceManager::new().unwrap();
    let name = mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION_V4)
        .unwrap();
    assert_eq!(name, "wallhaven");
    assert!(mgr.plugins().unwrap().is_empty());

    let sources = mgr.discover_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].plugin_id, "wallhaven");
    assert!(sources[0].supports_search);
    assert!(sources[0].filters.iter().any(|filter| {
        filter.ty == DiscoverFilterType::MultiSelect
            && filter.options.iter().any(|option| option.value == "Anime")
    }));
    let purity = sources[0]
        .filters
        .iter()
        .find(|filter| filter.id == "purity")
        .unwrap();
    assert_eq!(purity.ty, DiscoverFilterType::MultiSelect);
    assert_eq!(
        purity
            .options
            .iter()
            .map(|option| &option.value)
            .collect::<Vec<_>>(),
        ["SFW", "Sketchy", "NSFW"]
    );
    assert_eq!(
        purity.options[0].label,
        LocalizedText::translated("test.plugin", "Safe for work")
    );
    assert_eq!(
        purity.options[1].label,
        LocalizedText::translated("test.plugin", "Questionable")
    );
    assert_eq!(sources[0].actions[0].kind, SourceActionKind::Form);
    assert_eq!(sources[0].actions[0].fields.len(), 1);
    assert_eq!(sources[0].actions[0].fields[0].key, "api_key");
    assert!(sources[0].actions[0].fields[0].secret);
    assert!(sources[0].actions[0].fields[0].required);
    assert_eq!(
        block_value(async { mgr.check_lifecycle("wallhaven").await })
            .unwrap()
            .unwrap()
            .state,
        PluginLifecycleState::SignedOut
    );

    let runtime = mgr.test_runtime("wallhaven");
    let runtime = runtime.blocking_lock();
    let env = runtime
        .plugin_lua_env(plugin_path.parent().unwrap(), "org.waywallen.test")
        .unwrap();
    let import: LuaFunction = env.get("import").unwrap();
    let api: LuaTable = import.call("wallhaven.api").unwrap();
    let session: LuaTable = import.call("wallhaven.session").unwrap();
    let purity_masks: LuaTable = runtime
        .lua
        .load(
            r#"
return function(api, session)
    local function search(tags)
        local captured = nil
        local response = {}
        function response:status() return 200 end
        function response:ok() return true end
        function response:json() return { data = {}, meta = {} } end
        local request = {}
        function request:headers(_) return self end
        function request:query(value) captured = value return self end
        function request:timeout(_) return self end
        function request:send() return response end
        local ctx = { http = {} }
        function ctx.http:get(_) return request end
        api.search(ctx, { tags = tags, page = 1 })
        return captured.purity
    end

    local default_mask = search({})
    local sketchy_mask = search({ "Sketchy" })
    local nsfw_without_login = pcall(function() search({ "NSFW" }) end)
    session.load("wallhaven-session-v1\n3\nkey")
    local mixed_mask = search({ "SFW", "NSFW" })
    session.sign_out()
    return {
        default_mask = default_mask,
        sketchy_mask = sketchy_mask,
        nsfw_without_login = nsfw_without_login,
        mixed_mask = mixed_mask,
    }
end
"#,
        )
        .eval::<LuaFunction>()
        .unwrap()
        .call((api, session))
        .unwrap();
    assert_eq!(purity_masks.get::<String>("default_mask").unwrap(), "100");
    assert_eq!(purity_masks.get::<String>("sketchy_mask").unwrap(), "010");
    assert!(!purity_masks.get::<bool>("nsfw_without_login").unwrap());
    assert_eq!(purity_masks.get::<String>("mixed_mask").unwrap(), "101");

    let map: LuaTable = import.call("wallhaven.map").unwrap();
    let search_item: LuaFunction = map.get("search_item").unwrap();
    let item = runtime.lua.create_table().unwrap();
    item.set("id", "abc").unwrap();
    let mapped: LuaTable = search_item.call(item).unwrap();
    assert_eq!(mapped.get::<String>("wp_type").unwrap(), "image");

    let details: LuaFunction = map.get("details").unwrap();
    let detail = runtime.lua.create_table().unwrap();
    detail.set("url", "https://wallhaven.cc/w/abc123").unwrap();
    let mapped: LuaTable = details.call(detail.clone()).unwrap();
    assert_eq!(
        mapped.get::<String>("web_url").unwrap(),
        "https://wallhaven.cc/w/abc123"
    );

    let download: LuaFunction = map.get("download").unwrap();
    detail.set("id", "abc123").unwrap();
    let download_mapped: LuaTable = download.call(detail.clone()).unwrap();
    assert_eq!(
        download_mapped.get::<String>("web_url").unwrap(),
        "https://wallhaven.cc/w/abc123"
    );
    // A wallpaper Wallhaven reports no uploader for keeps the empty author
    // the listing has always produced.
    assert_eq!(mapped.get::<String>("author").unwrap(), "");

    let uploader = runtime.lua.create_table().unwrap();
    uploader.set("username", "Qtn").unwrap();
    detail.set("uploader", uploader).unwrap();
    let mapped: LuaTable = details.call(detail).unwrap();
    assert_eq!(mapped.get::<String>("author").unwrap(), "Qtn");
}

#[test]
fn call_resolve_relays_directory_item() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resolver.lua");
    std::fs::write(
        &path,
        r#"
local M = {}
function M.info()
    return { name = "resolver", capabilities = { discover = { search = true, resolve = true } } }
end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
function M.discover.resolve(ctx, params)
    return {
        name = "R " .. params.id,
        wp_type = "scene",
        resource = "scene.pkg",
        preview = "preview.jpg",
        description = "d",
        tags = { "t" },
        external_id = params.id,
        size = 7,
    }
end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    let got =
        block_value(async { mgr.call_resolve("resolver", "id1", "/some/dir").await }).unwrap();
    assert_eq!(got.name, "R id1");
    assert_eq!(got.wp_type, "scene");
    assert_eq!(got.resource, "scene.pkg");
    assert_eq!(got.preview.as_deref(), Some("preview.jpg"));
    assert_eq!(got.external_id, "id1");
    assert_eq!(got.size, Some(7));
}

#[test]
fn refresh_dynamic_tags_replaces_declared_tags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dyn.lua");
    std::fs::write(
        &path,
        r#"
local M = {}
function M.info()
    return {
        name = "dyn",
        capabilities = { discover = { search = true, tags = { "fallback" } } },
    }
end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
function M.discover.tags(ctx) return { "Live1", "Live2" } end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    // Before the refresh, discovery advertises the static fallback.
    assert_eq!(
        mgr.discover_sources().unwrap()[0].filters[0]
            .options
            .iter()
            .map(|option| option.value.clone())
            .collect::<Vec<_>>(),
        vec!["fallback"]
    );

    block_value(async { mgr.refresh_dynamic_tags().await });
    assert_eq!(
        mgr.discover_sources().unwrap()[0].filters[0]
            .options
            .iter()
            .map(|option| option.value.clone())
            .collect::<Vec<_>>(),
        vec!["Live1", "Live2"]
    );
}

#[test]
fn discover_filters_validate_selected_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("filters.lua");
    std::fs::write(
        &path,
        r#"
local M = {}
function M.info()
    return {
        name = "filters",
        capabilities = {
            discover = {
                search = true,
                filters = {
                    {
                        id = "kind",
                        title = "Kind",
                        type = "select",
                        options = {
                            { value = "A", label = "Alpha" },
                            { value = "B", label = "Beta" },
                        },
                    },
                    { id = "tags", title = "Tags", type = "multi_select", options = {
                        { value = "X", label = "X" }, { value = "Y", label = "Y" },
                    } },
                },
            },
        },
    }
end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION_V3)
        .unwrap();
    let kind = mgr.discover_sources().unwrap()[0]
        .filters
        .iter()
        .find(|filter| filter.id == "kind")
        .unwrap()
        .clone();
    assert_eq!(kind.options[0].label, LocalizedText::raw("Alpha"));
    assert_eq!(kind.options[1].label, LocalizedText::raw("Beta"));
    assert!(block_value(async {
        mgr.call_discover("filters", "", "", 1, &["A".to_string(), "X".to_string()])
            .await
    })
    .is_ok());
    assert!(block_value(async {
        mgr.call_discover("filters", "", "", 1, &["A".to_string(), "B".to_string()])
            .await
    })
    .is_err());
    assert!(block_value(async {
        mgr.call_discover("filters", "", "", 1, &["unknown".to_string()])
            .await
    })
    .is_err());
}

#[test]
fn discover_filters_require_object_options() {
    let dir = tempfile::tempdir().unwrap();
    for (name, options) in [
        ("raw", "{ \"A\", \"B\" }"),
        ("missing_label", "{ { value = \"A\" } }"),
        ("empty_label", "{ { value = \"A\", label = \"\" } }"),
        (
            "duplicate",
            "{ { value = \"A\", label = \"A\" }, { value = \"A\", label = \"B\" } }",
        ),
    ] {
        let path = dir.path().join(format!("{name}.lua"));
        std::fs::write(
            &path,
            format!(
                r#"
local M = {{}}
function M.info()
    return {{
        name = "{name}",
        capabilities = {{
            discover = {{
                search = true,
                filters = {{ {{
                    id = "kind",
                    title = "Kind",
                    type = "select",
                    options = {options},
                }} }},
            }},
        }},
    }}
end
M.discover = {{}}
function M.discover.search(ctx, params) return {{ items = {{}}, has_more = false }} end
return M
"#
            ),
        )
        .unwrap();

        let mgr = SourceManager::new().unwrap();
        assert!(mgr
            .load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION_V3)
            .is_err());
    }
}

#[test]
fn discover_filters_accept_legacy_values_only_for_v3() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy_filters.lua");
    std::fs::write(
        &path,
        r#"
local M = {}
function M.info()
    return {
        name = "legacy_filters",
        capabilities = {
            discover = {
                search = true,
                filters = { {
                    id = "type",
                    title = "Type",
                    type = "multi_select",
                    values = { "Scene", "Video" },
                } },
            },
        },
    }
end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION_V3)
        .unwrap();
    let options = &mgr.discover_sources().unwrap()[0].filters[0].options;
    assert_eq!(options.len(), 2);
    assert_eq!(options[0].value, "Scene");
    assert_eq!(options[0].label, LocalizedText::raw("Scene"));
    assert_eq!(options[1].value, "Video");
    assert_eq!(options[1].label, LocalizedText::raw("Video"));
    assert!(block_value(async {
        mgr.call_discover("legacy_filters", "", "", 1, &["Scene".to_string()])
            .await
    })
    .is_ok());

    let error = SourceManager::new()
        .unwrap()
        .load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION_V4)
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("values is only supported for entry_version 3"));
}

#[test]
fn discover_filters_reject_options_with_legacy_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ambiguous_filters.lua");
    std::fs::write(
        &path,
        r#"
local M = {}
function M.info()
    return {
        name = "ambiguous_filters",
        capabilities = {
            discover = {
                search = true,
                filters = { {
                    id = "type",
                    title = "Type",
                    type = "multi_select",
                    options = { { value = "Scene", label = "Scene" } },
                    values = { "Scene" },
                } },
            },
        },
    }
end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
return M
"#,
    )
    .unwrap();

    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION_V3)
        .is_err());
}

#[test]
fn remote_discovery_context_omits_filesystem_mutation() {
    let plugin = tempfile::tempdir().unwrap();
    let path = plugin.path().join("guarded.lua");
    std::fs::write(
        &path,
        r#"
local M = {}
function M.info()
    return { name = "guarded", capabilities = { discover = { search = true } } }
end
M.discover = {}
function M.discover.search(ctx, params)
    local parsed = ctx.json.parse('{"ok":true}')
    ctx.http:set_cookie(
        "https://example.com/",
        "fixture=cookie; Domain=example.com; Path=/; Secure"
    )
    local slept = pcall(function() ctx.time.sleep(0) end)
    return {
        items = { {
            id = tostring(
                ctx.remove_dir == nil and ctx.libraries == nil and ctx.fs == nil
                and ctx.config ~= nil and ctx.json ~= nil
                and ctx.base64 ~= nil and ctx.time ~= nil and ctx.random ~= nil
                and parsed.ok and ctx.json_parse('{"ok":true}').ok
                and ctx.json.encode({1, 2}) == "[1,2]"
                and ctx.json_encode({1, 2}) == "[1,2]"
                and ctx.base64.decode("d2FsbA==") == "wall"
                and ctx.base64_decode("d2FsbA==") == "wall"
                and ctx.time.unix() > 1000000000
                and ctx.time_unix() > 1000000000
                and #ctx.random.hex(12) == 24
                and slept
                and ctx.url.decode_component("wall%7Cpaper") == "wall|paper"
                and ctx.http:cookie("https://example.com/", "fixture") == "cookie"
            ),
            title = "",
            preview_url = "",
            author = "",
        } },
        has_more = false,
    }
end
return M
"#,
    )
    .unwrap();
    let mgr = SourceManager::new().unwrap();
    mgr.load_plugin(&path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    let r = block_value(async { mgr.call_discover("guarded", "", "", 1, &[]).await }).unwrap();
    assert_eq!(r.items[0].id, "true");
}

#[test]
fn v3_lifecycle_actions_qrlogin_and_subscription_share_plugin_owned_state() {
    let dir = tempfile::tempdir().unwrap();
    let state_root = dir.path().join("state");
    std::fs::write(dir.path().join("legacy-session.json"), "legacy secret").unwrap();
    let state_store = crate::plugin::state_store::PluginStateStore::new(
        state_root.clone(),
        dir.path().to_path_buf(),
    );
    let manager =
        SourceManager::with_probe_and_state_store(Arc::new(AvFormatProbe::new()), state_store)
            .unwrap();
    let plugin_path = dir.path().join("main.lua");
    std::fs::write(
        &plugin_path,
        r#"
local M = {}
local signed_in = false
local display = ""
local subscriptions = {}

function M.info()
    return {
        name = "account_provider",
        capabilities = {
            discover = { search = true, subscription = true },
        },
        actions = {
            {
                id = "sign_in",
                kind = "qr_login",
                label = "Log in",
                description = "Open the account app",
                browse_description = "Log in to browse this source",
                browse_button_label = "Continue",
            },
            { id = "sign_out", kind = "invoke" },
            {
                id = "set_alias",
                kind = "form",
                fields = {
                    { key = "alias", label = "Alias", required = true },
                },
            },
        },
        status = { { id = "account" } },
        state_migrations = {
            { schema_id = "legacy-session-v1", file = "legacy-session.json" },
        },
    }
end

M.lifecycle = {}
function M.lifecycle.load(blob)
    if blob == nil then return end
    local flag, value = string.match(blob, "^([^|]+)|(.*)$")
    signed_in = flag == "1"
    display = value or ""
end
function M.lifecycle.save()
    return (signed_in and "1" or "0") .. "|" .. display
end
function M.lifecycle.check(ctx)
    return {
        state = signed_in and "signed_in" or "signed_out",
        display_value = display,
        avatar_url = "https://example.invalid/avatar.png",
    }
end
function M.lifecycle.migrate(schema_id, raw)
    if schema_id ~= "legacy-session-v1" or raw ~= "legacy secret" then
        error("wrong migration input")
    end
    return "0|migrated"
end

M.actions = {}
function M.actions.status(ctx)
    return {
        status = { account = tr("Account: %1", display) },
        actions = {
            sign_in = { visible = not signed_in, enabled = not signed_in },
            sign_out = { visible = signed_in, enabled = signed_in },
        },
    }
end
function M.actions.invoke(ctx, action_id, values)
    if action_id == "sign_out" then
        signed_in = false
        display = ""
    elseif action_id == "set_alias" then
        display = values.alias
    else
        error("unexpected action")
    end
end

M.qrlogin = {}
function M.qrlogin.begin(ctx, action_id)
    return {
        key = { polls = 0 },
        challenge = "https://example.invalid/challenge",
        poll_after_ms = 25,
        expires_in_ms = 1000,
        title = tr("Sign in"),
        instruction = tr("Scan with %1", "phone"),
    }
end
function M.qrlogin.poll(ctx, key)
    key.polls = key.polls + 1
    if key.polls == 1 then
        return {
            state = "awaiting_confirmation",
            display_value = tr("Approve on %1", "phone"),
        }
    end
    signed_in = true
    display = "alice"
    return { state = "succeeded", display_value = display }
end
function M.qrlogin.cancel(ctx, key)
    key.cancelled = true
end

M.discover = {}
function M.discover.search(ctx, params)
    return { items = {}, has_more = false }
end

M.subscription = {}
function M.subscription.status(ctx, ids)
    local result = {}
    for _, id in ipairs(ids) do result[id] = subscriptions[id] or "unknown" end
    if ids[1] == "status-error" then
        return { items = result, error = tr("Request failed with HTTP %1", 503) }
    end
    return result
end
function M.subscription.subscribe(ctx, id)
    if id == "rejected" then
        return { accepted = false, error = tr("Denied %1", id) }
    end
    subscriptions[id] = "subscribed"
    return { accepted = true }
end
function M.subscription.unsubscribe(ctx, id)
    subscriptions[id] = "unsubscribed"
    return { accepted = true }
end

return M
"#,
    )
    .unwrap();

    manager
        .load_plugin(&plugin_path, "org.test", "1.0", ENTRY_VERSION_V3)
        .unwrap();
    assert!(dir.path().join("legacy-session.migrated.bak").is_file());
    assert_eq!(
        std::fs::read_to_string(state_root.join("org.test.state")).unwrap(),
        "0|migrated"
    );
    assert_eq!(
        manager.discover_sources().unwrap()[0].remote_capability,
        Some(RemoteCapability::Subscription)
    );

    let sources = block_value(async { manager.discover_sources_with_status().await }).unwrap();
    assert_eq!(sources[0].status[0].value.text(), "Account: %1");
    assert_eq!(
        sources[0].status[0].value.message().unwrap().arguments,
        ["migrated"]
    );
    assert_eq!(sources[0].avatar_url, "https://example.invalid/avatar.png");
    assert_eq!(sources[0].actions[0].label.text(), "Log in");
    assert_eq!(
        sources[0].actions[0].description.text(),
        "Open the account app"
    );
    assert_eq!(sources[0].actions[0].browse_button_label.text(), "Continue");
    assert_eq!(
        sources[0].actions[0].browse_description.text(),
        "Log in to browse this source"
    );
    assert!(sources[0].actions[0].label.message().is_none());
    assert!(sources[0].actions[0].visible);
    assert!(!sources[0].actions[1].visible);
    assert_eq!(sources[0].actions[2].fields[0].key, "alias");

    let begin =
        block_value(async { manager.begin_qr_login("account_provider", "sign_in").await }).unwrap();
    assert_eq!(begin.challenge, "https://example.invalid/challenge");
    assert_eq!(begin.poll_after_ms, 25);
    assert_eq!(begin.title.message().unwrap().msgid, "Sign in");
    assert_eq!(begin.instruction.message().unwrap().arguments, ["phone"]);
    let first = block_value(async {
        manager
            .poll_qr_login("account_provider", begin.operation_id)
            .await
    })
    .unwrap();
    assert_eq!(first.state, QrLoginPollState::AwaitingConfirmation);
    assert_eq!(first.display_value.message().unwrap().arguments, ["phone"]);
    let second = block_value(async {
        manager
            .poll_qr_login("account_provider", begin.operation_id)
            .await
    })
    .unwrap();
    assert_eq!(second.state, QrLoginPollState::Succeeded);
    assert_eq!(
        block_value(async { manager.check_lifecycle("account_provider").await })
            .unwrap()
            .unwrap()
            .state,
        PluginLifecycleState::SignedIn
    );
    assert_eq!(
        std::fs::read_to_string(state_root.join("org.test.state")).unwrap(),
        "1|alice"
    );

    let ids = vec![
        "item".to_string(),
        "missing".to_string(),
        "items".to_string(),
    ];
    let before =
        block_value(async { manager.subscription_status("account_provider", &ids).await }).unwrap();
    assert!(before
        .items
        .iter()
        .all(|item| item.state == SubscriptionState::Unknown));
    assert!(
        block_value(async {
            manager
                .set_subscription("account_provider", "item", true)
                .await
        })
        .unwrap()
        .accepted
    );
    let subscribed =
        block_value(async { manager.subscription_status("account_provider", &ids).await }).unwrap();
    assert_eq!(subscribed.items[0].state, SubscriptionState::Subscribed);
    assert_eq!(subscribed.items[1].state, SubscriptionState::Unknown);
    let status_error_ids = vec!["status-error".to_string()];
    let status_error = block_value(async {
        manager
            .subscription_status("account_provider", &status_error_ids)
            .await
    })
    .unwrap();
    assert_eq!(status_error.error.message().unwrap().arguments, ["503"]);
    let rejected = block_value(async {
        manager
            .set_subscription("account_provider", "rejected", true)
            .await
    })
    .unwrap();
    assert!(!rejected.accepted);
    assert_eq!(rejected.error.text(), "Denied %1");
    assert_eq!(rejected.error.message().unwrap().arguments, ["rejected"]);
    block_value(async {
        manager
            .set_subscription("account_provider", "item", false)
            .await
    })
    .unwrap();
    let unsubscribed =
        block_value(async { manager.subscription_status("account_provider", &ids).await }).unwrap();
    assert_eq!(unsubscribed.items[0].state, SubscriptionState::Unsubscribed);

    let missing = block_value(async {
        manager
            .invoke_action("account_provider", "set_alias", &HashMap::new())
            .await
    });
    assert!(missing
        .unwrap_err()
        .to_string()
        .contains("requires field 'alias'"));
    let values = HashMap::from([("alias".to_string(), "configured".to_string())]);
    block_value(async {
        manager
            .invoke_action("account_provider", "set_alias", &values)
            .await
    })
    .unwrap();
    assert_eq!(
        block_value(async { manager.check_lifecycle("account_provider").await })
            .unwrap()
            .unwrap()
            .display_value
            .text(),
        "configured"
    );

    block_value(async {
        manager
            .invoke_action("account_provider", "sign_out", &HashMap::new())
            .await
    })
    .unwrap();
    assert_eq!(
        block_value(async { manager.check_lifecycle("account_provider").await })
            .unwrap()
            .unwrap()
            .state,
        PluginLifecycleState::SignedOut
    );
}

#[test]
fn supported_remote_capabilities_are_mutually_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.lua");
    let script = |flags: &str, api: &str| {
        format!(
            r#"
local M = {{}}
function M.info()
    return {{ name = "remote", capabilities = {{ discover = {{ search = true, {flags} }} }} }}
end
M.discover = {{}}
function M.discover.search(ctx, params) return {{ items = {{}}, has_more = false }} end
{api}
return M
"#
        )
    };

    std::fs::write(&path, script("", "")).unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
        .is_ok());

    let orphan_details = "function M.discover.details(ctx, id) return {} end";
    std::fs::write(&path, script("", orphan_details)).unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
        .is_err());
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V4)
        .is_err());

    let orphan_actions = "M.actions = {}\nfunction M.actions.status(ctx) return {} end";
    std::fs::write(&path, script("", orphan_actions)).unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
        .is_err());

    std::fs::write(
        &path,
        script(
            "download = true",
            "function M.discover.download(ctx, id) return { wp_type = \"image\" } end",
        ),
    )
    .unwrap();
    let download_manager = SourceManager::new().unwrap();
    download_manager
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
        .unwrap();
    assert!(block_value(async {
        download_manager
            .subscription_status("remote", &["item".to_string()])
            .await
    })
    .is_err());

    let subscription_api = r#"
M.subscription = {}
function M.subscription.status(ctx, ids) return {} end
function M.subscription.subscribe(ctx, id) return {} end
        function M.subscription.unsubscribe(ctx, id) return {} end
"#;
    std::fs::write(&path, script("subscription = true", subscription_api)).unwrap();
    let subscription_manager = SourceManager::new().unwrap();
    subscription_manager
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
        .unwrap();
    assert!(
        block_value(async { subscription_manager.call_download("remote", "item").await }).is_err()
    );

    std::fs::write(
        &path,
        script(
            "subscription = true",
            &format!("function M.discover.download(ctx, id) return {{}} end\n{subscription_api}"),
        ),
    )
    .unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
        .is_err());

    std::fs::write(
        &path,
        script(
            "download = true",
            &format!("function M.discover.download(ctx, id) return {{}} end\n{subscription_api}"),
        ),
    )
    .unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
        .is_err());

    std::fs::write(
            &path,
            script(
                "download = true, subscription = true",
                &format!(
                    "function M.discover.download(ctx, id) return {{ wp_type = \"image\" }} end\n{subscription_api}"
                ),
            ),
        )
        .unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V3)
        .is_err());
}

#[test]
fn failed_state_migration_preserves_the_legacy_blob() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("legacy.json");
    std::fs::write(&legacy, "legacy secret").unwrap();
    let state_root = dir.path().join("state");
    let state_store = crate::plugin::state_store::PluginStateStore::new(
        state_root.clone(),
        dir.path().to_path_buf(),
    );
    let manager =
        SourceManager::with_probe_and_state_store(Arc::new(AvFormatProbe::new()), state_store)
            .unwrap();
    let entry = dir.path().join("main.lua");
    std::fs::write(
        &entry,
        r#"
local M = {}
function M.info()
    return {
        name = "migration_failure",
        capabilities = {},
        state_migrations = { { schema_id = "legacy", file = "legacy.json" } },
    }
end
M.lifecycle = {}
function M.lifecycle.load(blob) end
function M.lifecycle.save() return "new" end
function M.lifecycle.check(ctx) return { state = "signed_out" } end
function M.lifecycle.migrate(schema_id, raw) error("migration rejected") end
return M
"#,
    )
    .unwrap();

    assert!(manager
        .load_plugin(&entry, "org.failure", "1", ENTRY_VERSION_V3)
        .is_err());
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "legacy secret");
    assert!(!dir.path().join("legacy.migrated.bak").exists());
    assert!(!state_root.join("org.failure.state").exists());
}

#[test]
fn invalid_qr_begin_result_does_not_retain_the_opaque_key() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.lua");
    std::fs::write(
        &entry,
        r#"
local M = {}
function M.info()
    return {
        name = "invalid_qr",
        capabilities = {},
        actions = { { id = "sign_in", kind = "qr_login" } },
    }
end
M.actions = {}
function M.actions.status(ctx) return { actions = {} } end
M.qrlogin = {}
function M.qrlogin.begin(ctx, action_id) return { key = {} } end
function M.qrlogin.poll(ctx, key) return { state = "awaiting_scan" } end
return M
"#,
    )
    .unwrap();
    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&entry, "org.invalid", "1", ENTRY_VERSION_V3)
        .unwrap();
    assert!(block_value(async { manager.begin_qr_login("invalid_qr", "sign_in").await }).is_err());
    assert!(manager
        .test_runtime("invalid_qr")
        .blocking_lock()
        .qr_operations
        .is_empty());
}

#[test]
fn subscription_mutation_does_not_scan_source_libraries() {
    let dir = tempfile::tempdir().unwrap();
    let state_root = dir.path().join("state");
    let state_store = crate::plugin::state_store::PluginStateStore::new(
        state_root.clone(),
        dir.path().to_path_buf(),
    );
    let manager =
        SourceManager::with_probe_and_state_store(Arc::new(AvFormatProbe::new()), state_store)
            .unwrap();
    let entry = dir.path().join("main.lua");
    std::fs::write(
        &entry,
        r#"
local M = {}
local scans = 0
function M.info()
    return {
        name = "separate_flows",
        capabilities = {
            source = { scan = true, types = { "image" } },
            discover = { search = true, subscription = true },
        },
    }
end
M.lifecycle = {}
function M.lifecycle.load(blob) end
function M.lifecycle.save() return tostring(scans) end
function M.lifecycle.check(ctx)
    return { state = "signed_in", display_value = "test" }
end
M.source = {}
function M.source.scan(ctx) scans = scans + 1 return {} end
M.discover = {}
function M.discover.search(ctx, params) return { items = {}, has_more = false } end
M.subscription = {}
function M.subscription.status(ctx, ids)
    local result = {}
    for _, id in ipairs(ids) do result[id] = "unknown" end
    return result
end
function M.subscription.subscribe(ctx, id) return { accepted = true } end
function M.subscription.unsubscribe(ctx, id) return { accepted = true } end
return M
"#,
    )
    .unwrap();
    manager
        .load_plugin(&entry, "org.test", "1", ENTRY_VERSION_V3)
        .unwrap();

    let subscription_entry = WallpaperEntry {
        item_id: 1,
        name: "Workshop item".to_string(),
        wp_type: "image".to_string(),
        resource: "item.jpg".to_string(),
        preview: None,
        description: None,
        tags: Vec::new(),
        external_id: Some("item".to_string()),
        web_url: None,
        size: None,
        width: None,
        height: None,
        content_rating: None,
        modified_at: None,
        create_at: 0,
        plugin_name: "separate_flows".to_string(),
        library_root: String::new(),
    };
    assert!(manager.supports_item_unsubscribe(&subscription_entry));
    assert!(!manager.supports_item_unsubscribe(&WallpaperEntry {
        external_id: None,
        ..subscription_entry
    }));

    block_value(async {
        manager
            .set_subscription("separate_flows", "item", true)
            .await
    })
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(state_root.join("org.test.state")).unwrap(),
        "0"
    );
}

#[test]
fn plugin_http_sessions_are_isolated_persisted_and_cleared_by_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let state_root = dir.path().join("state");
    let state_store = crate::plugin::state_store::PluginStateStore::new(
        state_root.clone(),
        dir.path().to_path_buf(),
    );
    let script = |name: &str, cookie: &str| {
        format!(
            r#"
local M = {{}}
function M.info()
    return {{
        name = "{name}",
        capabilities = {{ discover = {{ search = true }} }},
        actions = {{ {{ id = "sign_out", kind = "invoke" }} }},
    }}
end
M.discover = {{}}
function M.discover.search(ctx, params)
    if params.query == "set" then
        ctx.http:set_cookie(
            "https://example.com/",
            "session={cookie}; Domain=example.com; Path=/; Secure"
        )
    elseif params.query == "fail" then
        ctx.http:set_cookie(
            "https://example.com/",
            "session=failed; Domain=example.com; Path=/; Secure"
        )
        error("callback failed")
    end
    return {{
        items = {{ {{
            id = ctx.http:cookie("https://example.com/", "session") or "none",
            title = "",
            preview_url = "",
            author = "",
        }} }},
        has_more = false,
    }}
end
M.actions = {{}}
function M.actions.status(ctx)
    return {{ actions = {{ sign_out = {{ visible = true, enabled = true }} }} }}
end
function M.actions.invoke(ctx, action_id)
    if action_id ~= "sign_out" then error("unexpected action") end
    ctx.http:clear_cookies()
end
return M
"#
        )
    };
    let first_path = dir.path().join("first.lua");
    let second_path = dir.path().join("second.lua");
    std::fs::write(&first_path, script("first", "first")).unwrap();
    std::fs::write(&second_path, script("second", "second")).unwrap();

    let manager = SourceManager::with_probe_and_state_store(
        Arc::new(AvFormatProbe::new()),
        state_store.clone(),
    )
    .unwrap();
    manager
        .load_plugin(&first_path, "org.first", "1", ENTRY_VERSION_V3)
        .unwrap();
    manager
        .load_plugin(&second_path, "org.second", "1", ENTRY_VERSION_V3)
        .unwrap();
    let first =
        block_value(async { manager.call_discover("first", "set", "", 1, &[]).await }).unwrap();
    assert_eq!(first.items[0].id, "first");
    let second =
        block_value(async { manager.call_discover("second", "", "", 1, &[]).await }).unwrap();
    assert_eq!(second.items[0].id, "none");
    assert!(state_root.join("org.first.cookies").is_file());
    assert!(!state_root.join("org.first.state").exists());

    let failed = block_value(async { manager.call_discover("first", "fail", "", 1, &[]).await });
    assert!(failed.is_err());

    let restored = SourceManager::with_probe_and_state_store(
        Arc::new(AvFormatProbe::new()),
        state_store.clone(),
    )
    .unwrap();
    restored
        .load_plugin(&first_path, "org.first", "1", ENTRY_VERSION_V3)
        .unwrap();
    let after_restart =
        block_value(async { restored.call_discover("first", "", "", 1, &[]).await }).unwrap();
    assert_eq!(after_restart.items[0].id, "failed");

    block_value(async {
        restored
            .invoke_action("first", "sign_out", &HashMap::new())
            .await
    })
    .unwrap();
    let signed_out =
        SourceManager::with_probe_and_state_store(Arc::new(AvFormatProbe::new()), state_store)
            .unwrap();
    signed_out
        .load_plugin(&first_path, "org.first", "1", ENTRY_VERSION_V3)
        .unwrap();
    let after_sign_out =
        block_value(async { signed_out.call_discover("first", "", "", 1, &[]).await }).unwrap();
    assert_eq!(after_sign_out.items[0].id, "none");
}

#[tokio::test]
async fn plugins_have_independent_runtime_locks() {
    let dir = tempfile::tempdir().unwrap();
    let script = |name: &str| {
        format!(
            r#"
local M = {{}}
function M.info()
    return {{
        name = "{name}",
        capabilities = {{ discover = {{ search = true }} }},
    }}
end
M.discover = {{}}
function M.discover.search(ctx, params)
    return {{ items = {{}}, has_more = false }}
end
return M
"#,
        )
    };
    let first = dir.path().join("first.lua");
    let second = dir.path().join("second.lua");
    std::fs::write(&first, script("first")).unwrap();
    std::fs::write(&second, script("second")).unwrap();
    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&first, "org.first", "1", ENTRY_VERSION_V3)
        .unwrap();
    manager
        .load_plugin(&second, "org.second", "1", ENTRY_VERSION_V3)
        .unwrap();

    let first_handle = manager.handle("first").unwrap();
    let first_guard = first_handle.runtime.lock().await;
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        manager.call_discover("second", "", "", 1, &[]),
    )
    .await
    .is_ok());
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        manager.call_discover("first", "", "", 1, &[]),
    )
    .await
    .is_err());
    drop(first_guard);
}

#[test]
fn plugin_registry_replacement_keeps_current_runtime_until_candidate_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let script = |name: &str| {
        format!(
            r#"
local M = {{}}
function M.info()
    return {{ name = "{name}", capabilities = {{ discover = {{ search = true }} }} }}
end
M.discover = {{}}
function M.discover.search(ctx, params) return {{ items = {{}}, has_more = false }} end
return M
"#
        )
    };
    let old_path = dir.path().join("old.lua");
    let new_path = dir.path().join("new.lua");
    let invalid_path = dir.path().join("invalid.lua");
    std::fs::write(&old_path, script("old")).unwrap();
    std::fs::write(&new_path, script("new")).unwrap();
    std::fs::write(&invalid_path, "error('invalid replacement')").unwrap();

    let current = SourceManager::new().unwrap();
    current
        .load_plugin(&old_path, "org.old", "1", ENTRY_VERSION_V3)
        .unwrap();
    block_value(current.suspend_plugins());
    let invalid = SourceManager::new().unwrap();
    assert!(invalid
        .load_plugin(&invalid_path, "org.new", "1", ENTRY_VERSION_V3)
        .is_err());
    assert_eq!(current.discover_sources().unwrap()[0].name, "old");
    assert!(block_value(async { current.call_discover("old", "", "", 1, &[]).await }).is_err());
    current.resume_plugins();
    assert!(block_value(async { current.call_discover("old", "", "", 1, &[]).await }).is_ok());

    let replacement = SourceManager::new().unwrap();
    replacement
        .load_plugin(&new_path, "org.new", "1", ENTRY_VERSION_V3)
        .unwrap();
    replacement
        .retain_plugins_from(&current, &HashSet::from(["org.old".to_string()]))
        .unwrap();
    block_value(current.suspend_plugins());
    current.replace_plugins(replacement).unwrap();
    assert_eq!(
        current
            .discover_sources()
            .unwrap()
            .into_iter()
            .map(|source| source.name)
            .collect::<Vec<_>>(),
        vec!["new", "old"]
    );
}

#[tokio::test]
async fn lua_callbacks_have_a_host_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("deadline.lua");
    std::fs::write(
        &entry,
        r#"
local M = {}
function M.info()
    return { name = "deadline", capabilities = { discover = { search = true } } }
end
M.discover = {}
function M.discover.search(ctx, params)
    while true do end
end
return M
"#,
    )
    .unwrap();

    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&entry, "org.deadline", "1", ENTRY_VERSION_V3)
        .unwrap();
    manager
        .set_test_callback_timeout("deadline", Duration::from_millis(40))
        .await;

    let error = tokio::time::timeout(
        Duration::from_secs(1),
        manager.call_discover("deadline", "", "", 1, &[]),
    )
    .await
    .expect("host deadline did not interrupt Lua")
    .unwrap_err();
    assert!(error.to_string().contains("timed out"));
}

#[test]
fn video_source_plugin_discovers_video_files() {
    let lib = tempfile::tempdir().unwrap();
    let nested = lib.path().join("album");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(lib.path().join("clip.MP4"), b"video bytes").unwrap();
    std::fs::write(lib.path().join("animated.gif"), b"image source owns gif").unwrap();
    std::fs::write(nested.join("poster.png"), b"not a video").unwrap();
    std::fs::write(nested.join("loop.webm"), b"more video bytes").unwrap();

    let plugin_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("plugins/org.waywallen.video/main.lua");

    let mgr = SourceManager::new().unwrap();
    let name = mgr
        .load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION)
        .unwrap();
    assert_eq!(name, "video");

    let mut libs = HashMap::new();
    libs.insert(
        "video".to_string(),
        vec![lib.path().to_string_lossy().to_string()],
    );
    block(async { mgr.scan_all(&libs).await.unwrap() });

    let entries = mgr.list();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e.wp_type == "video"));
    assert!(entries.iter().all(|e| e.plugin_name == "video"));
    assert!(entries.iter().all(|e| e.preview.is_none()));
    assert!(entries.iter().all(|e| e.size.is_some()));
    assert!(entries.iter().all(|e| e.width.is_none()));
    assert!(entries.iter().all(|e| e.height.is_none()));
    assert!(entries.iter().all(|e| e.content_rating.is_none()));
    // SPAWN_VERSION 3 keeps the canonical resource path in
    // `entry.resource`.

    let clip_path = lib.path().join("clip.MP4").to_string_lossy().to_string();
    let clip = entries
        .iter()
        .find(|entry| entry.resource == clip_path)
        .unwrap()
        .clone();
    assert_eq!(clip.name, "clip");
    assert_eq!(clip.resource, clip_path);

    let apply = block_value(async { mgr.call_apply("video", &clip).await.unwrap() });
    assert_eq!(apply.extras.get("path"), Some(&clip.resource));
    assert!(apply.default_user_properties.is_empty());

    assert_eq!(mgr.list_by_type("video").len(), 2);
    assert!(mgr.list_by_type("image").is_empty());
}

#[test]
fn wallpaper_apply_returns_resources_and_authored_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("main.lua");
    std::fs::write(
        &plugin_path,
        r#"
local M = {}
function M.info()
    return {
        name = "apply_test",
        capabilities = { wallpaper = { apply = true } },
    }
end
M.wallpaper = {}
function M.wallpaper.apply(entry, ctx)
    return {
        extras = { path = entry.resource, token = "resource" },
        default_user_properties = {
            ["waywallen.scheme_color"] = "0.1 0.2 0.3",
        },
    }
end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    mgr.load_plugin(&plugin_path, "test.plugin", "1.0", ENTRY_VERSION_V3)
        .unwrap();
    let entry = WallpaperEntry {
        item_id: 1,
        name: "Wallpaper".into(),
        wp_type: "video".into(),
        resource: "/tmp/wallpaper.mp4".into(),
        preview: None,
        description: None,
        tags: Vec::new(),
        external_id: None,
        web_url: None,
        size: None,
        width: None,
        height: None,
        content_rating: None,
        modified_at: None,
        create_at: 0,
        plugin_name: "apply_test".into(),
        library_root: "/tmp".into(),
    };

    let apply = block_value(async { mgr.call_apply("apply_test", &entry).await.unwrap() });
    assert_eq!(apply.extras.get("path"), Some(&entry.resource));
    assert_eq!(
        apply.extras.get("token").map(String::as_str),
        Some("resource")
    );
    assert_eq!(
        apply
            .default_user_properties
            .get("waywallen.scheme_color")
            .map(String::as_str),
        Some("0.1 0.2 0.3")
    );
}

#[tokio::test]
async fn plugin_oom_is_catchable_reusable_and_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let heavy_path = dir.path().join("heavy.lua");
    let light_path = dir.path().join("light.lua");
    // `heavy` builds a large table only when the query is "overflow"; `light` is trivial.
    std::fs::write(
        &heavy_path,
        r#"
local M = {}
function M.info()
    return { name = "heavy", capabilities = { discover = { search = true } } }
end
M.discover = {}
function M.discover.search(ctx, params)
    if params.query == "overflow" then
        local t = {}
        local chunk = string.rep("x", 1024 * 1024)
        for i = 1, 512 do t[i] = chunk .. tostring(i) end
    end
    return { items = {}, has_more = false }
end
return M
"#,
    )
    .unwrap();
    std::fs::write(
        &light_path,
        r#"
local M = {}
function M.info()
    return { name = "light", capabilities = { discover = { search = true } } }
end
M.discover = {}
function M.discover.search(ctx, params)
    return { items = {}, has_more = false }
end
return M
"#,
    )
    .unwrap();

    let mgr = SourceManager::new().unwrap();
    mgr.load_plugin(&heavy_path, "org.heavy", "1", ENTRY_VERSION_V3)
        .unwrap();
    mgr.load_plugin(&light_path, "org.light", "1", ENTRY_VERSION_V3)
        .unwrap();

    // Tighten ONLY the heavy VM's cap just above its live usage so the over-allocation
    // trips it deterministically (keyed off used_memory, not a fixed baseline).
    {
        let rt = mgr.test_runtime("heavy");
        let guard = rt.lock().await;
        let cap = guard.lua.used_memory() + 8 * 1024 * 1024;
        guard.lua.set_memory_limit(cap).unwrap();
    }

    // The overrun surfaces as a typed Err (mlua MemoryError -> Error::DiscoverFailed),
    // not a panic/abort; the redacted message carries "memory".
    let err = mgr
        .call_discover("heavy", "overflow", "", 1, &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::DiscoverFailed { .. }),
        "unexpected error: {err:?}"
    );
    assert!(
        err.to_string().to_lowercase().contains("memory"),
        "message: {err}"
    );

    // The SAME plugin's VM is reusable afterwards (on-demand emergency GC reclaims the
    // failed callback's now-unreachable table).
    assert!(mgr.call_discover("heavy", "", "", 1, &[]).await.is_ok());

    // The OTHER plugin's VM is provably unaffected.
    assert!(mgr.call_discover("light", "", "", 1, &[]).await.is_ok());
}
