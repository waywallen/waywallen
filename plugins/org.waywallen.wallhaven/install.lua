local plugin = "share/waywallen/plugins/org.waywallen.wallhaven"

local function plugin_path(path)
  return plugin .. "/" .. path
end

lito.install({
  files = {
    { source = "plugin.toml", destination = plugin_path("plugin.toml") },
    { source = "main.lua", destination = plugin_path("main.lua") },
    { source = "wallhaven/api.lua", destination = plugin_path("wallhaven/api.lua") },
    { source = "wallhaven/discover.lua", destination = plugin_path("wallhaven/discover.lua") },
    { source = "wallhaven/map.lua", destination = plugin_path("wallhaven/map.lua") },
    { source = "wallhaven/session.lua", destination = plugin_path("wallhaven/session.lua") },
    { source = "wallhaven/source.lua", destination = plugin_path("wallhaven/source.lua") },
    { source = "wallhaven/wallpaper.lua", destination = plugin_path("wallhaven/wallpaper.lua") },
    { source = "i18n/de.po", destination = plugin_path("i18n/de.po") },
    { source = "i18n/ru.po", destination = plugin_path("i18n/ru.po") },
    { source = "i18n/zh-CN.po", destination = plugin_path("i18n/zh-CN.po") },
  },
  inventories = {
    {
      destination = plugin_path("files.txt"),
      relative_to = plugin,
    },
  },
})
