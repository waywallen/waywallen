local M = {}

function M.remove(ctx, item)
    local path = item.resource or item.path
    if type(path) ~= "string" or path == "" then
        error("Sketchfab item path is missing")
    end
    ctx.remove_remote_artifact(path)
end

return M
