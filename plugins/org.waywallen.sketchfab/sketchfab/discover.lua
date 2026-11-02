local api = import("sketchfab.api")
local map = import("sketchfab.map")
local session = import("sketchfab.session")
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

return M
