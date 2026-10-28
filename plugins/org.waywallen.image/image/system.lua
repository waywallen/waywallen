local M = {}

local function trim(value)
    return (value or ""):match("^%s*(.-)%s*$")
end

local function clean(path)
    return path == "/" and path or path:gsub("/+$", "")
end

local function within(path, root)
    return path == root or path:sub(1, #root + 1) == root .. "/"
end

local function sorted(values)
    table.sort(values)
    return values
end

local function locale(ctx)
    for _, key in ipairs({ "LC_ALL", "LC_MESSAGES", "LANG" }) do
        local value = ctx.env(key)
        if value and value ~= "" then return value:gsub("[.@].*$", ""):gsub("-", "_") end
    end
    return ""
end

function M.data_dirs(ctx)
    local value = ctx.env("XDG_DATA_DIRS")
    if not value or value == "" then value = "/usr/local/share:/usr/share" end
    local result, seen = {}, {}
    for dir in value:gmatch("[^:]+") do
        dir = clean(dir)
        if dir:sub(1, 1) == "/" and not seen[dir] then
            result[#result + 1] = dir
            seen[dir] = true
        end
    end
    return result
end

function M.auto_detect(ctx)
    local result = {}
    for _, dir in ipairs(M.data_dirs(ctx)) do
        for _, name in ipairs({ "backgrounds", "wallpapers" }) do
            local path = dir .. "/" .. name
            if ctx.fs.readable_dir(path) then result[#result + 1] = path end
        end
    end
    return result
end

function M.kind(ctx, root)
    local path = clean(ctx.fs.canonicalize(root) or root)
    local data_dirs = M.data_dirs(ctx)
    data_dirs[#data_dirs + 1] = "/usr/share"
    data_dirs[#data_dirs + 1] = "/usr/local/share"
    for _, dir in ipairs(data_dirs) do
        for _, name in ipairs({ "wallpapers", "backgrounds" }) do
            local base = dir .. "/" .. name
            base = clean(ctx.fs.canonicalize(base) or base)
            if within(path, base) then return name end
        end
    end
    if ctx.fs.is_dir(root .. "/contents/images") or ctx.fs.is_dir(root .. "/contents/images_dark") then
        return "wallpapers"
    end
    if ctx.filename(path) == "wallpapers" then
        for _, dir in ipairs(ctx.list_dirs(root)) do
            if ctx.fs.is_dir(dir .. "/contents/images") then return "wallpapers" end
        end
    end
    if ctx.filename(path) == "backgrounds"
        and ctx.fs.is_dir(ctx.fs.parent(path) .. "/gnome-background-properties") then
        return "backgrounds"
    end
end

local function read(ctx, path, parse, cache)
    cache.documents = cache.documents or {}
    local key = ctx.fs.canonicalize(path) or path
    if cache.documents[key] ~= nil then return cache.documents[key] or nil end
    local content = ctx.read_file(path)
    if not content then cache.documents[key] = false; return nil end
    local ok, result = pcall(parse, content)
    cache.documents[key] = ok and result or false
    if ok then return result end
    ctx.log("image: invalid wallpaper metadata " .. path .. ": " .. tostring(result))
end

local function walk(ctx, root, depth, emit)
    if not ctx.fs.readable_dir(root) then
        ctx.log("image: cannot read wallpaper directory " .. root)
        return
    end
    for _, path in ipairs(ctx.fs.list_files(root)) do emit(path) end
    if depth > 0 then
        for _, dir in ipairs(sorted(ctx.list_dirs(root))) do walk(ctx, dir, depth - 1, emit) end
    end
end

local function kde_package(ctx, root, emit, cache)
    local metadata = read(ctx, root .. "/metadata.json", ctx.json.parse, cache)
    local plugin = type(metadata) == "table" and metadata.KPlugin or nil
    if type(plugin) ~= "table" then plugin = {} end
    local lang = locale(ctx)
    local name = plugin["Name[" .. lang .. "]"]
        or plugin["Name[" .. lang:match("^[^_]*") .. "]"] or plugin.Name
    if type(name) ~= "string" or name == "" then name = ctx.filename(root) end
    local has_dark = ctx.fs.is_dir(root .. "/contents/images_dark")
    for _, variant in ipairs({ "images", "images_dark" }) do
        local dir = root .. "/contents/" .. variant
        if ctx.fs.is_dir(dir) then
            local label = name
            if has_dark then label = label .. (variant == "images" and " · Light" or " · Dark") end
            walk(ctx, dir, 0, function(path) emit(path, label) end)
        end
    end
end

local function children(node, tag)
    local result = {}
    for _, child in ipairs(node.children) do
        if child.tag == tag then result[#result + 1] = child end
    end
    return result
end

local function title(ctx, node)
    local fallback, language
    local lang = locale(ctx)
    for _, name in ipairs(children(node, "name")) do
        local value = trim(name.text)
        local code = (name.attrs["xml:lang"] or ""):gsub("-", "_")
        if code == lang and value ~= "" then return value end
        if code == lang:match("^[^_]*") and value ~= "" then language = value end
        if code == "" and value ~= "" then fallback = value end
    end
    return language or fallback
end

local function file_path(ctx, node, owner)
    local path = trim(node.text)
    if path == "" then return nil end
    if path:sub(1, 1) ~= "/" then path = ctx.fs.parent(owner) .. "/" .. path end
    return path
end

local function static_files(ctx, path, cache)
    local document = read(ctx, path, ctx.xml.parse, cache)
    local result = {}
    if not document or document.tag ~= "background" then return result end
    for _, static in ipairs(children(document, "static")) do
        for _, file in ipairs(children(static, "file")) do
            local image = file_path(ctx, file, path)
            if image then result[#result + 1] = image end
            for _, size in ipairs(children(file, "size")) do
                image = file_path(ctx, size, path)
                if image then result[#result + 1] = image end
            end
        end
    end
    return result
end

local function gnome_metadata(ctx, directory, cache)
    if cache[directory] then return cache[directory] end
    local index = { names = {}, excluded = {} }
    cache[directory] = index
    for _, path in ipairs(ctx.fs.list_files(directory)) do
        if (ctx.extension(path) or ""):lower() == "xml" then
            local document = read(ctx, path, ctx.xml.parse, cache)
            if document and document.tag == "wallpapers" then
                for _, item in ipairs(children(document, "wallpaper")) do
                    local name = title(ctx, item)
                    for _, child in ipairs(item.children) do
                        if child.tag == "filename" or child.tag == "filename-dark" or child.tag == "thumbnail" then
                            local image = file_path(ctx, child, path)
                            if image then
                                local files = { image }
                                if (ctx.extension(image) or ""):lower() == "xml" then
                                    files = static_files(ctx, image, cache)
                                end
                                for _, resource in ipairs(files) do
                                    local key = ctx.fs.canonicalize(resource) or resource
                                    if child.tag == "thumbnail" or item.attrs.deleted == "true" then
                                        index.excluded[key] = true
                                    elseif name then
                                        local label = name
                                        if #files > 1 then
                                            label = label .. " · " .. ctx.filename(resource)
                                        elseif child.tag == "filename-dark" then
                                            label = label .. " · Dark"
                                        elseif #children(item, "filename-dark") > 0 then
                                            label = label .. " · Light"
                                        end
                                        index.names[key] = index.names[key] or label
                                    end
                                end
                            end
                        end
                    end
                end
            end
        end
    end
    return index
end

function M.collect(ctx, root, kind, emit, cache)
    if kind == "wallpapers" then
        if ctx.fs.is_dir(root .. "/contents/images") or ctx.fs.is_dir(root .. "/contents/images_dark") then
            kde_package(ctx, root, emit, cache)
        else
            for _, path in ipairs(ctx.fs.list_files(root)) do emit(path) end
            for _, dir in ipairs(sorted(ctx.list_dirs(root))) do
                if ctx.fs.is_file(dir .. "/metadata.json") or ctx.fs.is_dir(dir .. "/contents/images")
                    or ctx.fs.is_dir(dir .. "/contents/images_dark") then
                    kde_package(ctx, dir, emit, cache)
                else
                    walk(ctx, dir, 1, emit)
                end
            end
        end
        return
    end
    local base = clean(ctx.fs.canonicalize(root) or root)
    while base ~= "/" and ctx.filename(base) ~= "backgrounds" do
        base = ctx.fs.parent(base) or "/"
    end
    local index = { names = {}, excluded = {} }
    if ctx.filename(base) == "backgrounds" then
        index = gnome_metadata(ctx, ctx.fs.parent(base) .. "/gnome-background-properties", cache)
    end
    walk(ctx, root, 6, function(path)
        local key = ctx.fs.canonicalize(path) or path
        if not index.excluded[key] then emit(path, index.names[key]) end
    end)
end

return M
