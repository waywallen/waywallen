local map = import("sketchfab.map")
local M = {}

local function number(value, minimum, maximum)
    if type(value) ~= "number" or value ~= value or math.abs(value) > 9007199254740991
        or (minimum and value < minimum) or (maximum and value > maximum) then
        error("environment-values-invalid", 0)
    end
    return value == 0 and 0.0 or value / 1.0
end

local function integer(value, minimum, maximum)
    value = number(value, minimum, maximum)
    if value ~= math.floor(value) then error("environment-values-invalid", 0) end
    return math.floor(value)
end

local function boolean(value)
    if type(value) ~= "boolean" then error("environment-values-invalid", 0) end
    return value
end

local function status(json, value, reason)
    return json.object { status = value, reason = reason }
end

local function input_status(json, value)
    if value == nil then return status(json, "missing") end
    if value == json.null then return status(json, "null") end
    if json.kind(value) ~= "object" then return status(json, "invalid", "expected-object") end
end

function M.describe(options, json, references)
    local source, background = options.environment, options.background
    local states = json.object {
        environment = input_status(json, source),
        background = input_status(json, background),
    }
    local fallback = json.object { ibl_intensity = 0.0, background = false }
    local enabled, visible = false, false
    if not states.environment then
        local ok, value = pcall(boolean, source.enable)
        if ok then
            enabled = value
            states.environment = status(json, enabled and "unsupported" or "disabled",
                enabled and "environment-resources-not-prepared" or nil)
        else
            states.environment = status(json, "invalid", "invalid-section-values")
        end
    end
    if not states.background then
        local ok, state, reason = pcall(function()
            if background.enable == "color" then
                map.list(background.color, 3, json)
                if #background.color ~= 3 then error("invalid color") end
                local color = json.array {}
                for _, component in ipairs(background.color) do
                    component = number(component, 0, 1)
                    color[#color + 1] = component <= 0.04045 and component / 12.92
                        or ((component + 0.055) / 1.055) ^ 2.4
                end
                fallback.background_color = color
                return "mapped"
            elseif background.enable == false then
                return "disabled"
            elseif background.enable == "environment" then
                visible = true
                return "unsupported", "environment-resources-not-prepared"
            elseif background.enable == "fixed" then
                return "unsupported", "fixed-background-resource-not-managed"
            end
            return "unsupported", "background-mode-unsupported"
        end)
        states.background = status(json, ok and state or "invalid", not ok and "invalid-section-values" or reason)
    end
    if not enabled and not visible then return fallback, states end
    local function failure(state, reason)
        if enabled then states.environment = status(json, state, reason) end
        if visible then states.background = status(json, state, reason) end
        return fallback, states
    end
    if not references then return failure("unsupported", "unverified-export-reference") end
    if json.kind(source) ~= "object" or states.environment.status == "invalid" then
        return failure("invalid", "invalid-environment-reference")
    end
    local ok, intent = pcall(function()
        if not map.valid_model_id(source.uid) then error("environment-id-invalid", 0) end
        local exposure = number(source.exposure, 0)
        local rotation = number(source.rotation)
        local blur = source.blur == nil and 0.0 or number(source.blur, 0, 1)
        local background_exposure = source.backgroundExposure == nil and 1.0
            or number(source.backgroundExposure, 0)
        local shadow = source.shadowEnable ~= nil and boolean(source.shadowEnable) or false
        local c, s = math.cos(rotation), math.sin(rotation)
        return json.object {
            uid = source.uid,
            lighting = enabled,
            background = visible,
            blur = blur,
            shadow = enabled and shadow,
            profile = json.object {
                reference = "scene-root",
                basis = json.array { c, -s, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
                    -s, -c, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0 },
                ibl_intensity = enabled and exposure or 0.0,
                background = visible,
                background_intensity = number(exposure * background_exposure, 0),
                background_color = fallback.background_color,
            },
        }
    end)
    if not ok then return failure("invalid", "invalid-section-values") end
    return fallback, states, intent
end

local function power_of_two(value, maximum)
    if type(value) ~= "number" or value < 1 or value > maximum or value ~= math.floor(value) then return false end
    while value > 1 do
        if value % 2 ~= 0 then return false end
        value = value / 2
    end
    return true
end

local function texture(descriptor, kind, format, encoding, json)
    local selected
    for _, value in ipairs(map.list(descriptor.textures, 32, json)) do
        if json.kind(value) == "object" and value.type == kind and value.format == format
            and value.encoding == encoding then
            if selected then error("environment-texture-ambiguous", 0) end
            selected = value
        end
    end
    if not selected then error("environment-texture-layout-unsupported", 0) end
    return selected
end

local function image(texture, maximum, blur, json)
    local selected, selected_blur, distance
    for _, value in ipairs(map.list(texture.images, 64, json)) do
        if json.kind(value) == "object" and power_of_two(value.width, 9007199254740991)
            and value.height == value.width then
            if blur ~= nil then
                local delta = math.abs(number(value.blur, 0, 1) - blur)
                if distance == nil or delta < distance then
                    selected_blur, distance = value.blur, delta
                    selected = value.width <= maximum and value or nil
                elseif delta == distance and value.blur == selected_blur and value.width <= maximum
                    and (not selected or value.width > selected.width) then
                    selected = value
                end
            elseif value.width <= maximum and (not selected or value.width > selected.width) then
                selected = value
            end
        end
    end
    if not selected then error("environment-texture-size-unsupported", 0) end
    return selected
end

local function compressed_size(value, json)
    if value == nil or value == json.null then return nil end
    return integer(value, 1, 64 * 1024 * 1024)
end

local function image_identity(value, json)
    if not map.valid_model_id(value.uid) then error("environment-image-id-invalid", 0) end
    local blur
    if value.blur ~= nil and value.blur ~= json.null then blur = number(value.blur, 0, 1) end
    return json.object {
        uid = value.uid,
        width = value.width,
        height = value.height,
        blur = blur,
        size = compressed_size(value.sizeCompressed, json),
    }
end

function M.plan(ctx, model_id, intent, payload)
    local json, descriptor = ctx.json.strict
    if json.kind(payload) ~= "object" then error("environment-descriptor-invalid", 0) end
    for _, value in ipairs(map.list(payload.results, 128, json)) do
        if json.kind(value) == "object" and value.uid == intent.uid then
            if descriptor then error("environment-id-ambiguous", 0) end
            descriptor = value
        end
    end
    if not descriptor then error("environment-id-not-found", 0) end
    if descriptor.processing ~= "Succeeded" then error("environment-not-ready", 0) end
    local background_texture = texture(descriptor, "background", "cubemap", "luv", json)
    local specular_texture = texture(descriptor, "specular_ue4", "cubemap", "luv", json)
    local brdf_texture = texture(descriptor, "brdf_ue4", "lut", "rg16", json)
    local background = image(background_texture, 512, intent.blur, json)
    local specular = image(specular_texture, 1024, nil, json)
    local brdf = image(brdf_texture, 512, nil, json)
    local limit = integer(specular_texture.limitSize, 1, specular.width)
    if not power_of_two(limit, specular.width) then error("environment-specular-limit-invalid", 0) end
    local sh = json.array {}
    map.list(descriptor.diffuseSPH, 256, json)
    if #descriptor.diffuseSPH < 27 then error("environment-diffuse-sh-invalid", 0) end
    for i = 1, 27 do sh[i] = number(descriptor.diffuseSPH[i]) end
    local resources, urls, bytes = {}, {}, 0
    local function resource(id, value, expected)
        local url = value.file
        if type(url) ~= "string" or #url > 16384 or url:find("[%c]")
            or not url:match("^https://media%.sketchfab%.com/") then
            error("environment-resource-url-invalid", 0)
        end
        local parsed = ctx.url.parse(url)
        local prefix = "/environments/" .. intent.uid .. "/"
        if not parsed or parsed.scheme ~= "https" or parsed.host ~= "media.sketchfab.com"
            or parsed.port ~= 443 or parsed.fragment ~= nil or parsed.path:sub(1, #prefix) ~= prefix then
            error("environment-resource-url-invalid", 0)
        end
        if value.sizeUncompressed ~= nil and value.sizeUncompressed ~= json.null
            and integer(value.sizeUncompressed, 1, 64 * 1024 * 1024) ~= expected then
            error("environment-resource-size-invalid", 0)
        end
        local size = compressed_size(value.sizeCompressed, json)
        if urls[url] then
            if urls[url].size ~= size then error("environment-resource-size-conflict", 0) end
            return urls[url].id
        end
        bytes = bytes + (size or 0)
        if bytes > 128 * 1024 * 1024 then error("environment-resource-budget-exceeded", 0) end
        local entry = { id = id, url = url, size = size }
        resources[#resources + 1], urls[url] = entry, entry
        return id
    end
    local mip_count, specular_bytes, size = 0, 0, specular.width
    repeat
        mip_count, specular_bytes, size = mip_count + 1, specular_bytes + 24 * size * size, math.floor(size / 2)
    until size == 0
    local parameters = json.object {
        version = 1,
        background = json.object {
            resource = resource("background", background, 24 * background.width * background.width),
            face_size = background.width,
        },
        specular = json.object {
            resource = resource("specular", specular, specular_bytes),
            face_size = specular.width,
            limit_size = limit,
        },
        brdf = json.object {
            resource = resource("brdf", brdf, 4 * brdf.width * brdf.height),
            width = brdf.width, height = brdf.height,
        },
        diffuse_sh = sh,
        output_prefix = "environment",
    }
    local name = descriptor.name
    if type(name) ~= "string" or #name > 256 then name = nil end
    local identity = json.object {
        uid = intent.uid,
        name = name,
        source = "https://sketchfab.com/i/models/" .. model_id .. "/environments?optimized=1",
        background = image_identity(background, json),
        specular = image_identity(specular, json),
        brdf = image_identity(brdf, json),
    }
    intent.mip_count = mip_count
    intent.lod_scale = math.log(specular.width / limit) / math.log(2)
    return identity, resources, parameters
end

function M.attach(ctx, data, intent, payload)
    if not intent then return end
    local json, value = ctx.json.strict, data.sketchfab
    local ok, identity, resources, parameters = pcall(M.plan, ctx, value.model_id, intent, payload)
    if not ok then
        local reason = type(identity) == "string" and identity:match("^environment%-[a-z%-]+$")
            or "environment-descriptor-invalid"
        if intent.lighting then value.sections.environment = status(json, "unsupported", reason) end
        if intent.background then value.sections.background = status(json, "unsupported", reason) end
        return
    end
    value.environment = identity
    value.pending_environment = intent
    if intent.lighting then value.sections.environment = status(json, "pending", "environment-preparation-required") end
    if intent.background then value.sections.background = status(json, "pending", "environment-preparation-required") end
    return resources, parameters
end

function M.unavailable(data, intent, json)
    if not intent then return end
    if intent.lighting then data.sketchfab.sections.environment = status(json, "unavailable", "environment-request-failed") end
    if intent.background then data.sketchfab.sections.background = status(json, "unavailable", "environment-request-failed") end
end

local function relative_path(value)
    if type(value) ~= "string" or value == "" or #value > 4096 or value:sub(1, 1) == "/"
        or value:find("[\\%c]") or value:find(":", 1, true) then error("Invalid prepared environment path") end
    for part in (value .. "/"):gmatch("(.-)/") do
        if part == "" or part == "." or part == ".." then error("Invalid prepared environment path") end
    end
    return value
end

function M.finalize(ctx, data, prepared)
    local json = ctx.json.strict
    local value = json.kind(data) == "object" and data.sketchfab
    local intent = json.kind(value) == "object" and value.pending_environment
    if not intent then
        if prepared ~= nil and prepared ~= json.null then error("Unexpected prepared Sketchfab resources") end
        return
    end
    if json.kind(intent) ~= "object" or json.kind(prepared) ~= "object"
        or json.kind(prepared.preprocessed) ~= "object" then error("Prepared Sketchfab environment is missing") end
    local input = prepared.preprocessed
    local output = json.object {}
    for _, name in ipairs({ "background", "diffuse", "brdf" }) do
        output[name] = ctx.artifact_resource_path(relative_path(input[name]))
    end
    output.specular = json.array {}
    map.list(input.specular, 11, json)
    if #input.specular ~= intent.mip_count then error("Prepared Sketchfab specular mip count is invalid") end
    for _, path in ipairs(input.specular) do
        output.specular[#output.specular + 1] = ctx.artifact_resource_path(relative_path(path))
    end
    if input.specular_lod_scale ~= intent.lod_scale or input.specular_lod_exponent ~= 0.5
        or input.brdf_filter ~= "nearest" then error("Prepared Sketchfab environment sampling is invalid") end
    output.specular_lod_scale = input.specular_lod_scale
    output.specular_lod_exponent = input.specular_lod_exponent
    output.brdf_filter = input.brdf_filter
    intent.profile.preprocessed = output
    value.profile.environment = intent.profile
    if intent.lighting then
        value.sections.environment = status(json, intent.shadow and "partial" or "mapped",
            intent.shadow and "environment-probe-shadow-unsupported" or nil)
    end
    if intent.background then value.sections.background = status(json, "mapped") end
    value.pending_environment = nil
    return data
end

return M
