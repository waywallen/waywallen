local M = {}

local function text(value, maximum)
    if type(value) ~= "string" then
        return nil
    end
    if #value > maximum then
        error("Sketchfab text exceeds the supported length")
    end
    return value
end

local function url(value)
    local value = text(value, 4096)
    if value and value:match("^https?://") and not value:find("[%c]") then
        return value
    end
    return nil
end

local function count(value)
    if type(value) == "number" and value >= 0 and value <= 9007199254740991 and value == math.floor(value) then
        return value
    end
    return nil
end

function M.list(value, maximum, json)
    if json.kind(value) ~= "array" then
        error("Sketchfab returned an invalid list")
    end
    local length = #value
    if length > maximum then
        error("Sketchfab returned too many list entries")
    end
    local entries = 0
    for key in pairs(value) do
        entries = entries + 1
        if type(key) ~= "number" or key < 1 or key > length or key ~= math.floor(key) then
            error("Sketchfab returned an invalid list")
        end
    end
    if entries ~= length then
        error("Sketchfab returned an invalid list")
    end
    return value
end

local function preview(images, target, json)
    if images == nil or images == json.null then
        return nil
    end
    local chosen, smallest, largest, fallback
    for _, image in ipairs(M.list(images, 32, json)) do
        if json.kind(image) == "object" then
            local image_url = url(image.url)
            local width, height = count(image.width), count(image.height)
            if image_url and width and height and width > 0 and height > 0 then
                local area = width * height
                if math.max(width, height) >= target and (not smallest or area < smallest) then
                    chosen, smallest = image_url, area
                end
                if not largest or area > largest then
                    fallback, largest = image_url, area
                end
            end
        end
    end
    return chosen or fallback or ""
end

local function names(values, json)
    if values == nil or values == json.null then
        return nil
    end
    local out = {}
    for _, value in ipairs(M.list(values, 128, json)) do
        if json.kind(value) == "object" then
            local name = text(value.name, 256) or text(value.slug, 256)
            if name and name ~= "" then
                table.insert(out, name)
            end
        end
    end
    return out
end

