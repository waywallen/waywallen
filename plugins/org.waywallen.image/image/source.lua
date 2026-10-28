local system = import("image.system")
local M = {}

local IMAGE_EXTS = {
    png = true, jpg = true, jpeg = true, webp = true,
    bmp = true, tiff = true, tif = true, avif = true, gif = true,
}

local function strip_ext(name)
    return name:match("(.+)%.[^.]+$") or name
end

function M.auto_detect(ctx)
    local candidates = {}
    local xdg = ctx.env("XDG_PICTURES_DIR")
    local home = ctx.env("HOME")
    if xdg and xdg ~= "" then
        candidates[#candidates + 1] = xdg
    elseif home and home ~= "" then
        candidates[#candidates + 1] = home .. "/Pictures/Wallpapers"
    end

    for _, path in ipairs(system.auto_detect(ctx)) do candidates[#candidates + 1] = path end
    local found, seen = {}, {}
    for _, path in ipairs(candidates) do
        local key = ctx.fs.canonicalize(path)
        if key and not seen[key] and ctx.fs.readable_dir(path) then
            seen[key] = true
            found[#found + 1] = path
        end
    end
    return found
end

function M.remove(ctx, item)
    local path = item.path or item.resource
    if ctx.fs.can_remove(path) == false then error("Image cannot be removed: permission denied") end
    ctx.remove_file(path)
end

function M.scan(ctx)
    local candidates, skipped, metadata = {}, {}, {}
    for _, root in ipairs(ctx.libraries()) do
        local kind = system.kind(ctx, root)
        local function collect(path, name)
            local ext = (ctx.extension(path) or ""):lower()
            if not IMAGE_EXTS[ext] then
                if kind and (ext == "jxl" or ext == "svg") then skipped[ext] = (skipped[ext] or 0) + 1 end
                return
            end
            local canonical = ctx.fs.canonicalize(path)
            if not canonical or not ctx.fs.is_file(path) then return end
            candidates[#candidates + 1] = {
                resource = path, library_root = root,
                canonical = canonical, system = kind ~= nil,
                symlink = ctx.fs.is_symlink(path),
                name = name or strip_ext(ctx.filename(path) or path),
                order = #candidates + 1,
            }
        end
        if kind then
            system.collect(ctx, root, kind, collect, metadata)
        else
            local function walk(dir, depth)
                for _, path in ipairs(ctx.fs.list_files(dir)) do collect(path) end
                if depth > 0 then
                    local dirs = ctx.list_dirs(dir)
                    table.sort(dirs)
                    for _, child in ipairs(dirs) do walk(child, depth - 1) end
                end
            end
            walk(root, 2)
        end
    end
    table.sort(candidates, function(a, b)
        if a.system ~= b.system then return not a.system end
        if not a.system then return a.order < b.order end
        if a.symlink ~= b.symlink then return not a.symlink end
        if a.library_root ~= b.library_root then return a.library_root < b.library_root end
        return a.resource < b.resource
    end)
    local entries, seen_path, seen_canonical = {}, {}, {}
    for _, candidate in ipairs(candidates) do
        if not seen_path[candidate.resource] and not (candidate.system and seen_canonical[candidate.canonical]) then
            seen_path[candidate.resource] = true
            seen_canonical[candidate.canonical] = true
            local media = candidate.system and ctx.probe(candidate.canonical) or nil
            local name = candidate.name
            if media and media.width and media.height then
                name = name .. " · " .. media.width .. "×" .. media.height
            end
            entries[#entries + 1] = {
                name = name, wp_type = "image",
                resource = candidate.resource, library_root = candidate.library_root,
                size = media and media.size or ctx.file_size(candidate.resource),
                width = media and media.width, height = media and media.height,
                can_remove = ctx.fs.can_remove(candidate.resource),
            }
        end
    end
    for _, ext in ipairs({ "jxl", "svg" }) do
        if skipped[ext] then ctx.log("image: skipped unsupported " .. ext:upper() .. " resources") end
    end
    ctx.log("image: found " .. #entries .. " image wallpapers")
    return entries
end

return M
