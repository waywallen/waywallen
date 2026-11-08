local M = {}

function M.properties(entry, ctx)
    local properties = {
        ["waywallen.scheme_color"] = {
            text = tr("Scheme color"),
            type = "color",
            value = {0.0, 0.0, 0.0, 1.0},
        },
    }
    if entry and entry.sub_type == "timeline" then
        properties["waywallen.image_variant"] = {
            text = tr("Wallpaper variant"), type = "combo", value = "day-night",
            options = {
                {label = tr("Simulated day and night"), value = "day-night"},
                {label = tr("Always light"), value = "light"},
                {label = tr("Always dark"), value = "dark"},
            },
        }
    end
    return properties
end

function M.apply(entry)
    return {
        extras = {
            path = entry.resource,
        },
        default_user_properties = {},
    }
end

return M
