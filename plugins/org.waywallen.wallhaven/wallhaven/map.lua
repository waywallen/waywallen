local M = {}

local function file_ext(path)
    return string.lower(path:match("%.([^./?#]+)$") or "jpg")
end

local function tags_from_detail(detail)
    local out = {}
    for _, tag in ipairs(detail.tags or {}) do
        if tag.name and tag.name ~= "" then
            table.insert(out, tag.name)
        end
    end
    return out
end

local function rating(purity)
    if purity == "nsfw" then
        return "Mature"
    end
    if purity == "sketchy" then
        return "Questionable"
    end
    return "Everyone"
end

-- Only /w/<id> carries an uploader; the search listing has no such field, so a
-- name can be had for a wallpaper that has been opened and for no other.
local function uploader_name(detail)
    local uploader = detail.uploader
    if type(uploader) ~= "table" then
        return ""
    end
    return uploader.username or ""
end

-- The uploader's avatar comes in 200/128/32/20 px; 128 stays sharp at the
-- 32 px the detail panel draws it on a scaled display.
local function uploader_avatar(detail)
    local uploader = detail.uploader
    if type(uploader) ~= "table" or type(uploader.avatar) ~= "table" then
        return ""
    end
    local avatar = uploader.avatar
    return avatar["128px"] or avatar["200px"] or avatar["32px"] or avatar["20px"] or ""
end

local function title(item)
    if item.id and item.id ~= "" then
        return "Wallhaven " .. item.id
    end
    return item.url or item.path or "Wallhaven"
end

function M.search_item(item)
    local thumbs = item.thumbs or {}
    return {
        id = item.id or "",
        title = title(item),
        preview_url = thumbs.original or thumbs.large or thumbs.small or item.path or "",
        author = "",
        wp_type = "image",
        width = item.dimension_x,
        height = item.dimension_y,
        extra = {
            resolution = item.resolution or "",
            purity = item.purity or "",
        },
    }
end

function M.details(detail)
    local description = detail.source or detail.url or ""
    return {
        author = uploader_name(detail),
        author_avatar_url = uploader_avatar(detail),
        description = description,
        size = tostring(detail.file_size or ""),
        width = detail.dimension_x,
        height = detail.dimension_y,
        tags = tags_from_detail(detail),
        web_url = detail.url or "",
        extra = {
            url = detail.url or "",
            path = detail.path or "",
            purity = detail.purity or "",
            resolution = detail.resolution or "",
        },
    }
end

function M.download(detail)
    local ext = file_ext(detail.path or "")
    local thumbs = detail.thumbs or {}
    return {
        wp_type = "image",
        url = detail.path or "",
        filename = "wallhaven-" .. tostring(detail.id or "wallpaper") .. "." .. ext,
        title = title(detail),
        preview_url = thumbs.original or thumbs.large or thumbs.small or "",
        description = detail.source or detail.url or "",
        tags = tags_from_detail(detail),
        external_id = tostring(detail.id or ""),
        web_url = detail.url or "",
        size = detail.file_size,
        width = detail.dimension_x,
        height = detail.dimension_y,
        content_rating = rating(detail.purity),
    }
end

return M
