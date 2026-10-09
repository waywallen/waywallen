local plugin = "share/waywallen/plugins/org.waywallen.image"

local function plugin_path(path)
  return plugin .. "/" .. path
end

lito.install({
  artifacts = {
    {
      target = { kind = "bin", name = "waywallen-image-renderer" },
      destination = "bin/waywallen-image-renderer",
    },
  },
  files = {
    { source = "plugin.toml", destination = plugin_path("plugin.toml") },
    { source = "main.lua", destination = plugin_path("main.lua") },
    { source = "image/source.lua", destination = plugin_path("image/source.lua") },
    { source = "image/wallpaper.lua", destination = plugin_path("image/wallpaper.lua") },
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
