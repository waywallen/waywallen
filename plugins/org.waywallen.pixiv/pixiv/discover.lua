local api = import("pixiv.api")
local map = import("pixiv.map")
local session = import("pixiv.session")
local M = {}

M.badges = map.badge_definitions

M.filters = {
    {
        id = "content_rating", title = tr("Content rating"), type = "multi_select",
        options = {
            { value = "everyone", label = tr("Everyone") },
            { value = "sensitive", label = tr("Potentially sensitive") },
            { value = "r18", label = "R-18" },
            { value = "r18g", label = "R-18G" },
        },
    },
    {
        id = "ai_generated", title = tr("Show AI-generated works"), type = "toggle",
        options = {{ value = "ai_generated", label = tr("Show AI-generated works") }},
    },
}

M.sorts = {
    { key = "recommended", label = tr("Recommended") },
    {
        key = "ranking", label = tr("Ranking"),
        options = {
            { key = "day", label = tr("Daily") },
            { key = "day_male", label = tr("Popular among men") },
            { key = "day_female", label = tr("Popular among women") },
            { key = "week_original", label = tr("Original") },
            { key = "week_rookie", label = tr("Rookie") },
            { key = "week", label = tr("Weekly") },
            { key = "month", label = tr("Monthly") },
            { key = "day_ai", label = tr("AI daily") },
            { key = "day_r18_ai", label = tr("R-18 AI daily") },
            { key = "day_r18", label = tr("R-18 daily") },
            { key = "week_r18", label = tr("R-18 weekly") },
            { key = "week_r18g", label = tr("R-18G weekly") },
        },
    },
    {
        key = "bookmarks", label = tr("Bookmarks"),
        options = {
            { key = "bookmarks_public", label = tr("Public") },
            { key = "bookmarks_private", label = tr("Private") },
        },
    },
}

local ranking_modes = {}
for _, option in ipairs(M.sorts[2].options) do ranking_modes[option.key] = true end
local bookmark_restrictions = { bookmarks_public = "public", bookmarks_private = "private" }

local function selected_filters(params)
    local known = { everyone = true, sensitive = true, r18 = true, r18g = true, ai_generated = true }
    local selected = {}
    for _, value in ipairs(params.tags or {}) do
        if not known[value] then error("Unsupported Pixiv filter") end
        selected[value] = true
    end
    if not (selected.everyone or selected.sensitive or selected.r18 or selected.r18g) then
        selected.everyone = true
    end
    return selected
end

local function listing(params, selected)
    if type(params.artist_id) == "string" and params.artist_id ~= "" then
        return "/v1/user/illusts", { user_id = api.id(params.artist_id), type = "illust", filter = "for_android" }
    end
    local word = type(params.query) == "string" and params.query:match("^%s*(.-)%s*$") or ""
    if word ~= "" then
        if #word > 1024 then error("Pixiv search text is too long") end
        return "/v1/search/illust", {
            word = word,
            search_target = "partial_match_for_tags",
            sort = "date_desc",
            filter = "for_android",
            search_ai_type = selected.ai_generated and 0 or 1,
        }
    end
    local restriction = bookmark_restrictions[params.sort]
    if restriction then
        return "/v1/user/bookmarks/illust", { user_id = session.account_id(), restrict = restriction }
    end
    if ranking_modes[params.sort] then
        return "/v1/illust/ranking", { mode = params.sort, filter = "for_android" }
    end
    if params.sort ~= nil and params.sort ~= "" and params.sort ~= "recommended" then error("Unsupported Pixiv sort") end
    return "/v1/illust/recommended", { content_type = "illust", filter = "for_android", include_ranking_label = "true" }
end

function M.search(ctx, params)
    local selected = selected_filters(params)
    local path, query = listing(params, selected)
    local current = params.cursor
    local payload = session.get(ctx, path, query, current)
    if ctx.json.strict.kind(payload.illusts) ~= "array" or #payload.illusts > 200 then
        error("Pixiv returned an invalid illustration list")
    end
    local items = {}
    for _, raw in ipairs(payload.illusts) do
        local item = map.normalize(ctx, raw)
        if item and item.preview_url ~= ""
            and selected[item.content_category]
            and (not item.ai_generated or selected.ai_generated) then
            table.insert(items, map.search_item(item))
        end
    end
    local next_cursor = api.cursor(ctx, payload.next_url, path, current)
    return { items = items, has_more = next_cursor ~= nil, next_cursor = next_cursor }
end

local function detail(ctx, id)
    id = api.id(id)
    local payload = session.get(ctx, "/v1/illust/detail", { illust_id = id, filter = "for_android" })
    local item = map.normalize(ctx, payload.illust)
    if not item then error("Animated Pixiv illustrations are not supported") end
    if item.id ~= id then error("Pixiv returned a different illustration") end
    return item
end

function M.details(ctx, id)
    return map.details(detail(ctx, id))
end

function M.download(ctx, id)
    return map.download(detail(ctx, id))
end

return M
