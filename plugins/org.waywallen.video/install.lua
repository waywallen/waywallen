local plugin = "share/waywallen/plugins/org.waywallen.video"

local function plugin_path(path)
  return plugin .. "/" .. path
end

lito.install({
  artifacts = {
    {
      target = { kind = "bin", name = "waywallen-video-renderer" },
      destination = "bin/waywallen-video-renderer",
    },
  },
  files = {
    { source = "plugin.toml", destination = plugin_path("plugin.toml") },
    { source = "main.lua", destination = plugin_path("main.lua") },
    { source = "video/source.lua", destination = plugin_path("video/source.lua") },
    { source = "video/wallpaper.lua", destination = plugin_path("video/wallpaper.lua") },
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
