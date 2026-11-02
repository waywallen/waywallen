local M = {}
local API = "https://api.sketchfab.com/v3"
local cooldown_until = 0

M.filters = {
    {
        id = "category",
        title = tr("Category"),
        type = "select",
        options = {
            { value = "category:animals-pets", label = tr("Animals & Pets") },
            { value = "category:architecture", label = tr("Architecture") },
            { value = "category:art-abstract", label = tr("Art & Abstract") },
            { value = "category:cars-vehicles", label = tr("Cars & Vehicles") },
            { value = "category:characters-creatures", label = tr("Characters & Creatures") },
            { value = "category:cultural-heritage-history", label = tr("Cultural Heritage & History") },
            { value = "category:electronics-gadgets", label = tr("Electronics & Gadgets") },
            { value = "category:fashion-style", label = tr("Fashion & Style") },
            { value = "category:food-drink", label = tr("Food & Drink") },
            { value = "category:furniture-home", label = tr("Furniture & Home") },
            { value = "category:music", label = tr("Music") },
            { value = "category:nature-plants", label = tr("Nature & Plants") },
            { value = "category:news-politics", label = tr("News & Politics") },
            { value = "category:people", label = tr("People") },
            { value = "category:places-travel", label = tr("Places & Travel") },
            { value = "category:science-technology", label = tr("Science & Technology") },
            { value = "category:sports-fitness", label = tr("Sports & Fitness") },
            { value = "category:weapons-military", label = tr("Weapons & Military") },
        },
    },
    {
        id = "license",
        title = tr("License"),
        type = "select",
        options = {
            { value = "license:by", label = tr("CC Attribution") },
            { value = "license:by-sa", label = tr("CC Attribution-ShareAlike") },
            { value = "license:by-nd", label = tr("CC Attribution-NoDerivs") },
            { value = "license:by-nc", label = tr("CC Attribution-NonCommercial") },
            { value = "license:by-nc-sa", label = tr("CC Attribution-NonCommercial-ShareAlike") },
            { value = "license:by-nc-nd", label = tr("CC Attribution-NonCommercial-NoDerivs") },
            { value = "license:cc0", label = tr("CC0 Public Domain") },
        },
    },
    {
        id = "downloadable",
        title = tr("Downloadable only"),
        type = "toggle",
        options = { { value = "feature:downloadable", label = tr("Downloadable only") } },
    },
    {
        id = "animated",
        title = tr("Animated only"),
        type = "toggle",
        options = { { value = "feature:animated", label = tr("Animated only") } },
    },
    {
        id = "staffpicked",
        title = tr("Staff picks only"),
        type = "toggle",
        options = { { value = "feature:staffpicked", label = tr("Staff picks only") } },
    },
}

local selections = {}
for _, filter in ipairs(M.filters) do
    for _, option in ipairs(filter.options) do
        selections[option.value] = filter.id
    end
end

local sorts = {
    recent = "-publishedAt",
    likes = "-likeCount",
    views = "-viewCount",
}

local function cursor(value, null)
    if value == nil or value == null then
        return nil
    end
    if type(value) ~= "string" or #value == 0 or #value > 4096 or value:find("[%c]") then
        error("Sketchfab returned an invalid cursor")
    end
    return value
end

local function request_json(ctx, path, query, headers, allow_unauthorized)
    local now = ctx.time.unix()
    if now < cooldown_until then
        error("Sketchfab rate limit; retry in " .. tostring(math.ceil(cooldown_until - now)) .. " seconds")
    end
    local response = ctx.http:get(API .. path)
        :headers(headers or {})
        :query(query)
        :timeout(20)
        :response_limit(4 * 1024 * 1024)
        :send()
    local status = response:status()
    if status == 429 then
        local retry = response:header_values("retry-after")
        local delay = type(retry[1]) == "string" and retry[1]:match("^%s*(%d+)%s*$")
        delay = tonumber(delay)
        local now = ctx.time.unix()
        local deadline = delay and (now + delay * 1.0)
        if not deadline or deadline == math.huge then
            delay, deadline = 30, now + 30
        end
        cooldown_until = deadline
        response:close()
        error("Sketchfab rate limit; retry in " .. tostring(delay) .. " seconds")
    end
    if not response:ok() then
        response:close()
        if status == 401 and allow_unauthorized then
            return nil, 401
        end
        if status == 404 then
            error("Sketchfab model was not found")
        end
        if status == 401 or status == 403 then
            error("Sketchfab access is restricted")
        end
        error("Sketchfab HTTP error " .. tostring(status))
    end
    local ok, text = pcall(function() return response:text({ strict_utf8 = true }) end)
    response:close()
    if not ok then
        error("Sketchfab response could not be read")
    end
    local decoded, payload = pcall(ctx.json.strict.decode, text)
    if not decoded or ctx.json.strict.kind(payload) ~= "object" then
        error("Sketchfab returned invalid JSON")
    end
    return payload
end

function M.next_cursor(ctx, payload, current)
    local json = ctx.json.strict
    local next_value
    if payload.cursors ~= nil and payload.cursors ~= json.null then
        if json.kind(payload.cursors) ~= "object" then
            error("Sketchfab returned invalid pagination")
        end
        next_value = cursor(payload.cursors.next, json.null)
    end
    if payload.next ~= nil and payload.next ~= json.null then
        if type(payload.next) ~= "string" then
            error("Sketchfab returned an invalid next page")
        end
        local parsed = ctx.url.parse(payload.next)
        if not parsed or parsed.scheme ~= "https" or parsed.host ~= "api.sketchfab.com"
            or parsed.port ~= 443 or parsed.path ~= "/v3/search" or parsed.fragment ~= nil then
            error("Sketchfab returned an invalid next page")
        end
        local query = ctx.url.decode(parsed.query or "")
        if query.type ~= "models" then
            error("Sketchfab returned an invalid next page")
        end
        local from_url = cursor(query.cursor)
        if not from_url or (next_value and next_value ~= from_url) then
            error("Sketchfab returned inconsistent pagination")
        end
        next_value = from_url
    end
    if next_value ~= nil and next_value == current then
        error("Sketchfab repeated the current cursor")
    end
    return next_value
end

function M.search(ctx, params, headers)
    local next_value = cursor(params.cursor)
    if (params.page or 1) > 1 and not next_value then
        error("Sketchfab requires a cursor for the next page")
    end
    local query = { type = "models", count = "24", q = params.query or "", cursor = next_value }
    if params.sort and params.sort ~= "" and params.sort ~= "relevance" then
        query.sort_by = sorts[params.sort]
        if not query.sort_by then
            error("Unsupported Sketchfab sort")
        end
    end
    local selected = {}
    for _, value in ipairs(params.tags or {}) do
        local filter = selections[value]
        if not filter or selected[filter] then
            error("Invalid Sketchfab filter selection")
        end
        selected[filter] = true
        if filter == "category" then
            query.categories = value:sub(#"category:" + 1)
        elseif filter == "license" then
            query.license = value:sub(#"license:" + 1)
        else
            query[filter] = "true"
        end
    end
    return request_json(ctx, "/search", query, headers)
end

function M.model(ctx, id, headers)
    if type(id) ~= "string" or #id ~= 32 or not id:match("^[a-fA-F0-9]+$") then
        error("Invalid Sketchfab model id")
    end
    return request_json(ctx, "/models/" .. id, {}, headers)
end

function M.me(ctx, token)
    if type(token) ~= "string" or token == "" then
        error("Sketchfab API Token is required")
    end
    return request_json(ctx, "/me", {}, { Authorization = "Token " .. token }, true)
end

return M
