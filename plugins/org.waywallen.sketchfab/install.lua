local plugin = "share/waywallen/plugins/org.waywallen.sketchfab"

local function plugin_path(path)
    return plugin .. "/" .. path
end

lito.install({
    files = {
        { source = "plugin.toml", destination = plugin_path("plugin.toml") },
        { source = "main.lua", destination = plugin_path("main.lua") },
        { source = "sketchfab/api.lua", destination = plugin_path("sketchfab/api.lua") },
        { source = "sketchfab/discover.lua", destination = plugin_path("sketchfab/discover.lua") },
        { source = "sketchfab/map.lua", destination = plugin_path("sketchfab/map.lua") },
        { source = "sketchfab/presentation.lua", destination = plugin_path("sketchfab/presentation.lua") },
        { source = "sketchfab/session.lua", destination = plugin_path("sketchfab/session.lua") },
        { source = "sketchfab/source.lua", destination = plugin_path("sketchfab/source.lua") },
        { source = "sketchfab/wallpaper.lua", destination = plugin_path("sketchfab/wallpaper.lua") },
        { source = "i18n/zh-CN.po", destination = plugin_path("i18n/zh-CN.po") },
    },
    inventories = {
        {
            destination = plugin_path("files.txt"),
            relative_to = plugin,
        },
    },
})
