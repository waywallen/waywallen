local M = {}

function M.apply(entry, ctx)
    return {
        extras = {
            path = entry.resource,
            ["asset-root"] = ctx.remote_artifact_root(entry.resource),
        },
        default_user_properties = {},
    }
end

return M
