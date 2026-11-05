local api = import("sketchfab.api")
local map = import("sketchfab.map")
local environment = import("sketchfab.environment")
local M = {}
local LIMIT = 64 * 1024
local sections = { "orientation", "camera", "lighting", "environment", "background", "post_process", "ground", "materials" }

local function number(value, minimum, maximum)
    if type(value) ~= "number" or value ~= value or math.abs(value) > 9007199254740991
        or (minimum and value < minimum) or (maximum and value > maximum) then
        error("invalid number")
    end
    return value == 0 and 0.0 or value / 1.0
end

local function boolean(value)
    if type(value) ~= "boolean" then error("invalid boolean") end
    return value
end

local function vector(value, length, json, minimum, maximum)
    map.list(value, length, json)
    if #value ~= length then error("invalid vector") end
    local result = json.array {}
    for _, component in ipairs(value) do
        result[#result + 1] = number(component, minimum, maximum)
    end
    return result
end

local function status(json, value, reason)
    return json.object { status = value, reason = reason }
end

local function shadow_options(light, json)
    local bias = number(light.shadowBias, 0)
    return json.object {
        resolution = 1024,
        filter = "bilinear",
        bias = 0.00005,
        slope_bias = 0.05,
        max_bias = number(math.max(0.00005, bias * 2), 0),
        normal_bias = 0.0,
    }
end

local function snapshot(ctx, id)
    if not map.valid_model_id(id) then error("Invalid Sketchfab model id") end
    local json = ctx.json.strict
    return json.object {
        version = 2,
        adapter_version = 1,
        model_id = id,
        source = "https://sketchfab.com/i/models/" .. id .. "/options",
        captured_at = ctx.time.unix(),
        profile = json.null,
        sections = json.object {},
    }
end

function M.from_options(ctx, id, options)
    local json = ctx.json.strict
    local result = snapshot(ctx, id)
    if json.kind(options) ~= "object" then
        result.sections.options = status(json, "invalid", "expected-options-object")
        return json.object { sketchfab = result }
    end
    local profile = json.object { version = 2 }
    local references = true
    local environment_references = true
    local function section(name, value, convert)
        if value == nil then
            result.sections[name] = status(json, "missing")
        elseif value == json.null then
            result.sections[name] = status(json, "null")
        elseif json.kind(value) ~= "object" then
            result.sections[name] = status(json, "invalid", "expected-object")
        else
            local ok, state, reason, details = pcall(convert, value)
            result.sections[name] = status(json, ok and (state or "mapped") or "invalid",
                not ok and "invalid-section-values" or reason)
            if ok and details then result.sections[name].items = details end
        end
    end
    section("orientation", options.orientation, function(value)
        local matrix = vector(value.matrix, 16, json)
        for i, component in ipairs(matrix) do
            if i ~= 13 and i ~= 14 and i ~= 15
                and component ~= ((i == 1 or i == 6 or i == 11 or i == 16) and 1 or 0) then
                environment_references = false
            end
        end
        for i, component in ipairs(matrix) do
            if component ~= ((i == 1 or i == 6 or i == 11 or i == 16) and 1 or 0) then
                references = false
                return "unsupported", "non-identity-export-orientation"
            end
        end
    end)
    if result.sections.orientation.status == "invalid" or result.sections.orientation.status == "null" then
        references = false
        environment_references = false
    end
    section("camera", options.camera, function(value)
        if not references then return "unsupported", "unverified-export-reference" end
        local position, target = vector(value.position, 3, json), vector(value.target, 3, json)
        local fov = number(value.fov, 0, 180)
        if fov == 0 or fov == 180 or (position[1] == target[1] and position[2] == target[2]) then
            error("invalid camera")
        end
        profile.camera = json.object {
            reference = "scene-root",
            position = position,
            target = target,
            up = json.array { 0.0, 0.0, 1.0 },
            yfov = fov * math.pi / 180,
            clip_units = "world",
        }
    end)
    section("lighting", options.lighting, function(value)
        local directional, point, spots, details = json.array {}, json.array {}, json.array {}, json.array {}
        if not boolean(value.enable) then
            profile.directional_lights, profile.point_lights, profile.spot_lights = directional, point, spots
            return "disabled"
        end
        for _, light in ipairs(map.list(value.lights, 8, json)) do
            local ok, state, reason = pcall(function()
                if json.kind(light) ~= "object" then error("invalid light") end
                if not boolean(light.enable) then return "disabled" end
                local attached = boolean(light.attachedToCamera)
                local casts_shadow = light.castShadows ~= nil and boolean(light.castShadows) or false
                if not attached and not references and not (light.type == "SPOT" and environment_references) then
                    return "unsupported", "unverified-export-reference"
                end
                local matrix = vector(light.matrix, 16, json)
                if matrix[4] ~= 0 or matrix[8] ~= 0 or matrix[12] ~= 0 or matrix[16] ~= 1 then
                    error("invalid affine light matrix")
                end
                local entry = json.object {
                    color = vector(light.color, 3, json, 0),
                    intensity = number(number(light.intensity, 0) * math.pi, 0),
                    reference = attached and "view" or "scene-root",
                }
                if light.type == "DIRECTION" then
                    local x, y, z = matrix[9], matrix[10], matrix[11]
                    local length = math.sqrt(x * x + y * y + z * z)
                    if length == 0 then error("zero light direction") end
                    entry.direction = json.array { x / length, y / length, z / length }
                    if casts_shadow then
                        entry.shadow = shadow_options(light, json)
                    end
                    directional[#directional + 1] = entry
                elseif light.type == "POINT" then
                    entry.position = json.array { matrix[13], matrix[14], matrix[15] }
                    entry.attenuation = json.array { 1.0, 0.0, number(light.falloff, 0) }
                    entry.attenuation_units = "reference"
                    point[#point + 1] = entry
                    if casts_shadow then return "partial", "point-shadow-unsupported" end
                elseif light.type == "SPOT" then
                    local angle = number(light.angle, 0, 90)
                    local hardness = number(light.hardness, 0, 1)
                    local cutoff = math.cos(angle * math.pi / 180)
                    entry.position = json.array { matrix[13], matrix[14], matrix[15] }
                    entry.direction = json.array { -matrix[9], -matrix[10], -matrix[11] }
                    entry.inner_cos = cutoff + (1 - cutoff) * hardness
                    entry.outer_cos = cutoff
                    entry.attenuation = json.array { 1.0, 0.0, number(light.falloff, 0) }
                    entry.attenuation_units = "reference"
                    if casts_shadow and angle > 0 and angle < 90 then
                        entry.shadow = shadow_options(light, json)
                    end
                    spots[#spots + 1] = entry
                    if casts_shadow and not entry.shadow then
                        return "partial", "spot-shadow-projection-unsupported"
                    end
                else
                    return "unsupported", "light-type-unsupported"
                end
                return "mapped"
            end)
            details[#details + 1] = status(json, ok and state or "invalid", not ok and "invalid-light-values" or reason)
        end
        profile.directional_lights, profile.point_lights, profile.spot_lights = directional, point, spots
        for _, item in ipairs(details) do
            if item.status ~= "mapped" and item.status ~= "disabled" then
                return "partial", "some-lights-unavailable", details
            end
        end
        return "mapped", nil, details
    end)
    local fallback, environment_states, environment_intent = environment.describe(options, json, environment_references)
    if options.environment ~= nil or options.background ~= nil then profile.environment = fallback end
    result.sections.environment, result.sections.background = environment_states.environment, environment_states.background
    local post
    if json.kind(options.scene) == "object" then
        post = options.scene.postProcess
    elseif options.scene ~= nil then
        post = options.scene
    end
    section("post_process", post, function(value)
        local display = json.object {
            exposure = 1.0, tone_mapping = "linear", brightness = 1.0,
            saturation = 1.0, contrast = 0.0, fxaa = false, sharpen = 0.0, sampling_scale = 1.0,
        }
        if not boolean(value.enable) then profile.display = display; return "disabled" end
        local effects = json.object {}
        local function effect(name, convert)
            local input = value[name]
            if input == nil then effects[name] = status(json, "missing"); return end
            if input == json.null then effects[name] = status(json, "null"); return end
            local ok, state, reason = pcall(function()
                if json.kind(input) ~= "object" then error("invalid effect") end
                if not boolean(input.enable) then return "disabled" end
                return convert(input)
            end)
            effects[name] = status(json, ok and (state or "mapped") or "invalid",
                not ok and "invalid-effect-values" or reason)
        end
        effect("toneMapping", function(input)
            if input.method ~= "default" then return "unsupported", "tone-mapping-method-unsupported" end
            local exposure = number(input.exposure, 0)
            local brightness = number(1 + number(input.brightness, -1), 0)
            local saturation, contrast = number(input.saturation, 0), number(input.contrast, -1)
            display.exposure, display.brightness = exposure, brightness
            display.saturation, display.contrast = saturation, contrast
        end)
        effect("colorBalance", function(input)
            local balance = json.object {
                shadows = vector(input.low, 3, json, -1, 1),
                midtones = vector(input.mid, 3, json, -1, 1),
                highlights = vector(input.high, 3, json, -1, 1),
            }
            display.color_balance = balance
        end)
        effect("bloom", function(input)
            local bloom = json.object {
                intensity = number(input.factor, 0),
                threshold = number(input.threshold, 0),
                radius = number(input.radius, 0, 1),
            }
            if bloom.intensity < 0.01 then return "disabled", "below-effective-threshold" end
            display.bloom = bloom
        end)
        effect("sharpen", function(input) display.sharpen = number(input.factor, 0) end)
        effect("vignette", function(input)
            local amount, hardness = number(input.amount, 0, 1), number(input.hardness, 0, 1)
            local outer = 2 - amount - hardness
            local inner = math.min(hardness - amount, outer - 0.0001)
            if inner < 0 or outer <= 0 then return "unsupported", "negative-vignette-radius" end
            display.vignette = json.object {
                intensity = 1.0, inner_radius = inner, outer_radius = outer,
                center = json.array { 0.5, 0.5 },
            }
        end)
        effect("grain", function(input)
            if boolean(input.animated) then return "unsupported", "animated-grain-clock-unavailable" end
            display.grain = json.object { strength = number(input.factor, 0, 1), phase = 0.0 }
        end)
        for _, name in ipairs({ "taa", "dof", "ssao", "ssr", "chromaticAberration" }) do
            effect(name, function() return "unsupported", "effect-unsupported" end)
        end
        effects.fxaa = status(json, "unsupported", "runtime-triangle-dependent-default")
        profile.display = display
        for _, item in pairs(effects) do
            if item.status == "unsupported" or item.status == "invalid" then
                return "partial", "some-effects-unavailable", effects
            end
        end
        return "mapped", nil, effects
    end)
    section("ground", options.ground, function(value)
        if not boolean(value.enable) then return "disabled" end
        if not references then return "unsupported", "unverified-export-reference" end
        if value.shadowMode ~= "ShadowCatcher" then return "unsupported", "ground-mode-unsupported" end
        local position = vector(value.position, 3, json)
        local scale, opacity, fade = number(value.scale, 0), number(value.opacity, 0, 1), number(value.fade, 0, 1)
        if scale == 0 then error("invalid ground scale") end
        profile.shadow_catcher = json.object {
            reference = "scene-root", position = position, scale = scale,
            opacity = opacity * opacity, fade = 10 ^ fade - 1,
        }
    end)
    if options.materials == nil then
        result.sections.materials = status(json, "missing")
    elseif options.materials == json.null then
        result.sections.materials = status(json, "null")
    elseif (json.kind(options.materials) == "object" or json.kind(options.materials) == "array")
        and next(options.materials) == nil then
        result.sections.materials = status(json, "empty")
    else
        result.sections.materials = status(json, "unsupported", "export-material-binding-unavailable")
    end
    result.profile = profile
    local data = json.object { sketchfab = result }
    local encoded, bytes = pcall(json.encode, data)
    if not encoded or #bytes > LIMIT then
        result.profile = json.null
        result.sections = json.object { options = status(json, "unsupported",
            encoded and "presentation-budget-exceeded" or "presentation-values-not-encodable") }
        environment_intent = nil
    end
    return data, environment_intent
end

function M.capture(ctx, id)
    local ok, options = pcall(api.viewer_options, ctx, id)
    local data, intent, resources, prepare
    if ok then
        data, intent = M.from_options(ctx, id, options)
        if intent then
            local available, payload = pcall(api.environments, ctx, id)
            if available then
                resources, prepare = environment.attach(ctx, data, intent, payload)
            else
                environment.unavailable(data, intent, ctx.json.strict)
            end
        end
    else
        local value = snapshot(ctx, id)
        value.sections.options = status(ctx.json.strict, "unavailable", "options-request-failed")
        data = ctx.json.strict.object { sketchfab = value }
    end
    local json = ctx.json.strict
    local encoded, bytes = pcall(json.encode, data)
    if not encoded or #bytes > LIMIT then
        local value = data.sketchfab
        value.profile, value.pending_environment, value.environment = json.null, nil, nil
        value.sections = json.object { options = status(json, "unsupported",
            encoded and "presentation-budget-exceeded" or "presentation-values-not-encodable") }
        resources, prepare = nil, nil
    end
    if type(ctx.log) == "function" then
        local states = data.sketchfab.sections
        if states.options then ctx.log("sketchfab: presentation options " .. states.options.status) end
        for _, name in ipairs(sections) do
            if states[name] then ctx.log("sketchfab: presentation " .. name .. " " .. states[name].status) end
        end
    end
    return data, resources, prepare
end

function M.finalize(ctx, metadata, prepared)
    local data = environment.finalize(ctx, metadata.provider_data, prepared)
    if data and #ctx.json.strict.encode(data) > LIMIT then error("Sketchfab presentation snapshot exceeds 64 KiB") end
    return data
end

function M.render_profile(metadata, json)
    local data = metadata.provider_data
    local value = json.kind(data) == "object" and data.sketchfab
    if json.kind(value) ~= "object" or value.version ~= 2 or value.adapter_version ~= 1
        or not map.valid_model_id(value.model_id) or value.model_id ~= metadata.external_id then
        error("Sketchfab presentation snapshot is incompatible; remove and download the model again")
    end
    if value.profile == json.null then return nil end
    if value.pending_environment ~= nil then error("Sketchfab environment preparation is incomplete") end
    if json.kind(value.profile) ~= "object" or value.profile.version ~= 2 then
        error("Sketchfab presentation profile is invalid")
    end
    local encoded = json.encode_canonical(value.profile)
    if #encoded > LIMIT then error("Sketchfab presentation profile exceeds 64 KiB") end
    return encoded
end

return M
