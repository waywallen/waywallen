local system = import("image.system")
local M = {}

local function strip_ext(name)
    return name:match("(.+)%.[^.]+$") or name
end

function M.auto_detect(ctx)
    local candidates = system.auto_detect(ctx)
    local home, pictures = ctx.env("HOME"), ctx.env("XDG_PICTURES_DIR")
    local path = pictures and pictures ~= "" and pictures or (home and home .. "/Pictures/Wallpapers")
    if path then candidates[#candidates+1] = {path=path, settings={scan_mode="files"}} end
    local found, seen = {}, {}
    for _, candidate in ipairs(candidates) do
        local key = ctx.fs.canonicalize(candidate.path)
        if key and not seen[key] and ctx.fs.readable_dir(candidate.path) then
            seen[key] = true
            found[#found+1] = candidate
        end
    end
    return found
end

function M.remove(ctx, item)
    if item.sub_type == "timeline" then error("Desktop wallpaper definitions cannot be removed") end
    local path = item.path or item.resource
    if ctx.fs.can_remove(path) == false then error("Image cannot be removed: permission denied") end
    ctx.remove_file(path)
end

function M.scan(ctx)
    local entries, seen = {}, {}
    for _, root in ipairs(ctx.libraries()) do
        local settings = ctx.library_settings(root)
        local timeline = settings.scan_mode == "wallpapers"
        local function collect(path, name)
            if not timeline and not system.supported(ctx, path) then return end
            local canonical = ctx.fs.canonicalize(path)
            if not canonical or seen[canonical] or not ctx.fs.is_file(path) then return end
            seen[canonical] = true
            local size
            if not timeline then size = ctx.file_size(path) end
            entries[#entries+1] = {
                name = name or strip_ext(ctx.filename(path) or path),
                wp_type = "image", sub_type = timeline and "timeline" or "",
                resource = path, library_root = root, preview = nil,
                size = size,
                can_remove = not timeline and ctx.fs.can_remove(path) or false,
            }
        end
        if timeline then
            system.collect(ctx, root, collect)
        else
            local visited = {}
            local function walk(directory, depth)
                local key = ctx.fs.canonicalize(directory)
                if not key or visited[key] then return end
                visited[key] = true
                for _, path in ipairs(ctx.fs.list_files(directory)) do collect(path) end
                if depth > 0 then
                    local directories = ctx.list_dirs(directory)
                    table.sort(directories)
                    for _, child in ipairs(directories) do walk(child, depth - 1) end
                end
            end
            walk(root, 2)
        end
    end
    ctx.log("image: found " .. #entries .. " image wallpapers")
    return entries
end

return M
