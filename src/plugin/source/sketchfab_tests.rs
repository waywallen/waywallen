use super::*;

fn entry() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/org.waywallen.sketchfab/main.lua")
}

fn manager_at(root: &Path) -> SourceManager {
    manager_with_entry(root, &entry())
}

fn manager_with_entry(root: &Path, entry: &Path) -> SourceManager {
    let manager = SourceManager::with_probe_and_state_store(
        Arc::new(AvFormatProbe::new()),
        crate::plugin::state_store::PluginStateStore::new(root.join("state"), root.join("legacy")),
    )
    .unwrap();
    manager
        .load_plugin(entry, "org.waywallen.sketchfab", "0.1.0", ENTRY_VERSION_V5)
        .unwrap();
    manager
}

fn manager() -> (tempfile::TempDir, SourceManager) {
    let directory = tempfile::tempdir().unwrap();
    let manager = manager_at(directory.path());
    (directory, manager)
}

fn lua_test(body: &str) {
    let (_directory, manager) = manager();
    let runtime = manager.test_runtime("sketchfab");
    let runtime = runtime.blocking_lock();
    let environment = runtime
        .plugin_lua_env(entry().parent().unwrap(), "org.waywallen.sketchfab")
        .unwrap();
    let import: LuaFunction = environment.get("import").unwrap();
    for name in ["api", "discover", "map", "session"] {
        runtime
            .lua
            .globals()
            .set(
                name,
                import
                    .call::<LuaTable>(format!("sketchfab.{name}"))
                    .unwrap(),
            )
            .unwrap();
    }
    let json = mlua_extra::json::create_nullable_module(&runtime.lua).unwrap();
    json.set(
        "strict",
        mlua_extra::json::strict::create_module(&runtime.lua).unwrap(),
    )
    .unwrap();
    runtime.lua.globals().set("json", json).unwrap();
    runtime
        .lua
        .globals()
        .set("url", mlua_extra::url::create_module(&runtime.lua).unwrap())
        .unwrap();
    runtime
        .lua
        .load(
            r#"
local queue, requests = {}, {}
local clock = 1000
local J = json.strict
ctx = { json = json, url = url, time = { unix = function() return clock end }, http = {} }
function advance(seconds) clock = clock + seconds end
local function json_value(value)
    if type(value) == 'table' then
        for _, member in pairs(value) do json_value(member) end
        if not J.kind(value) then
            if #value > 0 then J.array(value) else J.object(value) end
        end
    end
    return value
end
function reply(payload, status, retry, elapsed, transport_error)
    if type(payload) == 'table' and type(payload.results) == 'table' and not J.kind(payload.results) then
        J.array(payload.results)
    end
    table.insert(queue, { body = type(payload) == 'string' and payload or J.encode(json_value(payload)),
        status_code = status or 200, retry = retry, elapsed = elapsed, transport_error = transport_error })
end
function ctx.http:get(path)
    local captured = { path = path }
    table.insert(requests, captured)
    local request = {}
    function request:headers(value) captured.headers = value; return self end
    function request:query(value) captured.query = value; return self end
    function request:timeout(value) assert(value == 20); return self end
    function request:response_limit(value) assert(value == 4194304); captured.limit = value; return self end
    function request:send()
        local response = assert(table.remove(queue, 1), 'unexpected HTTP request')
        clock = clock + (response.elapsed or 0)
        if response.transport_error then error(response.transport_error) end
        function response:status() return self.status_code end
        function response:ok() return self.status_code >= 200 and self.status_code < 300 end
        function response:header_values(name) assert(name == 'retry-after'); return { self.retry } end
        function response:close() self.closed = true end
        function response:text()
            assert(not self.closed)
            if #self.body > captured.limit then error('response body limit') end
            return self.body
        end
        captured.response = response
        return response
    end
    return request
end
function last_request() return requests[#requests] end
function request_count() return #requests end
function uid(n) return string.format('%032x', n or 1) end
function model(n, fields)
    local value = fields or {}
    value.uid = uid(n)
    return value
end
function normalize(value) return map.normalize(json_value(value), J) end
function next_cursor(value, current) return api.next_cursor(ctx, json_value(value), current) end
function search(models, next_value)
    reply({ results = models, cursors = { next = next_value } })
    return discover.search(ctx, {})
end
function fields(detail)
    local values = {}
    for _, field in ipairs(detail.fields) do values[field.key] = field end
    return values
end
"#,
        )
        .exec()
        .unwrap();
    runtime.lua.load(body).exec().unwrap();
}

#[test]
fn sketchfab_plugin_is_browse_only_and_installed_with_translations() {
    let (_directory, manager) = manager();
    assert!(manager.plugins().unwrap().is_empty());
    let sources = manager.discover_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].plugin_id, "sketchfab");
    assert!(sources[0].supports_search);
    assert!(sources[0].remote_capability.is_none());
    assert_eq!(sources[0].actions.len(), 2);
    let login = sources[0]
        .actions
        .iter()
        .find(|action| action.id == "sketchfab_sign_in")
        .unwrap();
    assert_eq!(login.kind, SourceActionKind::Form);
    assert!(!login.required_for_browsing);
    assert_eq!(login.fields.len(), 1);
    assert_eq!(login.fields[0].key, "api_token");
    assert!(login.fields[0].secret && login.fields[0].required);
    assert_eq!(sources[0].status.len(), 1);
    assert_eq!(sources[0].status[0].id, "sketchfab_account");
    assert_eq!(sources[0].filters.len(), 5);
    assert_eq!(sources[0].filters[0].ty, DiscoverFilterType::Select);
    assert_eq!(sources[0].filters[1].ty, DiscoverFilterType::Select);
    let mut values = HashSet::new();
    for filter in &sources[0].filters {
        for option in &filter.options {
            assert!(values.insert(&option.value));
        }
    }
    let root = entry().parent().unwrap().to_owned();
    let installation = std::fs::read_to_string(root.join("install.lua")).unwrap();
    for path in [
        "plugin.toml",
        "main.lua",
        "sketchfab/api.lua",
        "sketchfab/map.lua",
        "sketchfab/session.lua",
        "sketchfab/discover.lua",
        "i18n/zh-CN.po",
    ] {
        assert!(root.join(path).is_file());
        assert!(installation.contains(&format!("source = \"{path}\"")));
    }
    assert!(installation.contains("inventories"));
    assert!(installation.contains("files.txt"));
}

