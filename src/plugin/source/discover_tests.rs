use super::*;

fn plugin(script: &str, version: u32) -> (tempfile::TempDir, SourceManager) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.lua");
    std::fs::write(&path, script).unwrap();
    let manager = SourceManager::new().unwrap();
    manager
        .load_plugin(&path, "org.test.discover", "1", version)
        .unwrap();
    (directory, manager)
}

fn search_script(pagination: &str, body: &str) -> String {
    format!(
        r#"
local M = {{}}
function M.info()
    return {{ name = "test", capabilities = {{ discover = {{ search = true, {pagination} }} }} }}
end
M.discover = {{}}
function M.discover.search(ctx, params)
    {body}
end
return M
"#
    )
}

fn detail_script(fields: &str) -> String {
    format!(
        r#"
local M = {{}}
function M.info()
    return {{ name = "test", capabilities = {{ discover = {{ search = true, details = true }} }} }}
end
M.discover = {{}}
function M.discover.search(ctx, params) return {{ items = {{}}, has_more = false }} end
function M.discover.details(ctx, id)
    return {{ description = "model", size = "", tags = {{}}, {fields} }}
end
return M
"#
    )
}

#[tokio::test]
async fn discover_json_strict_preserves_shapes_without_changing_legacy_parse() {
    let script = search_script(
        "",
        r#"
assert(ctx.json.parse('{"missing":null}').missing == nil)
local codec = ctx.json.strict
local value = codec.decode('{"array":[],"object":{},"null":null}')
assert(codec.kind(value.array) == 'array')
assert(codec.kind(value.object) == 'object')
assert(value.null == codec.null)
return {items={},has_more=false}
"#,
    );
    let (_directory, manager) = plugin(&script, ENTRY_VERSION_V5);
    manager.call_discover("test", "", "", 1, &[]).await.unwrap();
}

#[tokio::test]
async fn discover_cursor_round_trip_is_explicit_and_independent() {
    let script = search_script(
        "pagination = 'cursor'",
        r#"
if not params.cursor then
    return {items = {}, has_more = true, next_cursor = params.query .. '+/opaque='}
end
assert(params.cursor == params.query .. '+/opaque=')
return {items = {}, has_more = false}
"#,
    );
    let (_directory, manager) = plugin(&script, ENTRY_VERSION_V5);
    assert_eq!(
        manager.discover_sources().unwrap()[0].pagination,
        DiscoverPagination::Cursor
    );
    for query in ["one", "two"] {
        let first = manager
            .call_discover("test", query, "", 1, &[])
            .await
            .unwrap();
        assert_eq!(first.next_cursor, format!("{query}+/opaque="));
        let second = manager
            .call_discover_with_cursor("test", query, "", 2, &[], &first.next_cursor)
            .await
            .unwrap();
        assert!(!second.has_more);
        assert!(second.next_cursor.is_empty());
    }
}

#[tokio::test]
async fn discover_cursor_validates_request_before_callback() {
    let script = search_script("pagination = 'cursor'", "error('callback must not run')");
    let (_directory, manager) = plugin(&script, ENTRY_VERSION_V5);
    for (page, cursor) in [
        (2, String::new()),
        (1, "x".repeat(MAX_DISCOVER_CURSOR_BYTES + 1)),
        (1, "bad\ncursor".into()),
    ] {
        let error = manager
            .call_discover_with_cursor("test", "", "", page, &[], &cursor)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::InvalidArgument(_)), "{error}");
    }
}

#[tokio::test]
async fn discover_cursor_rejects_inconsistent_and_repeated_results() {
    for body in [
        "return {items={},has_more=true}",
        "return {items={},has_more=false,next_cursor='next'}",
        "return {items={},has_more=true,next_cursor=params.cursor}",
        "return {items={},has_more=true,next_cursor=string.rep('x',4097)}",
        "return {items={},has_more=true,next_cursor={}}",
        r"return {items={},has_more=true,next_cursor='bad\ncursor'}",
    ] {
        let script = search_script("pagination = 'cursor'", body);
        let (_directory, manager) = plugin(&script, ENTRY_VERSION_V5);
        assert!(
            manager
                .call_discover_with_cursor("test", "", "", 2, &[], "current")
                .await
                .is_err(),
            "accepted {body}"
        );
    }
}

#[tokio::test]
async fn discover_cursor_is_versioned_and_page_sources_remain_compatible() {
    for version in [ENTRY_VERSION_V3, ENTRY_VERSION_V4, ENTRY_VERSION_V5] {
        let script = search_script("", "return {items={},has_more=params.page==1}");
        let (_directory, manager) = plugin(&script, version);
        assert_eq!(
            manager.discover_sources().unwrap()[0].pagination,
            DiscoverPagination::Page
        );
        assert!(
            manager
                .call_discover("test", "", "", 1, &[])
                .await
                .unwrap()
                .has_more
        );
        assert!(
            !manager
                .call_discover("test", "", "", 2, &[])
                .await
                .unwrap()
                .has_more
        );
        assert!(manager
            .call_discover_with_cursor("test", "", "", 2, &[], "token")
            .await
            .is_err());
    }
    for version in [ENTRY_VERSION_V3, ENTRY_VERSION_V4] {
        let script = search_script("pagination = 'cursor'", "return {items={},has_more=false}");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.lua");
        std::fs::write(&path, script).unwrap();
        assert!(SourceManager::new()
            .unwrap()
            .load_plugin(&path, "org.test", "1", version)
            .is_err());

        let script = search_script("", "return {items={},has_more=false,next_cursor=''}");
        let (_directory, manager) = plugin(&script, version);
        assert!(manager.call_discover("test", "", "", 1, &[]).await.is_err());
    }
}

