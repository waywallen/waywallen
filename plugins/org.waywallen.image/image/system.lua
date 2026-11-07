local M = {}
local IMAGE_EXTS = {png=true, jpg=true, jpeg=true, webp=true, bmp=true, tiff=true, tif=true, avif=true, gif=true}

function M.supported(ctx, path)
    return IMAGE_EXTS[(ctx.extension(path) or ""):lower()] == true
end

function M.resolution(ctx, path)
    local name = (ctx.filename(path) or ""):match("^([^.]*)")
    local width, height = (name or ""):match("^(%d+)x(%d+)$")
    width, height = tonumber(width), tonumber(height)
    if width and height and width > 0 and height > 0 and width <= 2147483647 and height <= 2147483647 then
        return {width=width, height=height}
    end
end

function M.data_dirs(ctx)
    local value = ctx.env("XDG_DATA_DIRS")
    if not value or value == "" then value = "/usr/local/share:/usr/share" end
    local result, seen = {}, {}
    for directory in value:gmatch("[^:]+") do
        directory = directory:gsub("/+$", "")
        if directory:sub(1, 1) == "/" and not seen[directory] then
            seen[directory] = true
            result[#result+1] = directory
        end
    end
    return result
end

function M.auto_detect(ctx)
    local result = {}
    for _, directory in ipairs(M.data_dirs(ctx)) do
        for _, suffix in ipairs({"wallpapers", "gnome-background-properties"}) do
            local path = directory .. "/" .. suffix
            if ctx.fs.readable_dir(path) then
                result[#result+1] = {path=path, settings={scan_mode="wallpapers"}}
            end
        end
    end
    return result
end

local function locale(ctx)
    for _, key in ipairs({"LC_ALL", "LC_MESSAGES", "LANG"}) do
        local value = ctx.env(key)
        if value and value ~= "" then return (value:gsub("[.@].*$", ""):gsub("-", "_")) end
    end
    return ""
end

local function document(ctx, path, parse)
    local bytes = ctx.read_file(path)
    if not bytes then return nil end
    local ok, value = pcall(parse, bytes)
    return ok and value or nil
end

local function kde(ctx, root, emit)
    local path = root .. "/metadata.json"
    if not ctx.fs.is_file(path) or not ctx.fs.is_dir(root .. "/contents/images") then return false end
    local value = document(ctx, path, ctx.json.parse)
    local plugin = value and type(value.KPlugin) == "table" and value.KPlugin or {}
    local lang = locale(ctx)
    local name = plugin["Name[" .. lang .. "]"] or plugin["Name[" .. lang:match("^[^_]*") .. "]"] or plugin.Name
    local media, distance, area
    for _, image in ipairs(ctx.fs.list_files(root .. "/contents/images")) do
        local size = M.resolution(ctx, image)
        if size and M.supported(ctx, image) then
            local d, a = math.abs(size.width / size.height - 16/9), size.width * size.height
            if not media or d < distance or (d == distance and a < area) then
                media, distance, area = size, d, a
                media.size = ctx.file_size(image)
            end
        end
    end
    emit(path, type(name) == "string" and name or ctx.filename(root), media)
    return true
end

local function xml_name(ctx, path)
    local value = document(ctx, path, ctx.xml.parse)
    local lang, fallback, language = locale(ctx)
    if value and value.tag == "wallpapers" then
        for _, child in ipairs(value.children or {}) do
            if child.tag == "wallpaper" then
                for _, field in ipairs(child.children or {}) do
                    if field.tag == "name" and (field.text or "") ~= "" then
                        local code = (field.attrs["xml:lang"] or ""):gsub("-", "_")
                        if code == lang then return field.text end
                        if code == lang:match("^[^_]*") then language = field.text end
                        if code == "" then fallback = field.text end
                    end
                end
                if fallback or language then return language or fallback end
            end
        end
    end
end

function M.collect(ctx, root, emit)
    local seen = {}
    local function walk(directory, depth)
        local key = ctx.fs.canonicalize(directory)
        if not key or seen[key] or not ctx.fs.readable_dir(directory) then return end
        seen[key] = true
        if kde(ctx, directory, emit) then return end
        for _, path in ipairs(ctx.fs.list_files(directory)) do
            if (ctx.extension(path) or ""):lower() == "xml" then emit(path, xml_name(ctx, path)) end
        end
        if depth > 0 then
            local directories = ctx.list_dirs(directory)
            table.sort(directories)
            for _, child in ipairs(directories) do walk(child, depth - 1) end
        end
    end
    walk(root, 6)
end

return M