#[test]
fn sketchfab_search_maps_sort_filters_and_opaque_cursor_without_authentication() {
    lua_test(
        r#"
reply({ results = {}, cursors = { next = 'opaque+/=' } })
local first = discover.search(ctx, { query = '风景 & trees', sort = 'relevance', page = 1 })
assert(first.has_more and first.next_cursor == 'opaque+/=')
local q = last_request().query
assert(q.q == '风景 & trees' and q.type == 'models' and q.count == '24')
assert(q.sort_by == nil and q.downloadable == nil and q.page == nil)
assert(last_request().path == 'https://api.sketchfab.com/v3/search')
assert(last_request().headers.Authorization == nil)
reply({ results = {} })
discover.search(ctx, { page = 2, cursor = first.next_cursor, sort = 'recent', tags = {
    'category:nature-plants', 'license:by', 'feature:downloadable', 'feature:animated', 'feature:staffpicked'
} })
q = last_request().query
assert(q.cursor == 'opaque+/=' and q.sort_by == '-publishedAt')
assert(q.categories == 'nature-plants' and q.license == 'by')
assert(q.downloadable == 'true' and q.animated == 'true' and q.staffpicked == 'true')
for _, sort in ipairs({ {'likes', '-likeCount'}, {'views', '-viewCount'} }) do
    reply({ results = {} }); api.search(ctx, { sort = sort[1] })
    assert(last_request().query.sort_by == sort[2])
end
for _, params in ipairs({
    { page = 2 }, { cursor = '' }, { sort = 'unknown' },
    { tags = {'unknown'} }, { tags = {'license:by', 'license:cc0'} },
    { tags = {'category:music', 'category:people'} }
}) do
    local before = request_count()
    assert(not pcall(api.search, ctx, params))
    assert(request_count() == before)
end
"#,
    );
}

