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
                settings = {
                    {key = "scan_mode", type = "string", default = "files", choices = {"files", "wallpapers"},
                     choice_labels = {tr("Image files"), tr("Desktop wallpapers")},
                     label = tr("Scan mode"), description = tr("Scan image files or desktop wallpaper definitions such as KDE and GNOME wallpapers.")},
                },
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
