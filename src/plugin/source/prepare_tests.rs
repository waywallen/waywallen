use super::*;
use crate::plugin::renderer_registry::{scan_plugins, PluginScan};
use std::os::unix::fs::{symlink, PermissionsExt};

fn manager(root: &Path) -> SourceManager {
    SourceManager::with_state_store(crate::plugin::state_store::PluginStateStore::new(
        root.join("state"),
        root.join("legacy"),
    ))
    .unwrap()
}

fn script(download: &str, resolve: &str) -> String {
    format!(
        r#"
local M = {{}}
function M.info()
    return {{name='prepare_test',capabilities={{discover={{search=true,download=true,resolve=true}}}}}}
end
M.discover = {{}}
function M.discover.search(ctx, params) return {{items={{}},has_more=false}} end
function M.discover.download(ctx, id)
    return {{wp_type='gltf',title='Model',external_id=id,url='https://example.invalid/model.zip',
        filename='model.zip',{download}}}
end
function M.discover.resolve(ctx, params)
    {resolve}
end
return M
"#
    )
}

const RESOLVE: &str =
    "return {name='Model',wp_type='gltf',resource='scene.gltf',external_id=params.id}";

fn plugin(download: &str, resolve: &str, version: u32) -> (tempfile::TempDir, SourceManager) {
    let root = tempfile::tempdir().unwrap();
    let entry = root.path().join("main.lua");
    std::fs::write(&entry, script(download, resolve)).unwrap();
    let manager = manager(root.path());
    manager
        .load_plugin(&entry, "org.test.prepare", "1", version)
        .unwrap();
    (root, manager)
}

#[tokio::test]
async fn legacy_downloads_and_resolve_do_not_require_prepare_support() {
    for version in [
        ENTRY_VERSION_V3,
        ENTRY_VERSION_V4,
        ENTRY_VERSION_V5,
        ENTRY_VERSION_V6,
    ] {
        let resolve = format!("assert(ctx.artifact_resource_path == nil)\n{RESOLVE}");
        let (root, manager) = plugin("", &resolve, version);
        let download = manager
            .call_download("prepare_test", "model")
            .await
            .unwrap();
        assert_eq!(download.transfer.format, DownloadFormat::File);
        assert!(download.transfer.resources.is_empty());
        assert!(download.transfer.prepare.is_none());
        let resolved = manager
            .call_resolve(
                "prepare_test",
                "model",
                root.path().to_str().unwrap(),
                Some(&download.metadata),
            )
            .await
            .unwrap();
        assert_eq!(resolved.resource, "scene.gltf");
        assert!(resolved.provider_data.is_none());
    }
}

#[tokio::test]
async fn version_six_rejects_prepare_download_fields_and_resolve_metadata() {
    for fields in [
        "resources=ctx.json.strict.array{},",
        "prepare=ctx.json.strict.object{},",
    ] {
        let (_root, manager) = plugin(fields, RESOLVE, ENTRY_VERSION_V6);
        let error = manager
            .call_download("prepare_test", "model")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("require Lua entry_version 7"),
            "{error}"
        );
    }
    let resolve = "return {name='Model',wp_type='gltf',resource='scene.gltf',external_id=params.id,provider_data=ctx.json.strict.object{new=true}}";
    let (root, manager) = plugin("", resolve, ENTRY_VERSION_V6);
    let error = manager
        .call_resolve("prepare_test", "model", root.path().to_str().unwrap(), None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("requires Lua entry_version 7"),
        "{error}"
    );
}

#[tokio::test]
async fn version_seven_requires_prepare_parameters_and_a_declared_helper() {
    let fields = "resources={{id='env',url='https://example.invalid/env.gz'}},";
    let (_root, manager) = plugin(fields, RESOLVE, ENTRY_VERSION_V7);
    let error = manager
        .call_download("prepare_test", "model")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("requires prepare parameters"),
        "{error}"
    );

    let (_root, manager) = plugin(
        "prepare=ctx.json.strict.object{},",
        RESOLVE,
        ENTRY_VERSION_V7,
    );
    let error = manager
        .call_download("prepare_test", "model")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("no declared prepare helper"),
        "{error}"
    );

    let (_root, manager) = plugin(
        "resources=ctx.json.strict.array{},",
        RESOLVE,
        ENTRY_VERSION_V7,
    );
    let download = manager
        .call_download("prepare_test", "model")
        .await
        .unwrap();
    assert!(download.transfer.resources.is_empty() && download.transfer.prepare.is_none());
}