#[test]
fn sketchfab_pagination_validates_service_urls_and_stops_at_null() {
    lua_test(
        r#"
local payload = json.strict.decode('{"results":[],"next":null,"cursors":{"next":null}}')
assert(next_cursor(payload) == nil)
assert(next_cursor({ next = 'https://api.sketchfab.com/v3/search?type=models&cursor=a%2Bb%3D' }) == 'a+b=')
assert(next_cursor({ cursors = { next = 'opaque' } }) == 'opaque')
for _, next_url in ipairs({
    'http://api.sketchfab.com/v3/search?type=models&cursor=x',
    'https://evil.example/v3/search?type=models&cursor=x',
    'https://api.sketchfab.com:444/v3/search?type=models&cursor=x',
    'https://api.sketchfab.com/v3/models?type=models&cursor=x',
    'https://api.sketchfab.com/v3/search?type=users&cursor=x',
    'https://api.sketchfab.com/v3/search?type=models',
    'https://api.sketchfab.com/v3/search?type=models&cursor=x#bad'
}) do assert(not pcall(next_cursor, { next = next_url })) end
assert(not pcall(next_cursor, { cursors = {next='same'} }, 'same'))
assert(not pcall(next_cursor, {
    next = 'https://api.sketchfab.com/v3/search?type=models&cursor=x', cursors = {next='other'}
}))
assert(not pcall(next_cursor, { next = false }))
assert(not pcall(next_cursor, { cursors = 'broken' }))
"#,
    );
}

