use super::*;

fn plugin(code: &str, version: u32) -> (tempfile::TempDir, SourceManager) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("main.lua");
    std::fs::write(&path, code).unwrap();
    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&path, "test.libraries", "1", version)
        .unwrap();
    (dir, manager)
}

const LEGACY_PLUGIN: &str = r#"
local M = {}
function M.info()
    return {
        name = "legacy",
        capabilities = {source = {types = {"image"}, scan = true, auto_detect = true}},
    }
end
M.source = {}
function M.source.auto_detect(ctx)
    return {"/legacy/Steam", "/legacy/Other"}
end
function M.source.scan(ctx)
    local thread = coroutine.create(function() return ctx.libraries() end)
    local ok, roots = coroutine.resume(thread)
    assert(ok and coroutine.status(thread) == "dead", "libraries must not yield")
    assert(type(roots[1]) == "string" and type(roots[2]) == "string")
    local entries = {}
    for _, steam_root in ipairs(roots) do
        local state = steam_root .. "/steamapps/workshop/appworkshop_431960.acf"
        assert(state:find("appworkshop_431960.acf", 1, true))
        entries[#entries + 1] = {
            name = steam_root,
            wp_type = "image",
            resource = steam_root .. "/item.png",
            library_root = steam_root,
        }
    end
    return entries
end
return M
"#;

#[tokio::test]
async fn library_context_preserves_legacy_plugins_for_every_supported_version() {
    for &version in SUPPORTED_ENTRY_VERSIONS {
        let (_dir, manager) = plugin(LEGACY_PLUGIN, version);
        let detected = manager.auto_detect_all().await.unwrap();
        let libraries = &detected["legacy"];
        assert_eq!(libraries.len(), 2);
        assert_eq!(libraries[0].path, "/legacy/Steam");
        assert!(libraries.iter().all(|library| library.settings.is_empty()));
        manager
            .scan_all(&HashMap::from([(
                "legacy".into(),
                libraries
                    .iter()
                    .map(|library| library.path.clone())
                    .collect(),
            )]))
            .await
            .unwrap();
        let entries = manager.list();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|entry| entry.resource == "/legacy/Steam/item.png"));
    }
}

const CONFIGURED_PLUGIN: &str = r#"
local M = {}
function M.info()
    return {
        name = "configured",
        capabilities = {source = {
            types = {"image"}, scan = true, auto_detect = true,
            settings = {{key = "scan_mode", default = "files", choices = {"files", "wallpapers"}}},
        }},
    }
end
M.source = {}
function M.source.auto_detect(ctx)
    return {
        "/legacy/path",
        {path = "/configured/path", settings = {scan_mode = "wallpapers"}},
        {path = "/default/path"},
    }
end
function M.source.scan(ctx)
    for _, path in ipairs(ctx.libraries()) do
        assert(type(path) == "string")
        local settings = ctx.library_settings(path)
        assert(settings.scan_mode == "files")
    end
    return {}
end
return M
"#;

#[tokio::test]
async fn library_context_accepts_mixed_auto_detect_results_and_supplies_defaults() {
    let (_dir, manager) = plugin(CONFIGURED_PLUGIN, ENTRY_VERSION);
    let detected = manager.auto_detect_all().await.unwrap();
    let libraries = &detected["configured"];
    assert_eq!(libraries.len(), 3);
    assert_eq!(libraries[0].settings["scan_mode"], "files");
    assert_eq!(libraries[1].settings["scan_mode"], "wallpapers");
    assert_eq!(libraries[2].settings["scan_mode"], "files");
    manager
        .scan_all(&HashMap::from([(
            "configured".into(),
            vec!["/configured/path".into()],
        )]))
        .await
        .unwrap();
}

#[tokio::test]
async fn library_context_rejects_invalid_auto_detect_settings() {
    let code = CONFIGURED_PLUGIN.replace(
        "settings = {scan_mode = \"wallpapers\"}",
        "settings = {scan_mode = \"invalid\"}",
    );
    let (_dir, manager) = plugin(&code, ENTRY_VERSION);
    assert!(manager.auto_detect_all().await.is_err());
}
