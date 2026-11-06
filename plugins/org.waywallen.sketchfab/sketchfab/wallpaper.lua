local map = import("sketchfab.map")
local M = {}

function M.apply(entry, ctx)
    local artifact = ctx.remote_artifact(entry.resource)
    return {
        extras = {
            path = entry.resource,
            ["asset-root"] = artifact.root,
            ["camera-preset"] = map.camera_preset(artifact.metadata.provider_data, ctx.json.strict),
        },
        default_user_properties = {},
    }
end

return M