#[test]
fn sketchfab_mapping_preserves_real_types_missing_fields_zero_and_false() {
    lua_test(
        r#"
local value = normalize(model(1, {
    name = '花🌷', animationCount = 0, vertexCount = 0, isDownloadable = false,
    user = { displayName = '', username = 'artist', profileUrl = 'https://sketchfab.com/artist' },
    thumbnails = { images = {
        {url='https://example.com/large.jpg', width=1920, height=1080},
        {url='https://example.com/small.jpg', width=256, height=144},
        {url='https://example.com/medium.jpg', width=720, height=405}
    } },
    license = {label='CC Attribution', slug='by', url='https://creativecommons.org/licenses/by/4.0/'},
    tags = { {name='Landscape'}, {slug='nature'} },
    archives = {glb={size=1234}}
}))
local item, detail = map.search_item(value), map.details(value)
assert(item.title == '花🌷' and item.wp_type == 'model')
assert(item.preview_url == 'https://example.com/medium.jpg')
assert(item.width == nil and item.height == nil and item.author == 'artist')
assert(detail.width == nil and detail.height == nil and detail.size == '')
assert(detail.author_url == 'https://sketchfab.com/artist')
assert(detail.tags[1] == 'Landscape' and detail.tags[2] == 'nature')
local info = fields(detail)
assert(info.author == nil)
assert(info.animations.value == '0' and info.vertices.value == '0')
assert(info.downloadable.value == 'false' and info.archive_glb.value == '1234 B')
assert(info.license.url == 'https://creativecommons.org/licenses/by/4.0/')
local missing = map.search_item(normalize(model(2)))
assert(missing.preview_url == '' and missing.author == '' and missing.author_avatar_url == '')
assert(#map.details(normalize(model(2))).fields == 0)
assert(map.details(normalize(model(2))).author_url == '')
assert(map.search_item(normalize(model(3, {thumbnails={images={
    {url='https://example.com/1.jpg',width=64,height=32},
    {url='https://example.com/2.jpg',width=256,height=128}
}}}))).preview_url == 'https://example.com/2.jpg')
assert(not pcall(normalize, {uid='../bad'}))
assert(not pcall(normalize, model(1, {name=string.rep('x', 1025)})))
assert(not pcall(map.list, json.strict.object{named={}}, 24, json.strict))
reply({ unexpected = {} }); assert(not pcall(discover.search, ctx, {}))
"#,
    );
}

#[test]
fn sketchfab_strict_json_rejects_malformed_shapes_and_preserves_null() {
    lua_test(
        r#"
reply('{"results":[],"next":null,"cursors":null}')
local empty = discover.search(ctx, {})
assert(#empty.items == 0 and not empty.has_more and empty.next_cursor == nil)
for _, payload in ipairs({
    '{"results":{}}', '{"results":null}', '{"results":[null]}',
    '{"results":[{}]}', '{"results":[[]]}', '{"results":"invalid"}', '[]'
}) do
    reply(payload)
    assert(not pcall(discover.search, ctx, {}), payload)
    assert(last_request().response.closed)
end
local id = '"uid":"' .. uid(1) .. '"'
for _, member in ipairs({
    '"tags":{}', '"categories":{}', '"thumbnails":{"images":{}}',
    '"user":{"avatar":{"images":{}}}'
}) do
    local value = json.strict.decode('{' .. id .. ',' .. member .. '}')
    assert(not pcall(map.normalize, value, json.strict), member)
end
local value = json.strict.decode('{' .. id .. ',"tags":[],"categories":[],"thumbnails":{"images":[]}}')
local normalized = map.normalize(value, json.strict)
assert(#normalized.tags == 0 and #normalized.categories == 0 and normalized.preview_url == '')
value = json.strict.decode('{' .. id .. ',"tags":null,"categories":null,"thumbnails":null,"user":null,"license":null}')
normalized = map.normalize(value, json.strict)
assert(normalized.tags == nil and normalized.categories == nil and normalized.author == nil)
assert(normalized.license == nil and normalized.preview_url == nil)
assert(not pcall(map.normalize, json.strict.array{}, json.strict))
assert(not pcall(map.normalize, json.strict.null, json.strict))
"#,
    );
}

#[test]
fn sketchfab_cache_merges_search_and_detail_metadata_and_is_bounded() {
    lua_test(
        r#"
search({model(1, {archives={glb={size=123}}, animationCount=5, isDownloadable=true,
    user={username='artist',profileUrl='https://sketchfab.com/artist'}})})
reply(model(1, {description='Detail', animationCount=0, isDownloadable=false,
    license={label='CC Attribution',fullName='Creative Commons Attribution',url='https://example.com/license',requirements='Credit the artist'}}))
local detail = discover.details(ctx, uid(1))
assert(detail.description == 'Detail')
assert(detail.author == 'artist' and detail.author_url == 'https://sketchfab.com/artist')
assert(fields(detail).author == nil)
assert(fields(detail).archive_glb.value == '123 B')
assert(fields(detail).downloadable.value == 'false' and fields(detail).animations.value == '0')
search({model(1, {license={label='CC Attribution'},archives={glb={}}})})
reply(model(1))
detail = discover.details(ctx, uid(1))
assert(fields(detail).license.url == 'https://example.com/license')
assert(fields(detail).license.value == 'Creative Commons Attribution')
assert(fields(detail).license_requirements.value == 'Credit the artist')
assert(fields(detail).archive_glb.value == '123 B')
advance(301); reply(model(1)); detail = discover.details(ctx, uid(1))
assert(fields(detail).archive_glb == nil and fields(detail).license == nil)
search({model(1, {archives={glb={size=456}}})})
for i=2,130 do search({model(i)}) end
reply(model(1)); assert(fields(discover.details(ctx, uid(1))).archive_glb == nil)
reply(model(200, {description='Direct'})); assert(discover.details(ctx, uid(200)).description == 'Direct')
reply(model(201)); assert(not pcall(discover.details, ctx, uid(200)))
"#,
    );
}

#[test]
fn sketchfab_http_errors_close_responses_and_rate_limits_do_not_sleep() {
    lua_test(
        r#"
for _, status in ipairs({401,403,404,500}) do
    reply('{}', status)
    assert(not pcall(api.model, ctx, uid(1)))
    assert(last_request().response.closed)
end
reply('{}', 429, '12', 20)
assert(not pcall(api.search, ctx, {}))
assert(last_request().response.closed)
local before = request_count()
local ok, message = pcall(api.search, ctx, {})
assert(not ok and tostring(message):find('retry in 12 seconds',1,true))
assert(request_count() == before)
advance(12); reply('{}', 429, 'Wed, 21 Oct 2015 07:28:00 GMT')
assert(not pcall(api.search, ctx, {}))
advance(29); assert(not pcall(api.search, ctx, {}))
advance(1); reply({results={}}); assert(not discover.search(ctx, {}).has_more)
reply('{}', 429, '3600'); assert(not pcall(api.search, ctx, {}))
before = request_count()
advance(300); assert(not pcall(api.search, ctx, {})); assert(request_count() == before)
advance(3299); assert(not pcall(api.search, ctx, {})); assert(request_count() == before)
advance(1); reply({results={}}); assert(not discover.search(ctx, {}).has_more)
reply('{}', 429, string.rep('9', 400)); assert(not pcall(api.search, ctx, {}))
advance(29); assert(not pcall(api.search, ctx, {}))
advance(1); reply({results={}}); assert(not discover.search(ctx, {}).has_more)
reply('not JSON'); assert(not pcall(api.search, ctx, {})); assert(last_request().response.closed)
reply('null'); assert(not pcall(api.search, ctx, {}))
reply(string.rep(' ',4194305)); assert(not pcall(api.search, ctx, {})); assert(last_request().response.closed)
"#,
    );
}

#[test]
fn sketchfab_session_verifies_candidates_and_authenticates_api_requests() {
    lua_test(
        r#"
assert(not session.authenticated() and session.save() == '')
assert(session.check(ctx).state == 'signed_out' and request_count() == 0)
local revision = session.revision()
reply({uid=uid(1),username='artist'})
session.sign_in(ctx, '  test-token  ')
assert(last_request().path == 'https://api.sketchfab.com/v3/me')
assert(last_request().headers.Authorization == 'Token test-token')
assert(session.authenticated() and session.revision() > revision)
assert(session.current_check().state == 'signed_in')
assert(session.headers().Authorization == 'Token test-token')
assert(session.save() == 'sketchfab-session-v1\n10\ntest-token')
assert(session.check(ctx).state == 'signed_in' and request_count() == 1)
search({model(1)})
assert(last_request().headers.Authorization == 'Token test-token')
reply(model(1)); discover.details(ctx, uid(1))
assert(last_request().headers.Authorization == 'Token test-token')
local saved = session.save()
session.load(saved)
reply({uid=uid(1),username='artist'})
assert(session.check(ctx).state == 'signed_in')
assert(last_request().path == 'https://api.sketchfab.com/v3/me')
revision = session.revision()
session.sign_out()
assert(not session.authenticated() and session.save() == '')
assert(session.revision() > revision and session.headers().Authorization == nil)
local requests = request_count()
assert(session.check(ctx).state == 'signed_out' and request_count() == requests)
search({})
assert(last_request().headers.Authorization == nil)
"#,
    );
}

#[test]
fn sketchfab_session_rejects_bad_input_without_requesting_or_disclosing_tokens() {
    lua_test(
        r#"
for _, value in ipairs({'', '   ', 'one\ntwo', 'one\rtwo', 'one two', 'one\0two', string.rep('a',4097)}) do
    local ok = pcall(session.sign_in, ctx, value)
    assert(not ok and request_count() == 0)
end
for _, blob in ipairs({false, '', 'unknown\n3\nabc', 'sketchfab-session-v1\n4\nabc',
    'sketchfab-session-v1\n3\na\nb', 'sketchfab-session-v1\n4097\n' .. string.rep('x',4097)}) do
    session.load(blob)
    assert(session.save() == '' and not session.authenticated())
end
reply({uid=uid(1),username='artist'})
session.sign_in(ctx, 'original-token')
local saved, revision = session.save(), session.revision()
local function rejected(payload, status, retry, transport_error)
    reply(payload, status, retry, nil, transport_error)
    local ok, message = pcall(session.sign_in, ctx, 'replacement-secret')
    assert(not ok and not tostring(message):find('replacement-secret',1,true))
    assert(session.save() == saved and session.revision() == revision)
    assert(session.headers().Authorization == 'Token original-token')
end
rejected('{}',401)
rejected('{}',403)
rejected('{}',500)
rejected('not JSON',200)
rejected({username='missing-uid'},200)
rejected({uid=uid(1),username=''},200)
rejected('{}',200,nil,'network error replacement-secret')
rejected('{}',429,'2')
advance(2)
assert(session.current_check().state == 'signed_in')
"#,
    );
}

#[test]
fn sketchfab_session_only_expires_on_confirmed_account_unauthorized() {
    lua_test(
        r#"
local saved = 'sketchfab-session-v1\n10\ntest-token'
for _, status in ipairs({403,500,429}) do
    session.load(saved)
    local revision = session.revision()
    reply('{}',status,'2')
    assert(session.check(ctx).state == 'error')
    assert(session.save() == saved and session.revision() == revision)
    advance(2)
end
session.load(saved)
reply('not JSON')
assert(session.check(ctx).state == 'error' and session.save() == saved)
reply('{}',200,nil,nil,'network error test-token')
assert(session.check(ctx).state == 'error' and session.save() == saved)
reply({uid=uid(1),username='artist'})
assert(session.check(ctx).state == 'signed_in')
local requests = request_count()
assert(session.check(ctx).state == 'signed_in' and request_count() == requests)
advance(61)
reply('{}',401)
assert(session.check(ctx).state == 'expired')
assert(not session.authenticated() and session.save() == '')
assert(session.headers().Authorization == nil)
"#,
    );
}

#[test]
fn sketchfab_session_changes_clear_model_cache_but_model_errors_keep_credentials() {
    lua_test(
        r#"
search({model(1,{archives={glb={size=11}}})})
reply({uid=uid(2),username='artist'})
session.sign_in(ctx,'first-token')
reply(model(1))
assert(fields(discover.details(ctx,uid(1))).archive_glb == nil)
search({model(1,{archives={glb={size=22}}})})
local saved, revision = session.save(), session.revision()
for _, status in ipairs({401,403}) do
    reply('{}',status)
    assert(not pcall(discover.search,ctx,{}))
    assert(session.save() == saved and session.revision() == revision)
end
reply({uid=uid(3),username='another-artist'})
session.sign_in(ctx,'second-token')
reply(model(1))
assert(fields(discover.details(ctx,uid(1))).archive_glb == nil)
search({model(1,{archives={glb={size=33}}})})
session.sign_out()
reply(model(1))
assert(fields(discover.details(ctx,uid(1))).archive_glb == nil)
assert(last_request().headers.Authorization == nil)
"#,
    );
}

fn session_action_entry(root: &Path) -> PathBuf {
    let source = entry().parent().unwrap().to_owned();
    std::fs::create_dir_all(root.join("sketchfab")).unwrap();
    for module in ["api", "discover", "map", "session"] {
        let path = format!("sketchfab/{module}.lua");
        std::fs::copy(source.join(&path), root.join(&path)).unwrap();
    }
    std::fs::copy(source.join("main.lua"), root.join("plugin_main.lua")).unwrap();
    let entry = root.join("main.lua");
    std::fs::write(
        &entry,
        r#"
local api = import('sketchfab.api')
api.me = function(ctx, token)
    if auth_test_mode == 'offline' then error('network unavailable') end
    if auth_test_mode == 'revoked' or token ~= 'valid-token' then return nil,401 end
    return {uid='00000000000000000000000000000001',username='artist'}
end
local main = import('plugin_main')
local invoke, check = main.actions.invoke, main.lifecycle.check
main.actions.invoke = function(ctx, ...)
    ctx.time.unix = function() return auth_test_clock or 1000 end
    return invoke(ctx, ...)
end
main.lifecycle.check = function(ctx)
    ctx.time.unix = function() return auth_test_clock or 1000 end
    return check(ctx)
end
return main
"#,
    )
    .unwrap();
    entry
}

async fn session_test_mode(manager: &SourceManager, mode: &str, time: i64) {
    let runtime = manager.test_runtime("sketchfab");
    let runtime = runtime.lock().await;
    runtime.lua.globals().set("auth_test_mode", mode).unwrap();
    runtime.lua.globals().set("auth_test_clock", time).unwrap();
}

#[tokio::test]
async fn sketchfab_session_public_actions_persist_restore_and_remove_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let entry = session_action_entry(root);
    let store =
        crate::plugin::state_store::PluginStateStore::new(root.join("state"), root.join("legacy"));
    let manager = manager_with_entry(root, &entry);
    let sources = manager.discover_sources_with_status().await.unwrap();
    let login = sources[0]
        .actions
        .iter()
        .find(|a| a.id == "sketchfab_sign_in")
        .unwrap();
    let logout = sources[0]
        .actions
        .iter()
        .find(|a| a.id == "sketchfab_sign_out")
        .unwrap();
    assert!(login.visible && login.enabled && !logout.visible);
    manager
        .invoke_action(
            "sketchfab",
            "sketchfab_sign_in",
            &HashMap::from([("api_token".into(), "valid-token".into())]),
        )
        .await
        .unwrap();
    let saved = store.load("org.waywallen.sketchfab").unwrap().unwrap();
    assert_eq!(saved, "sketchfab-session-v1\n11\nvalid-token");
    assert!(manager
        .invoke_action(
            "sketchfab",
            "sketchfab_sign_in",
            &HashMap::from([("api_token".into(), "bad-token".into())]),
        )
        .await
        .is_err());
    assert_eq!(
        store.load("org.waywallen.sketchfab").unwrap(),
        Some(saved.clone())
    );
    drop(manager);
    let manager = manager_with_entry(root, &entry);
    assert_eq!(
        manager
            .check_lifecycle("sketchfab")
            .await
            .unwrap()
            .unwrap()
            .state,
        PluginLifecycleState::SignedIn
    );
    let sources = manager.discover_sources_with_status().await.unwrap();
    let login = sources[0]
        .actions
        .iter()
        .find(|a| a.id == "sketchfab_sign_in")
        .unwrap();
    let logout = sources[0]
        .actions
        .iter()
        .find(|a| a.id == "sketchfab_sign_out")
        .unwrap();
    assert!(!login.visible && logout.visible && logout.enabled);
    session_test_mode(&manager, "offline", 1061).await;
    let sources = manager.discover_sources_with_status().await.unwrap();
    let logout = sources[0]
        .actions
        .iter()
        .find(|a| a.id == "sketchfab_sign_out")
        .unwrap();
    assert!(logout.visible && logout.enabled);
    assert_eq!(store.load("org.waywallen.sketchfab").unwrap(), Some(saved));
    manager
        .invoke_action("sketchfab", "sketchfab_sign_out", &HashMap::new())
        .await
        .unwrap();
    assert_eq!(
        store.load("org.waywallen.sketchfab").unwrap(),
        Some(String::new())
    );
    drop(manager);
    let manager = manager_with_entry(root, &entry);
    assert_eq!(
        manager
            .check_lifecycle("sketchfab")
            .await
            .unwrap()
            .unwrap()
            .state,
        PluginLifecycleState::SignedOut
    );
}

#[tokio::test]
async fn sketchfab_session_lifecycle_persists_revocation_but_not_transient_errors() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let entry = session_action_entry(root);
    let manager = manager_with_entry(root, &entry);
    let store =
        crate::plugin::state_store::PluginStateStore::new(root.join("state"), root.join("legacy"));
    manager
        .invoke_action(
            "sketchfab",
            "sketchfab_sign_in",
            &HashMap::from([("api_token".into(), "valid-token".into())]),
        )
        .await
        .unwrap();
    let saved = store.load("org.waywallen.sketchfab").unwrap();
    session_test_mode(&manager, "offline", 1061).await;
    assert_eq!(
        manager
            .check_lifecycle("sketchfab")
            .await
            .unwrap()
            .unwrap()
            .state,
        PluginLifecycleState::Error
    );
    assert_eq!(store.load("org.waywallen.sketchfab").unwrap(), saved);
    session_test_mode(&manager, "revoked", 1061).await;
    assert_eq!(
        manager
            .check_lifecycle("sketchfab")
            .await
            .unwrap()
            .unwrap()
            .state,
        PluginLifecycleState::Expired
    );
    assert_eq!(
        store.load("org.waywallen.sketchfab").unwrap(),
        Some(String::new())
    );
    drop(manager);
    let manager = manager_with_entry(root, &entry);
    assert_eq!(
        manager
            .check_lifecycle("sketchfab")
            .await
            .unwrap()
            .unwrap()
            .state,
        PluginLifecycleState::SignedOut
    );
}

#[tokio::test]
#[ignore = "contacts the public Sketchfab API for three anonymous GET requests"]
async fn sketchfab_live_browse() {
    let (_directory, manager) = manager();
    let first = manager
        .call_discover_with_cursor("sketchfab", "landscape", "relevance", 1, &[], "")
        .await
        .unwrap();
    assert!(!first.items.is_empty());
    assert!(first
        .items
        .iter()
        .all(|item| item.wp_type == "model" && item.width.is_none() && item.height.is_none()));
    let cursor = first.next_cursor.as_str();
    assert!(!cursor.is_empty());
    let second = manager
        .call_discover_with_cursor("sketchfab", "landscape", "relevance", 2, &[], cursor)
        .await
        .unwrap();
    assert!(!second.items.is_empty());
    assert_ne!(first.items[0].id, second.items[0].id);
    let detail = manager
        .call_details("sketchfab", &first.items[0].id)
        .await
        .unwrap();
    assert!(detail.web_url.starts_with("https://sketchfab.com/"));
    assert!(!detail.fields.is_empty());
}