function M.valid_model_id(value)
    return type(value) == "string" and (
        (#value == 32 and value:match("^[a-fA-F0-9]+$") ~= nil)
        or (#value == 27 and value:match("^[a-zA-Z0-9]+$") ~= nil)
    )
end

function M.normalize(item, json)
    if json.kind(item) ~= "object" or not M.valid_model_id(item.uid) then
        error("Sketchfab returned an invalid model id")
    end
    local model = {
        id = item.uid,
        title = text(item.name, 1024),
        description = text(item.description, 65536),
        web_url = url(item.viewerUrl),
        tags = names(item.tags, json),
        categories = names(item.categories, json),
        vertices = count(item.vertexCount),
        faces = count(item.faceCount),
        animations = count(item.animationCount),
    }
    if type(item.isDownloadable) == "boolean" then
        model.downloadable = item.isDownloadable
    end
    if type(item.isAgeRestricted) == "boolean" then
        model.restricted = item.isAgeRestricted
    end
    if json.kind(item.user) == "object" then
        model.author = {
            name = text(item.user.displayName, 1024),
            username = text(item.user.username, 1024),
            url = url(item.user.profileUrl),
        }
        if json.kind(item.user.avatar) == "object" then
            model.author.avatar = preview(item.user.avatar.images, 128, json)
        end
    end
    if json.kind(item.thumbnails) == "object" then
        model.preview_url = preview(item.thumbnails.images, 512, json)
    end
    if json.kind(item.license) == "object" then
        model.license = {
            label = text(item.license.label, 1024),
            name = text(item.license.fullName, 1024),
            slug = text(item.license.slug, 256),
            url = url(item.license.url),
            requirements = text(item.license.requirements, 4096),
        }
    end
    if json.kind(item.archives) == "object" then
        model.archives = {}
        for _, format in ipairs({ "glb", "gltf", "usdz" }) do
            local archive = item.archives[format]
            if json.kind(archive) == "object" and count(archive.size) ~= nil then
                model.archives[format] = { size = count(archive.size) }
            end
        end
    end
    if model.title == "" then
        model.title = nil
    end
    for _, members in ipairs({ model.author or {}, model.license or {} }) do
        for key, value in pairs(members) do
            if value == "" then
                members[key] = nil
            end
        end
    end
    return model
end

function M.merge(previous, incoming)
    local out = {}
    for key, value in pairs(previous or {}) do
        out[key] = value
    end
    for key, value in pairs(incoming) do
        if key == "license" or key == "author" or key == "archives" then
            local combined = {}
            for member, item in pairs(out[key] or {}) do
                combined[member] = item
            end
            for member, item in pairs(value) do
                combined[member] = item
            end
            out[key] = combined
        else
            out[key] = value
        end
    end
    return out
end

function M.search_item(model)
    local author = model.author or {}
    return {
        id = model.id,
        title = model.title or ("Sketchfab " .. model.id),
        preview_url = model.preview_url or "",
        author = author.name or author.username or "",
        author_avatar_url = author.avatar or "",
        wp_type = "model",
    }
end

function M.details(model)
    local author, license = model.author or {}, model.license or {}
    local fields = {}
    local function field(key, label, value, link)
        if value ~= nil and tostring(value) ~= "" then
            table.insert(fields, { key = key, label = label, value = tostring(value), url = link })
        end
    end
    field("license", tr("License"), license.name or license.label or license.slug, license.url)
    field("license_requirements", tr("License requirements"), license.requirements)
    if model.categories and #model.categories > 0 then
        field("categories", tr("Categories"), table.concat(model.categories, ", "))
    end
    field("vertices", tr("Vertices"), model.vertices)
    field("faces", tr("Faces"), model.faces)
    field("animations", tr("Animations"), model.animations)
    field("downloadable", tr("Downloadable"), model.downloadable)
    for _, entry in ipairs({
        { "glb", tr("GLB archive") },
        { "gltf", tr("glTF archive") },
        { "usdz", tr("USDZ archive") },
    }) do
        local archive = (model.archives or {})[entry[1]]
        if archive and archive.size ~= nil then
            field("archive_" .. entry[1], entry[2], tostring(archive.size) .. " B")
        end
    end
    return {
        author = author.name or author.username or "",
        author_avatar_url = author.avatar or "",
        author_url = author.url or "",
        description = model.description or "",
        size = "",
        web_url = model.web_url or "",
        tags = model.tags or {},
        fields = fields,
    }
end

function M.download(model)
    local author, license = model.author or {}, model.license or {}
    local attribution = {
        author = author.name or author.username or "",
        author_url = author.url or "",
        license = license.name or license.label or license.slug or "",
        license_url = license.url or "",
        requirements = license.requirements or "",
    }
    if attribution.author == "" or attribution.license == "" then
        error("Sketchfab model author and license information are required for downloading")
    end
    local web_url = model.web_url or ("https://sketchfab.com/models/" .. model.id)
    local credits = {
        "Provided by Sketchfab",
        "Author: " .. attribution.author,
        "Model: " .. web_url,
        "License: " .. attribution.license,
    }
    if attribution.author_url ~= "" then table.insert(credits, "Author URL: " .. attribution.author_url) end
    if attribution.license_url ~= "" then table.insert(credits, "License URL: " .. attribution.license_url) end
    if attribution.requirements ~= "" then table.insert(credits, attribution.requirements) end
    local description = table.concat(credits, "\n")
    if model.description and model.description ~= "" then
        description = model.description .. "\n\n" .. description
    end
    return {
        wp_type = "gltf",
        title = model.title or ("Sketchfab " .. model.id),
        preview_url = model.preview_url or "",
        description = description,
        tags = model.tags or {},
        external_id = model.id,
        web_url = web_url,
        content_rating = model.restricted ~= nil and (model.restricted and "Mature" or "Everyone") or nil,
        attribution = attribution,
    }
end

return M
