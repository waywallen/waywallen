local source = import("image.source")
local wallpaper = import("image.wallpaper")

local M = {}

function M.info()
    return {
        name = "image",
        capabilities = {
            source = {
                types = {"image"},
                scan = true,
                auto_detect = true,
                library_label = tr("Image Folder"),
                library_hint = tr([[A directory containing image files. Subdirectories up to two levels deep are also scanned. Supported formats: PNG, JPEG, WebP, BMP, TIFF, AVIF, GIF. Auto-detect also finds installed KDE and GNOME system wallpapers.]]),
            },
            wallpaper = {
                properties = true,
                apply = true,
            },
        },
    }
end

M.source = source
M.wallpaper = wallpaper

return M
