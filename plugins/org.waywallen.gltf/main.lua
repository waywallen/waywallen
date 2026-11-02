local source = import("gltf.source")
local wallpaper = import("gltf.wallpaper")

local M = {}

function M.info()
    return {
        name = "gltf",
        display_name = "glTF",
        capabilities = {
            source = {
                types = {"gltf"},
                scan = true,
                library_label = tr("glTF Folder"),
                library_hint = tr("A directory containing glTF or GLB models. Subdirectories up to two levels deep are also scanned. Keep each model's external buffers and textures beside it."),
            },
            wallpaper = {
                apply = true,
            },
        },
    }
end

M.source = source
M.wallpaper = wallpaper

return M
