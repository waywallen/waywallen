local api = import("pixiv.api")
local M = {}

local function text(value, maximum)
    if type(value) ~= "string" or #value > maximum then return "" end
    return value
end

local function image(ctx, value)
    if type(value) ~= "string" or #value > 4096 or value:find("[%c]") then return "" end
    local parsed = ctx.url.parse(value)
    if not parsed or parsed.scheme ~= "https" or parsed.port ~= 443 or type(parsed.host) ~= "string"
        or (parsed.host ~= "pximg.net" and parsed.host:sub(-10) ~= ".pximg.net") then return "" end
    return value, parsed
end

local function dimension(value)
    if type(value) == "number" and value > 0 and value <= 100000 and value == math.floor(value) then return value end
end

function M.image_headers()
    return { Referer = "https://www.pixiv.net/" }
end

M.badge_definitions = {
    { id = "ai", label = tr("AI-generated") },
    { id = "pages", label = tr("Image count") },
}

local function badges(item)
    local result = {}
    if item.ai_generated then table.insert(result, { id = "ai", text = "AI" }) end
    if item.page_count > 1 then table.insert(result, { id = "pages", text = tostring(item.page_count) }) end
    return result
end

local function download_tags(item)
    local result = {}
    local has_ai = false
    for _, tag in ipairs(item.tags) do
        table.insert(result, tag)
        if tag:lower() == "ai-generated" then has_ai = true end
    end
    if item.ai_generated and not has_ai then table.insert(result, "AI-generated") end
    return result
end

function M.normalize(ctx, item)
    if type(item) ~= "table" then error("Pixiv returned an invalid illustration") end
    if item.type == "ugoira" then return nil end
    if item.type ~= "illust" and item.type ~= "manga" then return nil end
    local id = api.id(item.id)
    local user = type(item.user) == "table" and item.user or {}
    local profile = type(user.profile_image_urls) == "table" and user.profile_image_urls or {}
    local urls = type(item.image_urls) == "table" and item.image_urls or {}
    local original = type(item.meta_single_page) == "table" and item.meta_single_page.original_image_url
    if type(item.meta_pages) == "table" and type(item.meta_pages[1]) == "table"
        and type(item.meta_pages[1].image_urls) == "table" then
        original = item.meta_pages[1].image_urls.original
    end
    local tags = {}
    for _, tag in ipairs(type(item.tags) == "table" and item.tags or {}) do
        local name = type(tag) == "table" and text(tag.name, 256) or ""
        if name ~= "" and #tags < 256 then table.insert(tags, name) end
    end
    local original_url, original_uri = image(ctx, original)
    local restriction = tonumber(item.x_restrict) or 0
    if restriction ~= 0 and restriction ~= 1 and restriction ~= 2 then error("Pixiv returned an invalid content restriction") end
    local category = restriction == 1 and "r18" or restriction == 2 and "r18g"
        or (tonumber(item.sanity_level) or 0) > 2 and "sensitive" or "everyone"
    return {
        id = id,
        title = text(item.title, 1024),
        preview_url = image(ctx, urls.large) ~= "" and image(ctx, urls.large) or image(ctx, urls.medium),
        original_url = original_url,
        original_extension = original_uri and original_uri.path:match("%.([a-zA-Z0-9]+)$"),
        author = text(user.name, 256),
        author_id = api.optional_id(user.id) or "",
        author_avatar_url = image(ctx, profile.medium),
        description = text(item.caption, 16384),
        width = dimension(item.width),
        height = dimension(item.height),
        page_count = dimension(item.page_count) or 1,
        tags = tags,
        content_category = category,
        ai_generated = tonumber(item.illust_ai_type) == 2,
        content_rating = restriction > 0 and "Mature" or category == "sensitive" and "Questionable" or "Everyone",
    }
end

local function title(item)
    return item.title ~= "" and item.title or "Pixiv " .. item.id
end

function M.search_item(item)
    return {
        id = item.id,
        title = title(item),
        preview_url = item.preview_url,
        preview_headers = M.image_headers(),
        author = item.author,
        author_avatar_url = item.author_avatar_url,
        author_avatar_headers = item.author_avatar_url ~= "" and M.image_headers() or {},
        wp_type = "image",
        width = item.width,
        height = item.height,
        badges = badges(item),
        extra = { pages = tostring(item.page_count) },
    }
end

function M.details(item)
    local fields = {}
    if item.page_count > 1 then
        fields[1] = {
            key = "pages",
            label = tr("Pages"),
            value = tostring(item.page_count),
        }
        fields[2] = {
            key = "download",
            label = tr("Download page"),
            value = "1 / " .. tostring(item.page_count),
        }
    end
    return {
        author = item.author,
        author_id = item.author_id,
        author_avatar_url = item.author_avatar_url,
        author_avatar_headers = item.author_avatar_url ~= "" and M.image_headers() or {},
        author_url = item.author_id ~= "" and "https://www.pixiv.net/users/" .. item.author_id or "",
        description = item.description,
        size = "",
        preview_url = item.preview_url,
        preview_headers = M.image_headers(),
        width = item.width,
        height = item.height,
        tags = item.tags,
        web_url = "https://www.pixiv.net/artworks/" .. item.id,
        fields = fields,
        extra = { pages = tostring(item.page_count) },
    }
end

function M.download(item)
    if item.original_url == "" then error("This Pixiv illustration has no accessible original image") end
    local extension = item.original_extension
    extension = extension and extension:lower()
    if extension ~= "jpg" and extension ~= "jpeg" and extension ~= "png" and extension ~= "gif"
        and extension ~= "webp" and extension ~= "avif" then error("Pixiv returned an unsupported image format") end
    return {
        wp_type = "image",
        url = item.original_url,
        headers = M.image_headers(),
        filename = "pixiv-" .. item.id .. "-p0." .. extension,
        title = title(item),
        preview_url = item.preview_url,
        preview_headers = M.image_headers(),
        description = item.description,
        tags = download_tags(item),
        external_id = item.id,
        web_url = "https://www.pixiv.net/artworks/" .. item.id,
        width = item.width,
        height = item.height,
        content_rating = item.content_rating,
        attribution = {
            author = item.author,
            author_url = item.author_id ~= "" and "https://www.pixiv.net/users/" .. item.author_id or "",
        },
    }
end

return M