#[test]
fn discover_cursor_rejects_unknown_pagination_modes() {
    let script = search_script("pagination = 'guess'", "return {items={},has_more=false}");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.lua");
    std::fs::write(&path, script).unwrap();
    assert!(SourceManager::new()
        .unwrap()
        .load_plugin(&path, "org.test", "1", ENTRY_VERSION_V5)
        .is_err());
}

#[tokio::test]
async fn discover_detail_fields_preserve_order_translation_and_text() {
    let script = detail_script(
        r#"fields = {
        {key='license',label=tr('License'),value='CC BY',url='https://creativecommons.org/licenses/by/4.0/'},
        {key='vertices',label='Vertices',value='0'},
        {key='author',label='Author',value='作者 <b>text</b>'},
    }"#,
    );
    let (_directory, manager) = plugin(&script, ENTRY_VERSION_V5);
    let details = manager.call_details("test", "uid").await.unwrap();
    assert_eq!(details.fields.len(), 3);
    assert_eq!(details.fields[0].key, "license");
    let message = details.fields[0].label.message().unwrap();
    assert_eq!(message.plugin_id, "org.test.discover");
    assert_eq!(message.msgid, "License");
    assert_eq!(details.fields[1].value, "0");
    assert!(details.fields[1].url.is_empty());
    assert_eq!(details.fields[2].value, "作者 <b>text</b>");
}

#[tokio::test]
async fn discover_detail_fields_reject_invalid_structure_and_bounds() {
    for fields in [
        "fields='bad'",
        "fields={false}",
        "fields={name={key='a',label='A',value='1'}}",
        "fields={[2]={key='a',label='A',value='1'}}",
        "fields={{key='',label='A',value='1'}}",
        "fields={{key='bad key',label='A',value='1'}}",
        "fields={{key='a',label='',value='1'}}",
        "fields={{key='a',label='A',value={}}}",
        "fields={{key='a',label='A',value='1'},{key='a',label='B',value='2'}}",
        "fields={{key='a',label=string.rep('x',513),value='1'}}",
        "fields={{key='a',label='A',value=string.rep('x',8193)}}",
    ] {
        let (_directory, manager) = plugin(&detail_script(fields), ENTRY_VERSION_V5);
        assert!(
            manager.call_details("test", "uid").await.is_err(),
            "accepted {fields}"
        );
    }
    let fields = (1..=33)
        .map(|index| format!("{{key='k{index}',label='K',value='1'}}"))
        .collect::<Vec<_>>()
        .join(",");
    let (_directory, manager) = plugin(
        &detail_script(&format!("fields={{{fields}}}")),
        ENTRY_VERSION_V5,
    );
    assert!(manager.call_details("test", "uid").await.is_err());
}

#[tokio::test]
async fn discover_detail_fields_reject_unsafe_urls() {
    for url in [
        "javascript:alert(1)",
        "file:///tmp/model",
        "data:text/html,test",
        "https://user:password@example.com/model",
        "https://example.com/has space",
        "//example.com/model",
    ] {
        let fields = format!("fields={{{{key='link',label='Link',value='Model',url='{url}'}}}}");
        let (_directory, manager) = plugin(&detail_script(&fields), ENTRY_VERSION_V5);
        assert!(
            manager.call_details("test", "uid").await.is_err(),
            "accepted {url}"
        );
    }
}

#[tokio::test]
async fn discover_detail_fields_require_v5_but_legacy_details_still_work() {
    for version in [ENTRY_VERSION_V3, ENTRY_VERSION_V4, ENTRY_VERSION_V5] {
        let (_directory, manager) = plugin(&detail_script(""), version);
        assert!(manager
            .call_details("test", "uid")
            .await
            .unwrap()
            .fields
            .is_empty());
    }
    for version in [ENTRY_VERSION_V3, ENTRY_VERSION_V4] {
        let (_directory, manager) = plugin(&detail_script("fields={}"), version);
        assert!(manager.call_details("test", "uid").await.is_err());
    }
}

#[tokio::test]
async fn discover_author_url_is_optional_and_preserves_public_profile_link() {
    for version in [ENTRY_VERSION_V3, ENTRY_VERSION_V4, ENTRY_VERSION_V5] {
        let (_directory, manager) = plugin(&detail_script(""), version);
        assert!(manager
            .call_details("test", "uid")
            .await
            .unwrap()
            .author_url
            .is_empty());
    }
    for url in [
        "",
        "https://sketchfab.com/artist",
        "http://example.com/artist",
    ] {
        let script = detail_script(&format!("author='Artist',author_url='{url}'"));
        let (_directory, manager) = plugin(&script, ENTRY_VERSION_V5);
        let details = manager.call_details("test", "uid").await.unwrap();
        assert_eq!(details.author, "Artist");
        assert_eq!(details.author_url, url);
    }
}

#[tokio::test]
async fn discover_author_url_rejects_unsafe_or_unbounded_links() {
    for url in [
        "javascript:alert(1)",
        "file:///tmp/author",
        "data:text/html,author",
        "https://user:secret@example.com/author",
        "https://example.com/has space",
        "//example.com/author",
    ] {
        let script = detail_script(&format!("author_url='{url}'"));
        let (_directory, manager) = plugin(&script, ENTRY_VERSION_V5);
        assert!(
            manager.call_details("test", "uid").await.is_err(),
            "accepted {url}"
        );
    }
    for value in ["{}", "'https://example.com/' .. string.rep('a',4096)"] {
        let script = detail_script(&format!("author_url={value}"));
        let (_directory, manager) = plugin(&script, ENTRY_VERSION_V5);
        assert!(manager.call_details("test", "uid").await.is_err());
    }
}
