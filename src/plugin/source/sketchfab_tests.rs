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
        .load_plugin(entry, "org.waywallen.sketchfab", "0.1.0", ENTRY_VERSION_V7)
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
    for name in [
        "api",
        "discover",
        "environment",
        "map",
        "presentation",
        "session",
        "source",
        "wallpaper",
    ] {
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
ctx.artifact_resource_path = function(path) return 'prepared/' .. path end
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

fn environment_test(body: &str) {
    lua_test(&[
        r#"
local J = json.strict
local options = J.decode([[{
  "environment":{"uid":"0000000000000000000000000000002a","enable":true,
    "exposure":2,"rotation":0,"blur":0.25,"backgroundExposure":0.5,"shadowEnable":true},
  "background":{"enable":"environment"}
}]])
local serial = 100
local function image(size, blur)
    serial = serial + 1
    return J.object { uid=uid(serial), width=size, height=size, blur=blur, sizeCompressed=100,
        file='https://media.sketchfab.com/environments/'..uid(42)..'/'..serial..'.bin.gz' }
end
local descriptor = J.object { uid=uid(42), name='Shared', processing='Succeeded',
    diffuseSPH=J.array{}, textures=J.array{
        J.object { type='background', format='cubemap', encoding='luv',
            images=J.array{image(128,0.5),image(512,0),image(256,0.5)} },
        J.object { type='specular_ue4', format='cubemap', encoding='luv', limitSize=8,
            images=J.array{image(64),image(2048),image(256),image(1024)} },
        J.object { type='brdf_ue4', format='lut', encoding='rg16', images=J.array{image(128)} }
    } }
for i=1,27 do descriptor.diffuseSPH[i] = 0.1*i end
local payload = J.object { results=J.array{J.object{uid=uid(43),isDefault=true},descriptor} }
local function prepared()
    local output = J.object { background='environment/background.hdr',diffuse='environment/diffuse.hdr',
        brdf='environment/brdf.hdr',specular=J.array{},specular_lod_scale=7,
        specular_lod_exponent=0.5,brdf_filter='nearest' }
    for i=0,10 do output.specular[#output.specular+1] = 'environment/specular-'..i..'.hdr' end
    return J.object {preprocessed=output}
end
"#,
        body,
    ]
    .concat());
}

#[test]
fn sketchfab_environment_resources_preserve_shared_identity_and_model_parameters() {
    environment_test(
        r#"
local data, intent = presentation.from_options(ctx,uid(7),options)
assert(intent and data.sketchfab.profile.environment.ibl_intensity == 0)
local resources, parameters = environment.attach(ctx,data,intent,payload)
local value = data.sketchfab
assert(#resources == 3 and value.environment.uid == uid(42))
assert(value.environment.background.width == 256 and value.environment.background.blur == 0.5)
assert(value.environment.specular.width == 1024 and value.environment.brdf.width == 128)
assert(parameters.specular.limit_size == 8 and intent.mip_count == 11 and intent.lod_scale == 7)
assert(parameters.exposure == nil and parameters.rotation == nil and parameters.token == nil)
assert(value.environment.background.file == nil and value.environment.specular.file == nil)
assert(value.sections.environment.status == 'pending' and value.sections.background.status == 'pending')
local metadata = {external_id=uid(7),provider_data=data}
assert(not pcall(presentation.render_profile,metadata,J))
assert(not J.encode(data):find('media.sketchfab.com',1,true))
local resolved = presentation.finalize(ctx,metadata,prepared())
assert(resolved == data and value.pending_environment == nil)
assert(value.sections.environment.status == 'partial'
    and value.sections.environment.reason == 'environment-probe-shadow-unsupported')
assert(value.sections.background.status == 'mapped')
local profile = value.profile.environment
assert(profile.ibl_intensity == 2 and profile.background_intensity == 1)
assert(profile.reference == 'scene-root' and profile.basis[7] == 1 and profile.basis[10] == -1)
assert(profile.preprocessed.background == 'prepared/environment/background.hdr')
assert(#profile.preprocessed.specular == 11 and profile.preprocessed.brdf_filter == 'nearest')
local encoded = presentation.render_profile(metadata,J)
assert(encoded == presentation.render_profile(metadata,J))
options.environment.exposure, options.environment.rotation = 4, 1
local other, other_intent = presentation.from_options(ctx,uid(8),options)
local other_resources, other_parameters = environment.attach(ctx,other,other_intent,payload)
assert(other.sketchfab.environment.uid == value.environment.uid)
assert(J.encode_canonical(other_parameters) == J.encode_canonical(parameters))
for i, resource in ipairs(resources) do assert(resource.url == other_resources[i].url) end
presentation.finalize(ctx,{provider_data=other},prepared())
assert(other.sketchfab.profile.environment.ibl_intensity == 4)
assert(other.sketchfab.profile.environment.basis[1] ~= profile.basis[1])
assert(other.sketchfab.profile.environment.preprocessed.background == profile.preprocessed.background)
assert(request_count() == 0)
"#,
    );
}

#[test]
fn sketchfab_environment_capture_and_resolve_reuse_the_normalized_snapshot() {
    environment_test(
        r#"
reply(options)
reply(payload)
local data, resources, parameters = presentation.capture(ctx,uid(7))
assert(request_count() == 2 and #resources == 3 and parameters.version == 1)
assert(last_request().path == 'https://sketchfab.com/i/models/'..uid(7)..'/environments')
assert(last_request().query.optimized == '1' and last_request().headers.Authorization == nil)
ctx.fs = {is_file=function(path) return path == '/artifact/scene.gltf' end,
    is_symlink=function() return false end}
local params = {dir='/artifact',id=uid(7),metadata={title='Model',wp_type='gltf',
    external_id=uid(7),provider_data=data},prepared=prepared()}
local result = discover.resolve(ctx,params)
assert(result.resource == 'scene.gltf' and result.provider_data == data and request_count() == 2)
local profile = presentation.render_profile({external_id=uid(7),provider_data=result.provider_data},J)
assert(profile:find('prepared/environment/',1,true))
options.environment.enable = false
options.background = J.object {enable='color',color=J.array{0.2,0.2,0.2}}
reply(options)
data, resources, parameters = presentation.capture(ctx,uid(8))
assert(request_count() == 3 and resources == nil and parameters == nil)
assert(data.sketchfab.sections.environment.status == 'disabled')
assert(data.sketchfab.sections.background.status == 'mapped')
assert(data.sketchfab.profile.environment.background_color[1] > 0)
options.background.enable = 'environment'
reply(options)
reply(payload)
data, resources = presentation.capture(ctx,uid(9))
assert(#resources == 3 and request_count() == 5)
presentation.finalize(ctx,{provider_data=data},prepared())
assert(data.sketchfab.profile.environment.ibl_intensity == 0)
assert(data.sketchfab.profile.environment.background == true)
"#,
    );
}

#[test]
fn sketchfab_environment_rejects_incompatible_descriptors_and_prepared_paths() {
    environment_test(
        r#"
local data, intent = presentation.from_options(ctx,uid(7),options)
local empty = J.object {results=J.array{J.object{uid=uid(43),isDefault=true}}}
assert(environment.attach(ctx,data,intent,empty) == nil)
assert(data.sketchfab.sections.environment.reason == 'environment-id-not-found')
assert(data.sketchfab.pending_environment == nil)
descriptor.textures[2].limitSize = 3
assert(environment.attach(ctx,data,intent,payload) == nil)
assert(data.sketchfab.sections.environment.reason == 'environment-specular-limit-invalid')
descriptor.textures[2].limitSize = 8
local images = descriptor.textures[1].images
images[1].width,images[1].height,images[3].width,images[3].height = 2048,2048,1024,1024
assert(environment.attach(ctx,data,intent,payload) == nil)
assert(data.sketchfab.sections.environment.reason == 'environment-texture-size-unsupported')
images[1].width,images[1].height,images[3].width,images[3].height = 128,128,256,256
local file = images[1].file
images[1].file = 'https://media.sketchfab.com/environments/'..uid(43)..'/resource.bin.gz'
images[3].file = images[1].file
assert(environment.attach(ctx,data,intent,payload) == nil)
assert(data.sketchfab.sections.environment.reason == 'environment-resource-url-invalid')
images[1].file,images[3].file = file,file
local resources = environment.attach(ctx,data,intent,payload)
assert(resources and #resources == 3)
for _, path in ipairs({'../escape.hdr','/tmp/absolute.hdr','a//b.hdr','a/./b.hdr','a\\b.hdr','a:b.hdr'}) do
    local output = prepared()
    output.preprocessed.background = path
    assert(not pcall(presentation.finalize,ctx,{provider_data=data},output))
end
local output = prepared()
output.preprocessed.specular_lod_scale = 10
assert(not pcall(presentation.finalize,ctx,{provider_data=data},output))
output = prepared()
table.remove(output.preprocessed.specular)
assert(not pcall(presentation.finalize,ctx,{provider_data=data},output))
assert(not pcall(presentation.finalize,ctx,{provider_data=data},nil))
presentation.finalize(ctx,{provider_data=data},prepared())
assert(not pcall(presentation.finalize,ctx,{provider_data=data},prepared()))
"#,
    );
}

#[test]
fn sketchfab_plugin_supports_model_downloads_and_installs_translations() {
    let (_directory, manager) = manager();
    assert!(manager.plugins().unwrap().is_empty());
    let sources = manager.discover_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].plugin_id, "sketchfab");
    assert!(sources[0].supports_search);
    assert!(matches!(
        sources[0].remote_capability,
        Some(RemoteCapability::Download)
    ));
    assert!(sources[0].remote_hint.text().is_empty());
    assert!(!sources[0].download_action.enabled);
    assert_eq!(sources[0].download_action.label.text(), "Login required");
    assert_eq!(
        sources[0].download_action.tooltip.text(),
        "Log in with a Sketchfab API Token to download models."
    );
    assert!(sources[0].download_action.label.is_translated());
    assert!(sources[0].download_action.tooltip.is_translated());
    assert!(manager.supports_item_remove("sketchfab"));
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
    let restricted = sources[0]
        .filters
        .iter()
        .find(|filter| filter.id == "restricted")
        .unwrap();
    assert_eq!(restricted.ty, DiscoverFilterType::Toggle);
    assert_eq!(restricted.title.text(), "Show restricted content");
    assert!(restricted.title.is_translated());
    assert_eq!(restricted.options.len(), 1);
    assert_eq!(restricted.options[0].value, "feature:restricted");
    assert!(restricted.options[0].label.is_translated());
    let mut values = HashSet::new();
    for filter in &sources[0].filters {
        assert_ne!(filter.id, "downloadable");
        for option in &filter.options {
            assert_ne!(option.value, "feature:downloadable");
            assert!(values.insert(&option.value));
        }
    }
    let root = entry().parent().unwrap().to_owned();
    let installation = std::fs::read_to_string(root.join("install.lua")).unwrap();
    for path in [
        "plugin.toml",
        "main.lua",
        "sketchfab/api.lua",
        "sketchfab/environment.lua",
        "sketchfab/map.lua",
        "sketchfab/presentation.lua",
        "sketchfab/session.lua",
        "sketchfab/discover.lua",
        "sketchfab/source.lua",
        "sketchfab/wallpaper.lua",
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
assert(q.sort_by == nil and q.downloadable == 'true' and q.page == nil)
assert(last_request().path == 'https://api.sketchfab.com/v3/search')
assert(last_request().headers.Authorization == nil)
reply({ results = {} })
discover.search(ctx, { page = 2, cursor = first.next_cursor, sort = 'recent', tags = {
    'category:nature-plants', 'license:by', 'feature:animated', 'feature:staffpicked'
} })
q = last_request().query
assert(q.cursor == 'opaque+/=' and q.sort_by == '-publishedAt')
assert(q.categories == 'nature-plants' and q.license == 'by')
assert(q.downloadable == 'true' and q.animated == 'true' and q.staffpicked == 'true')
for _, sort in ipairs({ {'likes', '-likeCount'}, {'views', '-viewCount'} }) do
    reply({ results = {} }); api.search(ctx, { sort = sort[1] })
    assert(last_request().query.sort_by == sort[2])
    assert(last_request().query.downloadable == 'true')
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
fn sketchfab_relevance_uses_likes_only_without_search_text() {
    lua_test(
        r#"
reply({results={}})
api.search(ctx,{})
assert(last_request().query.sort_by == '-likeCount')
for _, text in ipairs({'', ' \t\r\n'}) do
    for _, sort in ipairs({'', 'relevance'}) do
        reply({results={}})
        api.search(ctx,{query=text,sort=sort})
        local query = last_request().query
        assert(query.q == text and query.sort_by == '-likeCount' and query.downloadable == 'true')
        reply({results={}})
        api.search(ctx,{query=text,sort=sort,page=2,cursor='next-page'})
        query = last_request().query
        assert(query.cursor == 'next-page' and query.sort_by == '-likeCount')
    end
end
for _, text in ipairs({'ship', '风景', ' ship '}) do
    for _, sort in ipairs({'', 'relevance'}) do
        reply({results={}})
        api.search(ctx,{query=text,sort=sort})
        local query = last_request().query
        assert(query.q == text and query.sort_by == nil and query.downloadable == 'true')
    end
end
for _, sort in ipairs({{'recent','-publishedAt'}, {'views','-viewCount'}, {'likes','-likeCount'}}) do
    reply({results={}})
    api.search(ctx,{query='',sort=sort[1]})
    assert(last_request().query.sort_by == sort[2])
end
"#,
    );
}

#[test]
fn sketchfab_restricted_content_is_excluded_unless_selected() {
    lua_test(
        r#"
for _, params in ipairs({{}, {tags={}}, {page=2,cursor='next-page'}}) do
    reply({results={}})
    discover.search(ctx,params)
    local query = last_request().query
    assert(query.restricted == 'false' and query.downloadable == 'true')
    assert(last_request().headers.Authorization == nil)
end
for _, sort in ipairs({'relevance', 'recent', 'likes', 'views'}) do
    for page = 1, 2 do
        reply({results={}})
        discover.search(ctx,{query='ship',sort=sort,page=page,cursor=page == 2 and 'next-page' or nil,tags={
            'feature:restricted', 'category:places-travel', 'license:by', 'feature:animated', 'feature:staffpicked'
        }})
        local query = last_request().query
        assert(query.restricted == 'true' and query.downloadable == 'true')
        assert(query.categories == 'places-travel' and query.license == 'by')
        assert(query.animated == 'true' and query.staffpicked == 'true')
        assert(query.cursor == (page == 2 and 'next-page' or nil))
    end
end
reply({results={}})
discover.search(ctx,{tags={}})
assert(last_request().query.restricted == 'false')
local before = request_count()
assert(not pcall(api.search,ctx,{tags={'feature:restricted','feature:restricted'}}))
assert(request_count() == before)
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
fn sketchfab_legacy_model_ids_work_across_search_details_camera_and_download() {
    lua_test(
        r#"
local id = '7w7pAfrCfjovwykkEeRFLGw5SXS'
reply({results={{uid=id,name='bob'},model(1)},cursors={next='next-page'}})
local result = discover.search(ctx,{})
assert(#result.items == 2 and result.items[1].id == id and result.items[2].id == uid(1))
assert(result.has_more and result.next_cursor == 'next-page')
reply({uid=id,name='bob',description='Legacy model'})
assert(discover.details(ctx,id).description == 'Legacy model')
assert(last_request().path == 'https://api.sketchfab.com/v3/models/'..id)
reply({camera={position={1,2,3},target={0,0,0},fov=45}})
assert(api.viewer_options(ctx,id).camera.fov == 45)
assert(last_request().path == 'https://sketchfab.com/i/models/'..id..'/options')
reply({uid=uid(1),username='artist'})
session.sign_in(ctx,'test-token')
reply({uid=id,name='bob',user={username='artist'},license={slug='by'},isDownloadable=true})
reply({camera={position={1,2,3},target={0,0,0},fov=45}})
reply({gltf={url='https://cdn.example/model.zip',expires=300,size=42}})
local download = discover.download(ctx,id)
assert(download.external_id == id and download.filename == id..'.zip' and download.archive == 'zip')
assert(download.provider_data.sketchfab.profile.camera.position[2] == 2)
assert(last_request().path == 'https://api.sketchfab.com/v3/models/'..id..'/download')
for _, invalid in ipairs({
    '', 'bad', string.rep('a',26), string.rep('a',28), string.rep('g',32),
    string.rep('a',26)..'_', string.rep('a',26)..'/', string.rep('a',26)..'?',
    string.rep('a',26)..'\n', '../'..string.rep('a',24), false, 27, json.strict.null,
}) do
    local before = request_count()
    assert(not pcall(normalize,{uid=invalid}))
    assert(not pcall(api.model,ctx,invalid))
    assert(not pcall(api.viewer_options,ctx,invalid))
    assert(not pcall(api.download,ctx,invalid,session.headers()))
    assert(request_count() == before)
end
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
fn sketchfab_download_requires_login_and_preserves_visible_attribution() {
    lua_test(
        r#"
local ok, message = pcall(discover.download, ctx, uid(1))
assert(not ok and message:find('Log in to Sketchfab', 1, true))
assert(request_count() == 0)
assert(not pcall(api.download, ctx, uid(1), {}))
assert(request_count() == 0)
reply({uid=uid(1),username='artist'})
session.sign_in(ctx,'test-token')
search({model(1, {
    name='Cached title', user={username='artist',profileUrl='https://sketchfab.com/artist'},
    license={label='CC Attribution',slug='by',url='https://creativecommons.org/licenses/by/4.0/',requirements='Credit the author'},
    tags={{name='Landscape'}}, archives={gltf={size=100}}
})})
reply(model(1, {name='Current title',description='Current description',isDownloadable=true,isAgeRestricted=false,
    thumbnails={images={{url='https://images.example/preview.jpg',width=512,height=512}}}}))
reply({camera={position={1,2,3},target={0,0,0},fov=45,up=1.5},
    orientation={matrix={1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1}}})
reply({gltf={url='https://cdn.example/first.zip?signature=temporary',expires=300,size=200}})
local result = discover.download(ctx,uid(1))
assert(result.wp_type == 'gltf' and result.archive == 'zip' and result.filename == uid(1)..'.zip')
assert(result.title == 'Current title' and result.external_id == uid(1) and result.size == 200)
assert(result.preview_url == 'https://images.example/preview.jpg' and result.content_rating == 'Everyone')
assert(result.tags[1] == 'Landscape' and result.width == nil and result.height == nil)
assert(result.attribution.author == 'artist' and result.attribution.license == 'CC Attribution')
assert(result.attribution.author_url == 'https://sketchfab.com/artist')
assert(result.attribution.license_url == 'https://creativecommons.org/licenses/by/4.0/')
assert(result.attribution.requirements == 'Credit the author')
assert(result.provider_data.sketchfab.version == 2)
assert(result.provider_data.sketchfab.profile.camera.up[3] == 1)
assert(result.provider_data.sketchfab.profile.camera.position[2] == 2)
assert(result.provider_data.sketchfab.sections.orientation.status == 'mapped')
local preset = json.strict.decode('{"camera":{"position":[1,2,3],"target":[0,0,0],"fov":45}}')
preset.camera.fov = 180
assert(presentation.from_options(ctx,uid(1),preset).sketchfab.sections.camera.status == 'invalid')
preset.camera.fov = 45
preset.camera.position = json.strict.array{1,2}
assert(presentation.from_options(ctx,uid(1),preset).sketchfab.sections.camera.status == 'invalid')
preset.camera.position = json.strict.array{1,2,3}
preset.camera.oversized = string.rep('x',65536)
local normalized = presentation.from_options(ctx,uid(1),preset)
assert(normalized.sketchfab.profile.camera.position[1] == 1)
assert(not json.strict.encode(normalized):find('oversized',1,true))
assert(result.description:find('Current description',1,true))
assert(result.description:find('Provided by Sketchfab',1,true))
assert(result.description:find('Author: artist',1,true) and result.description:find('License: CC Attribution',1,true))
assert(not result.description:find('signature',1,true) and not result.description:find('test-token',1,true))
assert(last_request().path == 'https://api.sketchfab.com/v3/models/'..uid(1)..'/download')
assert(last_request().headers.Authorization == 'Token test-token')
local before = request_count()
advance(301)
reply(model(1, {name='Retried',user={username='artist'},license={slug='by'},isDownloadable=true}))
reply({},404)
reply({gltf={url='https://cdn.example/retry.zip?signature=fresh',expires=60}})
local retried = discover.download(ctx,uid(1))
assert(request_count() == before+3 and retried.url ~= result.url)
assert(retried.url == 'https://cdn.example/retry.zip?signature=fresh')
assert(retried.title == 'Retried' and retried.size == nil)
assert(retried.provider_data.sketchfab.profile == json.strict.null)
assert(retried.provider_data.sketchfab.sections.options.status == 'unavailable')
assert(last_request().headers.Authorization == 'Token test-token')
reply({},429,'60')
assert(not pcall(api.viewer_options,ctx,uid(1)))
assert(last_request().path == 'https://sketchfab.com/i/models/'..uid(1)..'/options')
assert(last_request().headers.Authorization == nil)
reply({gltf={url='https://cdn.example/model.zip',expires=300,size=42}})
assert(api.download(ctx,uid(1),session.headers()).filename == uid(1)..'.zip')
local camera_requests = request_count()
assert(not pcall(api.viewer_options,ctx,uid(1)) and request_count() == camera_requests)
"#,
    );
}

#[test]
fn sketchfab_presentation_converts_sections_without_requiring_a_camera() {
    lua_test(
        r#"
local J = json.strict
local options = J.decode([[{
  "lighting":{"enable":true,"lights":[
    {"enable":true,"type":"DIRECTION","attachedToCamera":false,"matrix":[1,0,0,0,0,1,0,0,0,0,1,0,4,5,6,1],"color":[0.2,0.4,0.6],"intensity":2,"castShadows":true,"shadowBias":0.005},
    {"enable":true,"type":"POINT","attachedToCamera":false,"matrix":[1,0,0,0,0,1,0,0,0,0,1,0,4,5,6,1],"color":[1,0,0],"intensity":0,"falloff":86.89857508671155},
    {"enable":false,"type":"INVALID","matrix":false}
  ]},
  "environment":{"enable":false,"exposure":99,"shadowEnable":true},
  "background":{"enable":"color","color":[0.26754385964912286,0.26754385964912286,0.26754385964912286]},
  "scene":{"postProcess":{"enable":true,
    "toneMapping":{"enable":true,"method":"default","exposure":0,"brightness":-1,"saturation":0,"contrast":-1},
    "bloom":{"enable":true,"factor":0.2,"threshold":0.7,"radius":0.1},
    "colorBalance":{"enable":true,"low":[0,0,0],"mid":[0,0,-0.06666666666666665],"high":[-0.01,0,0]},
    "vignette":{"enable":true,"amount":0.32222222222222224,"hardness":0.725},
    "sharpen":{"enable":true,"factor":0},
    "grain":{"enable":true,"factor":0.025,"animated":false},
    "taa":{"enable":true},"ssao":{"enable":false,"intensity":"ignored"}
  }},
  "ground":{"enable":true,"shadowMode":"ShadowCatcher","position":[0,0,-0.0255],"scale":3.3744694771942068,"opacity":0.8177777777777777,"fade":0.23209876543209876},
  "materials":{"unknown-id":{"Opacity":{"type":"dithering"},"token":"must-not-persist"}}
}]])
local data = presentation.from_options(ctx,uid(1),options)
local value, profile = data.sketchfab, data.sketchfab.profile
assert(value.version == 2 and value.adapter_version == 1 and value.model_id == uid(1))
assert(value.source == 'https://sketchfab.com/i/models/'..uid(1)..'/options' and value.captured_at == 1000)
assert(profile.version == 2 and profile.camera == nil and value.sections.camera.status == 'missing')
assert(value.sections.lighting.status == 'mapped' and value.sections.lighting.reason == nil)
assert(value.sections.lighting.items[1].reason == nil)
assert(#profile.directional_lights == 1 and #profile.point_lights == 1)
local directional, point = profile.directional_lights[1], profile.point_lights[1]
assert(directional.reference == 'scene-root' and directional.direction[3] == 1)
assert(math.abs(directional.intensity - 2*math.pi) < 1e-12 and directional.color[1] == 0.2)
assert(directional.shadow.resolution == 1024 and directional.shadow.filter == 'bilinear')
assert(directional.shadow.normal_bias == 0 and directional.shadow.max_bias == 0.01)
assert(point.position[1] == 4 and point.position[2] == 5 and point.position[3] == 6)
assert(point.intensity == 0 and point.attenuation[3] == 86.89857508671155)
assert(point.attenuation_units == 'reference')
assert(profile.environment.ibl_intensity == 0 and profile.environment.background == false)
assert(value.sections.environment.status == 'disabled')
assert(math.abs(profile.environment.background_color[1] - 0.05818448107355388) < 1e-12)
assert(profile.display.exposure == 0 and profile.display.brightness == 0)
assert(profile.display.saturation == 0 and profile.display.contrast == -1 and profile.display.sharpen == 0)
assert(profile.display.tone_mapping == 'linear' and profile.display.grain.phase == 0)
assert(profile.display.color_balance.midtones[3] == -0.06666666666666665)
assert(profile.display.bloom.intensity == 0.2 and profile.display.bloom.radius == 0.1)
assert(math.abs(profile.display.vignette.inner_radius - 0.40277777777777773) < 1e-12)
assert(math.abs(profile.shadow_catcher.opacity - 0.6687604938271604) < 1e-12)
assert(math.abs(profile.shadow_catcher.fade - 0.7064704232352543) < 1e-12)
assert(value.sections.post_process.items.taa.status == 'unsupported')
assert(value.sections.post_process.items.ssao.status == 'disabled')
assert(value.sections.post_process.items.ssao.reason == nil)
assert(value.sections.post_process.items.bloom.reason == nil)
assert(value.sections.materials.status == 'unsupported' and profile.material_overrides == nil)
local encoded = J.encode(data)
assert(#encoded < 64*1024 and not encoded:find('must-not-persist',1,true))
options.camera = J.null
assert(presentation.from_options(ctx,uid(1),options).sketchfab.sections.camera.status == 'null')
options.camera = J.object {position=J.array{1,2,3},target=J.array{0,0,0},fov=45}
for _, value in ipairs({false,'45',J.null,0/0,math.huge,1e40}) do
    options.camera.fov = value
    local data = presentation.from_options(ctx,uid(1),options).sketchfab
    assert(data.sections.camera.status == 'invalid' and data.profile.camera == nil)
    assert(data.profile.display.bloom.intensity == 0.2)
end
options.camera.fov = 45
options.orientation = J.object{matrix=J.array{0,-1,0,0,1,0,0,0,0,0,1,0,0,0,0,1}}
options.lighting.lights[1].attachedToCamera = true
data = presentation.from_options(ctx,uid(1),options).sketchfab
assert(data.sections.orientation.status == 'unsupported' and data.sections.camera.status == 'unsupported')
assert(data.profile.directional_lights[1].reference == 'view' and #data.profile.point_lights == 0)
assert(data.sections.ground.status == 'unsupported' and data.profile.shadow_catcher == nil)
options.environment.enable = true
options.background.enable = 'environment'
data = presentation.from_options(ctx,uid(1),options).sketchfab
assert(data.sections.environment.status == 'unsupported' and data.sections.background.status == 'unsupported')
assert(data.profile.environment.ibl_intensity == 0 and data.profile.environment.background == false)
options.lighting.enable, options.scene.postProcess.enable, options.ground.enable = false, false, false
data = presentation.from_options(ctx,uid(1),options).sketchfab
assert(#data.profile.directional_lights == 0 and #data.profile.point_lights == 0)
assert(data.sections.lighting.status == 'disabled' and data.sections.ground.status == 'disabled')
assert(data.sections.post_process.status == 'disabled' and data.profile.display.bloom == nil)
assert(J.kind(data.profile.directional_lights) == 'array' and J.kind(data.sections) == 'object')
assert(presentation.from_options(ctx,uid(1),J.null).sketchfab.profile == J.null)
"#,
    );
}

#[test]
fn sketchfab_render_profiles_are_stable_current_snapshots() {
    lua_test(
        r#"
local J = json.strict
local a = J.decode('{"camera":{"position":[1,2,3],"target":[0,0,0],"fov":45},"environment":{"enable":false}}')
local b = J.decode('{"environment":{"enable":false,"unused":"ignored"},"camera":{"fov":45.0,"target":[-0.0,0.0,0.0],"position":[1.0,2.0,3.0]}}')
local metadata = {external_id=uid(1),provider_data=presentation.from_options(ctx,uid(1),a)}
local first = presentation.render_profile(metadata,J)
advance(100)
metadata.provider_data = presentation.from_options(ctx,uid(1),b)
local second = presentation.render_profile(metadata,J)
assert(first == second and not first:find('captured_at',1,true) and not first:find(uid(1),1,true))
metadata = J.decode(J.encode(metadata))
assert(first == presentation.render_profile(metadata,J))
metadata.external_id = uid(2)
assert(not pcall(presentation.render_profile,metadata,J))
metadata.external_id = uid(1)
metadata.provider_data.sketchfab.adapter_version = 2
assert(not pcall(presentation.render_profile,metadata,J))
metadata.provider_data.sketchfab.adapter_version = 1
metadata.provider_data.sketchfab.profile.oversized = string.rep('x',65536)
assert(not pcall(presentation.render_profile,metadata,J))
metadata.provider_data.sketchfab.profile = J.null
assert(presentation.render_profile(metadata,J) == nil)
local logs = {}
ctx.log = function(message) logs[#logs+1] = message end
reply({},404)
local before = request_count()
local unavailable = presentation.capture(ctx,uid(1)).sketchfab
assert(request_count() == before+1 and #logs == 1)
assert(unavailable.profile == J.null and unavailable.sections.options.status == 'unavailable')
assert(last_request().headers.Authorization == nil)
assert(not J.encode(unavailable):find('test-token',1,true))
"#,
    );
}

#[test]
fn sketchfab_download_rejects_invalid_archive_data_without_reusing_urls() {
    lua_test(
        r#"
local headers = {Authorization='Token test-token'}
reply({glb={url='https://cdn.example/model.glb',expires=300,size=1000},
    gltf={url='https://cdn.example/model.zip',expires=300,size=900}})
local chosen = api.download(ctx,uid(1),headers)
assert(chosen.url == 'https://cdn.example/model.zip' and chosen.size == 900)
assert(chosen.filename == uid(1)..'.zip' and chosen.archive == 'zip')
reply({glb=false,gltf={url='https://cdn.example/other.zip',expires=300,size=200}})
assert(api.download(ctx,uid(1),headers).url == 'https://cdn.example/other.zip')
for _, value in ipairs({
    {glb={url='https://cdn.example/model.glb',expires=300,size=42}},
    {}, {gltf=json.strict.null}, {gltf=false}, {gltf={}},
    {gltf={url='http://cdn.example/model.zip',expires=300}},
    {gltf={url='file:///model.zip',expires=300}},
    {gltf={url='https://',expires=300}},
    {gltf={url='https://cdn.example/model.zip#fragment',expires=300}},
    {gltf={url='https://cdn.example/model.zip\nbad',expires=300}},
    {gltf={url='https://cdn.example/'..string.rep('a',16384),expires=300}},
    {gltf={url='https://cdn.example/model.zip'}},
    {gltf={url='https://cdn.example/model.zip',expires=0}},
    {gltf={url='https://cdn.example/model.zip',expires=-1}},
    {gltf={url='https://cdn.example/model.zip',expires='300'}},
    {gltf={url='https://cdn.example/model.zip',expires=300,size=-1}},
    {gltf={url='https://cdn.example/model.zip',expires=300,size=1.5}},
    {gltf={url='https://cdn.example/model.zip',expires=300,size='200'}},
    {gltf={url='https://cdn.example/model.zip',expires=300,size=9007199254740992}},
}) do
    reply(value)
    local ok, message = pcall(api.download,ctx,uid(1),headers)
    assert(not ok and not message:find('test-token',1,true))
    assert(last_request().response.closed)
end
reply({gltf={url='https://cdn.example/fresh.zip',expires=0.5,size=0}})
local value = api.download(ctx,uid(1),headers)
assert(value.url == 'https://cdn.example/fresh.zip' and value.size == 0)
reply({gltf={url='https://cdn.example/fresh.zip',expires=300,size=json.strict.null}})
assert(api.download(ctx,uid(1),headers).size == nil)
for _, id in ipairs({'bad','../model',string.rep('g',32)}) do
    local before = request_count()
    assert(not pcall(api.download,ctx,id,headers) and request_count() == before)
end
"#,
    );
}

#[test]
fn sketchfab_download_errors_are_specific_and_do_not_disclose_credentials() {
    lua_test(
        r#"
for _, failure in ipairs({
    {400,'not downloadable'}, {401,'authentication failed'},
    {403,'does not have permission'}, {404,'not found'}, {500,'HTTP error 500'},
}) do
    reply('{}',failure[1])
    local ok,message = pcall(api.download,ctx,uid(1),{Authorization='Token test-token'})
    assert(not ok and message:find(failure[2],1,true),message)
    assert(not message:find('test-token',1,true) and last_request().response.closed)
end
reply('{}',429,'60')
local ok,message = pcall(api.download,ctx,uid(1),{Authorization='Token test-token'})
assert(not ok and message:find('rate limit',1,true) and last_request().response.closed)
local before = request_count()
assert(not pcall(api.download,ctx,uid(1),{Authorization='Token test-token'}))
assert(request_count() == before)
advance(60)
reply('{}',200,nil,nil,'Authorization: Token test-token https://api.sketchfab.com/request')
ok,message = pcall(api.download,ctx,uid(1),{Authorization='Token test-token'})
assert(not ok and message:find('request failed',1,true) and not message:find('test-token',1,true))
"#,
    );
}

#[test]
fn sketchfab_download_requires_consistent_model_author_and_license_metadata() {
    lua_test(
        r#"
reply({uid=uid(1),username='artist'})
session.sign_in(ctx,'test-token')
for _, fields in ipairs({
    {name='No attribution'}, {user={username='artist'}}, {license={slug='by'}},
    {user={username='artist'},license={slug='by'},isDownloadable=false},
}) do
    session.sign_out()
    reply({uid=uid(1),username='artist'})
    session.sign_in(ctx,'test-token')
    local before = request_count()
    reply(model(1,fields))
    assert(not pcall(discover.download,ctx,uid(1)))
    assert(request_count() == before+1)
end
local before = request_count()
reply(model(2,{user={username='artist'},license={slug='by'},isDownloadable=true}))
assert(not pcall(discover.download,ctx,uid(1)) and request_count() == before+1)
"#,
    );
}

#[test]
fn sketchfab_resolve_uses_download_metadata_and_rejects_ambiguous_archives() {
    lua_test(
        r#"
local files,dirs,links = {},{},{}
ctx.fs = {
    is_file=function(path) return files[path] == true end,
    is_symlink=function(path) return links[path] == true end,
    list_files=function(path)
        local out={}
        for file in pairs(files) do if file:match('^(.*)/[^/]+$') == path then out[#out+1]=file end end
        table.sort(out)
        return out
    end,
}
ctx.list_dirs = function(path) return dirs[path] or {} end
ctx.extension = function(path) return path:match('%.([^./]+)$') end
local metadata = {wp_type='gltf',title='Model',external_id=uid(1),description='Author and license',tags={'tag'},size=42,content_rating='Mature'}
local params = {id=uid(1),dir='/download',metadata=metadata}
files['/download/scene.gltf']=true
files['/download/extra.gltf']=true
local result = discover.resolve(ctx,params)
assert(result.resource == 'scene.gltf' and result.name == metadata.title and result.wp_type == 'gltf')
assert(result.description == metadata.description and result.tags[1] == 'tag')
assert(result.external_id == uid(1) and result.size == 42 and result.content_rating == 'Mature')
assert(result.preview == nil and request_count() == 0)
files['/download/scene.glb']=true
assert(discover.resolve(ctx,params).resource == 'scene.gltf')
files['/download/scene.gltf']=nil
assert(discover.resolve(ctx,params).resource == 'scene.glb')
files={['/download/model.glb']=true}
assert(discover.resolve(ctx,params).resource == 'model.glb')
files['/download/other.glb']=true
assert(not pcall(discover.resolve,ctx,params))
files = {['/download/nested/model.gltf']=true}
dirs = {['/download']={'/download/nested'}}
result = discover.resolve(ctx,params)
assert(result.resource == 'nested/model.gltf' and request_count() == 0)
files['/download/model.glb']=true
assert(discover.resolve(ctx,params).resource == 'nested/model.gltf')
files['/download/other.gltf']=true
assert(not pcall(discover.resolve,ctx,params))
files={}
assert(not pcall(discover.resolve,ctx,params))
files={['/download/nested/model.gltf']=true}
links['/download/nested/model.gltf']=true
assert(not pcall(discover.resolve,ctx,params))
links={['/download/nested']=true}
assert(not pcall(discover.resolve,ctx,params))
assert(not pcall(discover.resolve,ctx,{id=uid(2),dir='/download',metadata=metadata}))
assert(not pcall(discover.resolve,ctx,{id=uid(1),dir='/download'}))
assert(request_count() == 0)
"#,
    );
}

#[test]
fn sketchfab_local_model_apply_and_remove_use_public_resource_interfaces() {
    lua_test(
        r#"
local path='/remote/sketchfab/item/scene.gltf'
local metadata = json.strict.object {
    external_id = uid(1),
    provider_data = presentation.from_options(ctx,uid(1),json.strict.decode('{"camera":{"position":[1,2,3],"target":[4,5,6],"fov":45,"up":1.5}}')),
}
local artifact_reads = 0
ctx.remote_artifact=function(resource)
    assert(resource == path)
    artifact_reads = artifact_reads + 1
    return {root='/storage/opaque-package-root',metadata=metadata}
end
ctx.remote_artifact_root=function() error('must reuse the parsed artifact') end
local applied = wallpaper.apply({resource=path},ctx)
assert(artifact_reads == 1)
assert(applied.extras.path == path and next(applied.default_user_properties) == nil)
assert(applied.extras['asset-root'] == '/storage/opaque-package-root')
assert(applied.extras['camera-preset'] == nil)
local profile = json.strict.decode(applied.extras['render-profile'])
local camera = profile.camera
assert(profile.version == 2 and camera.reference == 'scene-root')
assert(camera.position[1] == 1 and camera.position[2] == 2 and camera.position[3] == 3)
assert(camera.target[1] == 4 and camera.target[2] == 5 and camera.target[3] == 6)
assert(camera.up[1] == 0 and camera.up[2] == 0 and camera.up[3] == 1)
assert(math.abs(camera.yfov - math.pi/4) < 1e-12)
metadata.provider_data.sketchfab.version = 1
assert(not pcall(wallpaper.apply,{resource=path},ctx))
metadata.provider_data.sketchfab.version = 2
metadata.provider_data.sketchfab.profile = json.strict.null
assert(wallpaper.apply({resource=path},ctx).extras['render-profile'] == nil)
metadata.provider_data.sketchfab.profile = json.strict.object{version=1}
assert(not pcall(wallpaper.apply,{resource=path},ctx))
metadata.provider_data = nil
assert(not pcall(wallpaper.apply,{resource=path},ctx))
ctx.remote_artifact=function() error('resource is not an installed artifact') end
assert(not pcall(wallpaper.apply,{resource=path},ctx))
local removed
ctx.remove_remote_artifact=function(resource) removed=resource end
ctx.remove_dir=function() error('must not infer a directory to remove') end
ctx.remove_file=function() error('must not remove only the model entry') end
source.remove(ctx,{resource=path,path='/incorrect/path'})
assert(removed == path)
source.remove(ctx,{path=path})
assert(removed == path)
assert(not pcall(source.remove,ctx,{}))
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
    for module in [
        "api",
        "discover",
        "environment",
        "map",
        "presentation",
        "session",
        "source",
        "wallpaper",
    ] {
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

#[tokio::test]
async fn sketchfab_public_download_and_resolve_preserve_the_same_model_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let entry = session_action_entry(root);
    let callbacks = std::fs::read_to_string(&entry).unwrap();
    std::fs::write(
        &entry,
        format!(
            r#"
local api = import('sketchfab.api')
local download_test_model_calls = 0
api.model = function(ctx, id)
    download_test_model_calls = download_test_model_calls + 1
    assert(download_test_model_calls == 1, 'resolve must reuse download metadata')
    return ctx.json.strict.decode('{{"uid":"' .. id .. '","name":"Downloaded model","description":"Model description","isDownloadable":true,"user":{{"username":"artist","profileUrl":"https://sketchfab.com/artist"}},"license":{{"label":"CC Attribution","url":"https://creativecommons.org/licenses/by/4.0/","requirements":"Credit the author"}},"tags":[{{"name":"Landscape"}}]}}')
end
api.download = function(ctx, id, headers)
    assert(headers.Authorization == 'Token valid-token')
    return {{url='https://cdn.example/model.zip?signature=ephemeral',filename=id..'.zip',archive='zip',size=42}}
end
api.viewer_options = function(ctx, id)
    return ctx.json.strict.decode('{{"camera":{{"position":[1,2,3],"target":[0,0,0],"fov":45}}}}')
end
{callbacks}
"#
        ),
    )
    .unwrap();
    let manager = manager_with_entry(root, &entry);
    manager
        .invoke_action(
            "sketchfab",
            "sketchfab_sign_in",
            &HashMap::from([("api_token".into(), "valid-token".into())]),
        )
        .await
        .unwrap();
    let id = "00000000000000000000000000000001";
    let download = manager.call_download("sketchfab", id).await.unwrap();
    assert_eq!(download.transfer.format, DownloadFormat::Zip);
    assert!(download.transfer.resolve);
    assert_eq!(download.transfer.filename, format!("{id}.zip"));
    assert!(download.transfer.url.contains("signature=ephemeral"));
    assert_eq!(download.metadata.title, "Downloaded model");
    assert_eq!(download.metadata.wp_type, "gltf");
    assert_eq!(download.metadata.size, Some(42));
    let attribution = download.metadata.attribution.as_ref().unwrap();
    assert_eq!(attribution.author, "artist");
    assert_eq!(attribution.license, "CC Attribution");
    assert_eq!(attribution.requirements, "Credit the author");
    let persistent = serde_json::to_string(&download.metadata).unwrap();
    assert!(!persistent.contains("signature=ephemeral"));
    assert!(!persistent.contains("valid-token"));
    let snapshot = &download.metadata.provider_data.as_ref().unwrap()["sketchfab"];
    assert_eq!(snapshot["version"], 2);
    assert_eq!(snapshot["profile"]["version"], 2);
    assert_eq!(snapshot["profile"]["camera"]["reference"], "scene-root");
    let artifact = root.join("artifact");
    std::fs::create_dir(&artifact).unwrap();
    std::fs::write(
        artifact.join("scene.gltf"),
        b"model parsing belongs to vecgltf",
    )
    .unwrap();
    std::fs::write(artifact.join("scene.bin"), b"model buffer").unwrap();
    std::fs::create_dir(artifact.join("textures")).unwrap();
    std::fs::write(artifact.join("textures/color.png"), b"model texture").unwrap();
    let resolved = manager
        .call_resolve(
            "sketchfab",
            id,
            artifact.to_str().unwrap(),
            Some(&download.metadata),
        )
        .await
        .unwrap();
    assert_eq!(resolved.name, download.metadata.title);
    assert_eq!(resolved.resource, "scene.gltf");
    assert_eq!(resolved.description, download.metadata.description);
    assert_eq!(resolved.tags, download.metadata.tags);
    assert_eq!(resolved.size, Some(42));
    assert_eq!(resolved.external_id, id);
    assert!(resolved.preview.is_none());
    assert!(artifact.join("scene.bin").is_file());
    assert!(artifact.join("textures/color.png").is_file());
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
    assert!(!sources[0].download_action.enabled);
    assert_eq!(sources[0].download_action.label.text(), "Login required");
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
    assert!(sources[0].download_action.enabled);
    assert!(sources[0].download_action.label.text().is_empty());
    assert!(sources[0].download_action.tooltip.text().is_empty());
    assert!(sources[0].remote_hint.text().is_empty());
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
    assert!(sources[0].download_action.enabled);
    assert!(sources[0].download_action.label.text().is_empty());
    assert!(sources[0].download_action.tooltip.text().is_empty());
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
    let sources = manager.discover_sources_with_status().await.unwrap();
    assert!(!sources[0].download_action.enabled);
    assert_eq!(sources[0].download_action.label.text(), "Login required");
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
    let sources = manager.discover_sources_with_status().await.unwrap();
    assert!(!sources[0].download_action.enabled);
    assert_eq!(sources[0].download_action.label.text(), "Login required");
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
#[ignore = "copies an explicitly selected installed model into an isolated output directory"]
async fn sketchfab_saved_presentation_artifact_exports_render_profile() {
    let configured = |key| PathBuf::from(std::env::var_os(key).expect(key));
    let output = configured("WAYWALLEN_SKETCHFAB_OUTPUT_DIR")
        .canonicalize()
        .unwrap();
    assert!(output.is_dir());
    assert_eq!(
        configured("XDG_DATA_HOME"),
        output.join("data"),
        "run this test in its own process with an isolated XDG_DATA_HOME"
    );
    let id = std::env::var("WAYWALLEN_SKETCHFAB_MODEL_ID").unwrap();
    let source_root = configured("WAYWALLEN_SKETCHFAB_SOURCE_ROOT")
        .canonicalize()
        .unwrap();
    let original = crate::remote_asset::load(&source_root, "sketchfab", &id)
        .unwrap()
        .expect("the selected model must already be an installed artifact");
    assert!(!output.starts_with(&original.root));
    let options_path = configured("WAYWALLEN_SKETCHFAB_OPTIONS_PATH");
    assert!(std::fs::metadata(&options_path).unwrap().len() <= 4 * 1024 * 1024);
    let raw = std::fs::read(options_path).unwrap();
    let manager = manager_at(&output.join("plugin-state"));
    let provider_data = {
        let runtime = manager.test_runtime("sketchfab");
        let runtime = runtime.lock().await;
        let environment = runtime
            .plugin_lua_env(entry().parent().unwrap(), "org.waywallen.sketchfab")
            .unwrap();
        let import: LuaFunction = environment.get("import").unwrap();
        let presentation: LuaTable = import.call("sketchfab.presentation").unwrap();
        let normalize: LuaFunction = presentation.get("from_options").unwrap();
        let json = mlua_extra::json::create_nullable_module(&runtime.lua).unwrap();
        json.set(
            "strict",
            mlua_extra::json::strict::create_module(&runtime.lua).unwrap(),
        )
        .unwrap();
        let context = runtime.lua.create_table().unwrap();
        context.set("json", json).unwrap();
        let time = runtime.lua.create_table().unwrap();
        time.set(
            "unix",
            runtime
                .lua
                .create_function(|_, ()| {
                    Ok(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs())
                })
                .unwrap(),
        )
        .unwrap();
        context.set("time", time).unwrap();
        let options = mlua_extra::json::strict::decode(&runtime.lua, &raw).unwrap();
        let (value, environment): (LuaValue, LuaValue) =
            normalize.call((context, id.as_str(), options)).unwrap();
        assert!(
            environment.is_nil(),
            "models using environment resources must use the managed preparation integration test"
        );
        let value: serde_json::Value =
            serde_json::from_slice(&mlua_extra::json::strict::encode(&value).unwrap()).unwrap();
        value.as_object().unwrap().clone()
    };
    let target_root = crate::settings::remote_content_dir("sketchfab");
    assert!(!target_root.join(&id).exists());
    let staged = crate::remote_asset::StagedArtifact::create(&target_root).unwrap();
    for file in &original.artifact.files {
        let source = crate::remote_asset::validate_file(&original.root, &file.path).unwrap();
        let destination = staged.content().join(&file.path);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        assert_eq!(std::fs::copy(source, destination).unwrap(), file.bytes);
    }
    let mut metadata = original.artifact.metadata.clone();
    metadata.provider_data = Some(provider_data);
    let installed = staged
        .prepare(
            "sketchfab",
            &id,
            &original.artifact.resource,
            original.preview().as_deref(),
            metadata,
            original.artifact.files.clone(),
        )
        .unwrap()
        .publish(&target_root)
        .unwrap()
        .commit();
    let restored = crate::remote_asset::load(&target_root, "sketchfab", &id)
        .unwrap()
        .unwrap();
    let metadata = &restored.artifact.metadata;
    let entry = WallpaperEntry {
        item_id: 1,
        name: metadata.title.clone(),
        wp_type: metadata.wp_type.clone(),
        resource: installed.resource().to_str().unwrap().into(),
        preview: installed
            .preview()
            .map(|path| path.to_str().unwrap().into()),
        description: Some(metadata.description.clone()),
        tags: metadata.tags.clone(),
        external_id: Some(metadata.external_id.clone()),
        web_url: Some(metadata.web_url.clone()),
        update_available: false,
        size: metadata.size,
        width: None,
        height: None,
        content_rating: metadata.content_rating.clone(),
        can_remove: Some(true),
        modified_at: None,
        create_at: 0,
        plugin_name: "sketchfab".into(),
        library_root: target_root.to_str().unwrap().into(),
        hidden: false,
    };
    let applied = manager.call_apply("sketchfab", &entry).await.unwrap();
    assert_eq!(applied.extras["path"], entry.resource);
    assert_eq!(
        applied.extras["asset-root"],
        installed.root.to_str().unwrap()
    );
    assert!(!applied.extras.contains_key("camera-preset"));
    let profile = applied.extras.get("render-profile").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(profile).unwrap();
    assert_eq!(parsed["version"], 2);
    assert_eq!(
        manager
            .call_apply("sketchfab", &entry)
            .await
            .unwrap()
            .extras,
        applied.extras
    );
    let write = |name: &str, bytes: &[u8]| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(output.join(name))
            .unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
    };
    write("render-profile.json", profile.as_bytes());
    write(
        "snapshot.json",
        &serde_json::to_vec_pretty(metadata).unwrap(),
    );
    write(
        "extras.json",
        &serde_json::to_vec_pretty(&applied.extras).unwrap(),
    );
    write("entry.json", &serde_json::to_vec_pretty(&entry).unwrap());
    assert!(crate::remote_asset::load(&source_root, "sketchfab", &id)
        .unwrap()
        .is_some());
    println!("presentation output: {}", output.display());
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
