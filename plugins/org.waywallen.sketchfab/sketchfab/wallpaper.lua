local presentation = import("sketchfab.presentation")
local M = {}

function M.apply(entry, ctx)
    local artifact = ctx.remote_artifact(entry.resource)
    return {
        extras = {
            path = entry.resource,
            ["asset-root"] = artifact.root,
            ["render-profile"] = presentation.render_profile(artifact.metadata, ctx.json.strict),
        },
        default_user_properties = {},
    }
end

return M
