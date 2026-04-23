-- image — waywallen source plugin for plain still-image wallpapers.
--
-- Installed to <prefix>/share/waywallen/sources/image.lua. Emits entries
-- with wp_type = "image" that the daemon routes to waywallen-image-renderer.
-- Metadata `image` / `path` both carry the absolute file path; the daemon
-- forwards them as `--image <path>` and `--path <path>`.

local M = {}

function M.info()
    return {
        name = "image",
        types = {"image"},
        version = "0.1.0",
    }
end

-- Formats the renderer can decode today. GIF decodes its first frame in M1;
-- animated GIF/APNG/WebP pacing lands in M5 so we still list them — a
-- single-frame wallpaper is a reasonable fallback.
local IMAGE_EXTS = {
    png = true, jpg = true, jpeg = true, webp = true,
    bmp = true, tiff = true, tif = true, avif = true, gif = true,
}

local function strip_ext(name)
    return name:match("(.+)%.[^.]+$") or name
end

function M.auto_detect(ctx)
    -- Probe XDG/home defaults; return paths that actually exist so
    -- the daemon can register them as libraries.
    local candidates = {}
    local xdg = ctx.env("XDG_PICTURES_DIR")
    if xdg and xdg ~= "" then table.insert(candidates, xdg) end
    local home = ctx.env("HOME")
    if home and home ~= "" then table.insert(candidates, home .. "/Pictures/Wallpapers") end

    local found, seen = {}, {}
    for _, p in ipairs(candidates) do
        if not seen[p] and ctx.file_exists(p) then
            seen[p] = true
            table.insert(found, p)
        end
    end
    return found
end

function M.scan(ctx)
    local entries = {}
    -- Libraries are owned by the daemon DB and pushed in via
    -- ctx.libraries(); the plugin no longer reads config or env vars.
    -- An unconfigured image plugin sees an empty list and emits zero
    -- entries.
    local dirs = {}
    for _, d in ipairs(ctx.libraries()) do
        if ctx.file_exists(d) then table.insert(dirs, d) end
    end
    if #dirs == 0 then
        ctx.log("image: no image libraries configured")
        return entries
    end

    local seen_path = {}
    for _, dir in ipairs(dirs) do
        -- The daemon's glob is the Rust `glob` crate, which does not expand
        -- braces. Enumerate a few depth levels explicitly — enough for the
        -- common "~/Pictures/<album>/<file>" layouts without walking huge
        -- trees on every scan.
        local patterns = {
            dir .. "/*.*",
            dir .. "/*/*.*",
            dir .. "/*/*/*.*",
        }
        for _, pat in ipairs(patterns) do
            for _, path in ipairs(ctx.glob(pat)) do
                local ext = ctx.extension(path)
                if ext and IMAGE_EXTS[string.lower(ext)] and not seen_path[path] then
                    seen_path[path] = true
                    local filename = ctx.filename(path) or path
                    local name = strip_ext(filename)
                    table.insert(entries, {
                        -- Path-scoped id keeps files in different albums
                        -- with the same basename distinguishable.
                        id = "image:" .. path,
                        name = name,
                        wp_type = "image",
                        resource = path,
                        preview = path,
                        library_root = dir,
                        metadata = {
                            image = path,
                            path = path,
                        },
                    })
                end
            end
        end
    end

    ctx.log("image: found " .. #entries .. " image wallpapers in "
            .. #dirs .. " directories")
    return entries
end

return M
