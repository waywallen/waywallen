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

local function push_split(list, seen, s)
    if not s or s == "" then return end
    for d in string.gmatch(s, "[^:]+") do
        if d ~= "" and not seen[d] then
            seen[d] = true
            table.insert(list, d)
        end
    end
end

local function gather_dirs(ctx)
    -- Priority: explicit daemon config, then env, then XDG, then sensible
    -- defaults under $HOME. Each source may be colon-separated (PATH-style)
    -- so users can enumerate multiple wallpaper folders in one setting.
    local dirs, seen = {}, {}
    push_split(dirs, seen, ctx.config("image_dir"))
    push_split(dirs, seen, ctx.env("WAYWALLEN_IMAGE_DIR"))
    push_split(dirs, seen, ctx.env("XDG_PICTURES_DIR"))
    local home = ctx.env("HOME")
    if home and home ~= "" then
        push_split(dirs, seen, home .. "/Pictures/Wallpapers")
        push_split(dirs, seen, home .. "/Pictures")
    end

    local kept = {}
    for _, d in ipairs(dirs) do
        if ctx.file_exists(d) then table.insert(kept, d) end
    end
    return kept
end

function M.scan(ctx)
    local entries = {}
    local dirs = gather_dirs(ctx)
    if #dirs == 0 then
        ctx.log("image: no image directories found")
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