#[tokio::test]
async fn version_seven_rejects_invalid_resource_list_shapes() {
    for value in [
        "false",
        "'resource'",
        "{}",
        "ctx.json.strict.null",
        "ctx.json.strict.object{id='env',url='https://example.invalid/env.gz'}",
        "ctx.json.strict.array{1}",
        "{[2]={id='env',url='https://example.invalid/env.gz'}}",
        "{{id='env',url='https://example.invalid/env.gz',unexpected=true}}",
    ] {
        let fields = format!("resources={value},prepare=ctx.json.strict.object{{}},");
        let (_root, manager) = plugin(&fields, RESOLVE, ENTRY_VERSION_V7);
        let error = manager
            .call_download("prepare_test", "model")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("result.resources"),
            "resources={value}: {error}"
        );
    }
}

#[tokio::test]
async fn version_seven_prepare_parameters_reuse_strict_object_and_size_validation() {
    for value in [
        "false",
        "1",
        "'parameters'",
        "{}",
        "ctx.json.strict.null",
        "ctx.json.strict.array{}",
        "ctx.json.strict.object{value=0/0}",
        "ctx.json.strict.object{value=function() end}",
    ] {
        let fields = format!("prepare={value},");
        let (_root, manager) = plugin(&fields, RESOLVE, ENTRY_VERSION_V7);
        let error = manager
            .call_download("prepare_test", "model")
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("result.prepare must be a JSON object"),
            "prepare={value}: {error}"
        );
    }
    let (_root, manager) = plugin(
        "prepare=ctx.json.strict.object{value=string.rep('x',64*1024)},",
        RESOLVE,
        ENTRY_VERSION_V7,
    );
    let error = manager
        .call_download("prepare_test", "model")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("result.prepare exceeds 64 KiB"),
        "{error}"
    );
}

#[tokio::test]
async fn version_seven_resolve_metadata_reuses_download_json_validation() {
    for value in [
        "false",
        "{}",
        "ctx.json.strict.null",
        "ctx.json.strict.array{}",
        "ctx.json.strict.object{value=0/0}",
        "ctx.json.strict.object{value=function() end}",
    ] {
        let resolve = format!("return {{name='Model',wp_type='gltf',resource='scene.gltf',external_id=params.id,provider_data={value}}}");
        let (root, manager) = plugin("", &resolve, ENTRY_VERSION_V7);
        let error = manager
            .call_resolve("prepare_test", "model", root.path().to_str().unwrap(), None)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("result.provider_data must be a JSON object"),
            "provider_data={value}: {error}"
        );
    }
    let resolve = "return {name='Model',wp_type='gltf',resource='scene.gltf',external_id=params.id,provider_data=ctx.json.strict.object{value=string.rep('x',64*1024)}}";
    let (root, manager) = plugin("", resolve, ENTRY_VERSION_V7);
    let error = manager
        .call_resolve("prepare_test", "model", root.path().to_str().unwrap(), None)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("result.provider_data exceeds 64 KiB"),
        "{error}"
    );

    let resolve = "assert(params.metadata.provider_data.original == true and params.metadata.url == nil)\nreturn {name='Model',wp_type='gltf',resource='scene.gltf',external_id=params.id,provider_data=ctx.json.strict.object{replacement=ctx.json.strict.object{path=ctx.artifact_resource_path('environment/ibl.hdr')}}}";
    let (root, manager) = plugin(
        "provider_data=ctx.json.strict.object{original=true},",
        resolve,
        ENTRY_VERSION_V7,
    );
    let download = manager
        .call_download("prepare_test", "model")
        .await
        .unwrap();
    let resolved = manager
        .call_resolve(
            "prepare_test",
            "model",
            root.path().to_str().unwrap(),
            Some(&download.metadata),
        )
        .await
        .unwrap();
    let provider_data = resolved.provider_data.unwrap();
    assert!(!provider_data.contains_key("original"));
    assert_eq!(
        provider_data["replacement"]["path"],
        crate::remote_asset::resource_path(Path::new("environment/ibl.hdr"))
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert_eq!(download.metadata.provider_data.unwrap()["original"], true);
}

