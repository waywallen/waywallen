local M = {}

function M.apply(entry)
    return {
        extras = {
            path = entry.resource,
        },
        default_user_properties = {},
    }
end

return M
