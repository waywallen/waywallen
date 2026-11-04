local api = import("sketchfab.api")
local map = import("sketchfab.map")
local session = import("sketchfab.session")
local presentation = import("sketchfab.presentation")
local M = {}
local models = {}
local sequence = 0
local revision = session.revision()

local function sync_session()
    local current = session.revision()
    if revision ~= current then
        models, sequence, revision = {}, 0, current
    end
end

local function remember(ctx, incoming)
    local now = ctx.time.unix()
    local total, oldest, oldest_sequence = 0
    for id, entry in pairs(models) do
        if now >= entry.expires then
            models[id] = nil
        else
            total = total + 1
            if not oldest_sequence or entry.sequence < oldest_sequence then
                oldest, oldest_sequence = id, entry.sequence
            end
        end
    end
    local previous = models[incoming.id]
    local model = map.merge(previous and previous.model, incoming)
    if not previous and total >= 128 then
        models[oldest] = nil
    end
    sequence = sequence + 1
    models[incoming.id] = { model = model, expires = now + 300, sequence = sequence }
    return model
end

function M.search(ctx, params)
    sync_session()
    local payload = api.search(ctx, params, session.headers())
    local results = map.list(payload.results, 24, ctx.json.strict)
    local next_cursor = api.next_cursor(ctx, payload, params.cursor)
    local items = {}
    for _, item in ipairs(results) do
        table.insert(items, map.search_item(remember(ctx, map.normalize(item, ctx.json.strict))))
    end
    return { items = items, has_more = next_cursor ~= nil, next_cursor = next_cursor }
end

function M.details(ctx, id)
    sync_session()
    local model = map.normalize(api.model(ctx, id, session.headers()), ctx.json.strict)
    if model.id ~= id then
        error("Sketchfab returned a different model id")
    end
    return map.details(remember(ctx, model))
end

function M.download(ctx, id)
    if not session.authenticated() then
        error("Log in to Sketchfab to download models")
    end
    sync_session()
    local headers = session.headers()
    local model = map.normalize(api.model(ctx, id, headers), ctx.json.strict)
    if model.id ~= id then
        error("Sketchfab returned a different model id")
    end
    model = remember(ctx, model)
    if model.downloadable == false then
        error("This Sketchfab model is not downloadable")
    end
    local metadata = map.download(model)
    metadata.provider_data, metadata.resources, metadata.prepare = presentation.capture(ctx, id)
    local transfer = api.download(ctx, id, headers)
    metadata.url, metadata.filename, metadata.archive = transfer.url, transfer.filename, transfer.archive
    metadata.size = transfer.size
    return metadata
end

function M.resolve(ctx, params)
    local root, metadata = params.dir, params.metadata
    if type(root) ~= "string" or root == "" or type(metadata) ~= "table"
        or metadata.wp_type ~= "gltf" or metadata.external_id ~= params.id then
        error("Sketchfab download metadata is missing or inconsistent")
    end
    root = root:gsub("/+$", "")
    local entry = root .. "/scene.gltf"
    if not ctx.fs.is_file(entry) and ctx.fs.is_file(root .. "/scene.glb") then
        entry = root .. "/scene.glb"
    end
    if not ctx.fs.is_file(entry) or ctx.fs.is_symlink(entry) then
        local glb, gltf, visited = {}, {}, 0
        local function walk(directory, depth)
            visited = visited + 1
            if depth > 16 or visited > 4096 then
                error("Sketchfab archive directory structure exceeds the supported limits")
            end
            for _, path in ipairs(ctx.fs.list_files(directory)) do
                local extension = ctx.extension(path)
                extension = extension and extension:lower()
                if extension == "gltf" or extension == "glb" then
                    if ctx.fs.is_symlink(path) then error("Sketchfab archive contains a linked model entry") end
                    table.insert(extension == "glb" and glb or gltf, path)
                end
            end
            for _, directory in ipairs(ctx.list_dirs(directory)) do
                if ctx.fs.is_symlink(directory) then error("Sketchfab archive contains a linked directory") end
                walk(directory, depth + 1)
            end
        end
        walk(root, 0)
        local candidates = #gltf > 0 and gltf or glb
        if #candidates ~= 1 then error("Sketchfab download must contain a unique glTF or GLB model entry") end
        entry = candidates[1]
    end
    local prefix = root .. "/"
    if entry:sub(1, #prefix) ~= prefix then
        error("Sketchfab model entry is outside the downloaded archive")
    end
    return {
        name = metadata.title,
        wp_type = "gltf",
        resource = entry:sub(#prefix + 1),
        description = metadata.description or "",
        tags = metadata.tags or {},
        external_id = metadata.external_id,
        size = metadata.size,
        content_rating = metadata.content_rating,
        provider_data = presentation.finalize(ctx, metadata, params.prepared),
    }
end

return M