fn package_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let package = plugins.join("org.test.prepare");
    std::fs::create_dir_all(package.join("helpers")).unwrap();
    std::fs::write(
        package.join("main.lua"),
        script(
            "resources={{id='env',url='https://example.invalid/env.gz',size=3}},prepare=ctx.json.strict.object{version=1},",
            RESOLVE,
        ),
    )
    .unwrap();
    std::fs::write(package.join("helpers/prepare"), "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(
        package.join("helpers/prepare"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    manifest(&package, Some(ENTRY_VERSION_V7), "helpers/prepare", 1);
    std::fs::write(
        package.join("files.txt"),
        "plugin.toml\nmain.lua\nhelpers/prepare\n",
    )
    .unwrap();
    (root, plugins, package)
}

fn manifest(package: &Path, abi: Option<u32>, bin: &str, version: u32) {
    let abi = abi
        .map(|value| format!("entry_version={value}\n"))
        .unwrap_or_default();
    std::fs::write(
        package.join("plugin.toml"),
        format!(
            "[plugin]\nid='org.test.prepare'\nname='Prepare test'\nversion='1'\nentry='main.lua'\n{abi}\n[plugin.prepare]\nbin='{bin}'\nversion={version}\n"
        ),
    )
    .unwrap();
}

fn rejected(scan: PluginScan, expected: &str) {
    assert!(scan.entries.is_empty());
    assert_eq!(scan.incompatible.len(), 1);
    assert!(
        scan.incompatible[0].reason.contains(expected),
        "{:?}",
        scan.incompatible
    );
}

#[test]
fn scanner_rejects_missing_or_unlisted_prepare_helpers() {
    let (_root, plugins, package) = package_fixture();
    std::fs::remove_file(package.join("helpers/prepare")).unwrap();
    rejected(scan_plugins(&plugins, false), "os error 2");

    let (_root, plugins, package) = package_fixture();
    std::fs::write(package.join("files.txt"), "plugin.toml\nmain.lua\n").unwrap();
    rejected(scan_plugins(&plugins, false), "must be listed in files.txt");
}

#[test]
fn scanner_rejects_prepare_paths_versions_and_old_entry_abis() {
    for (bin, version, abi, expected) in [
        (
            "../prepare",
            1,
            Some(ENTRY_VERSION_V7),
            "normal relative path",
        ),
        (
            "/tmp/prepare",
            1,
            Some(ENTRY_VERSION_V7),
            "normal relative path",
        ),
        (
            "helpers/prepare",
            2,
            Some(ENTRY_VERSION_V7),
            "version must be 1",
        ),
        (
            "helpers/prepare",
            1,
            Some(ENTRY_VERSION_V6),
            "requires Lua entry_version 7",
        ),
        ("helpers/prepare", 1, None, "requires Lua entry_version 7"),
        (
            "helpers/prepare",
            1,
            Some(ENTRY_VERSION_V7 + 1),
            "is unsupported",
        ),
    ] {
        let (_root, plugins, package) = package_fixture();
        manifest(&package, abi, bin, version);
        rejected(scan_plugins(&plugins, false), expected);
    }
    let (_root, plugins, package) = package_fixture();
    std::fs::remove_file(package.join("helpers/prepare")).unwrap();
    symlink("../main.lua", package.join("helpers/prepare")).unwrap();
    rejected(scan_plugins(&plugins, false), "must not contain symlinks");
}

#[tokio::test]
async fn scanned_prepare_definition_reaches_the_loaded_source_entry() {
    let (root, plugins, package) = package_fixture();
    let scan = scan_plugins(&plugins, false);
    assert!(scan.incompatible.is_empty());
    assert_eq!(scan.entries.len(), 1);
    let entry = &scan.entries[0];
    let definition = entry.prepare.as_ref().unwrap();
    assert_eq!(definition.version, 1);
    assert_eq!(
        definition.bin,
        package.join("helpers/prepare").canonicalize().unwrap()
    );
    let manager = manager(root.path());
    assert_eq!(manager.load_entry(entry).unwrap(), "prepare_test");
    let session = manager.download_session("prepare_test").unwrap();
    let download = session.download("model").await.unwrap();
    assert_eq!(session.source_id(), "prepare_test");
    assert_eq!(session.plugin_version(), "1");
    assert_eq!(download.transfer.resources.len(), 1);
    assert_eq!(download.transfer.resources[0].id, "env");
    assert_eq!(download.transfer.resources[0].size, Some(3));
    assert_eq!(download.transfer.prepare.unwrap()["version"], 1);
}
