local plugin = "share/waywallen/plugins/org.waywallen.gltf"

local function plugin_path(path)
  return plugin .. "/" .. path
end

lito.install({
  artifacts = {
    {
      target = { kind = "bin", name = "waywallen-gltf-renderer" },
      destination = "bin/waywallen-gltf-renderer",
    },
  },
  files = {
    { source = "plugin.toml", destination = plugin_path("plugin.toml") },
    { source = "main.lua", destination = plugin_path("main.lua") },
    { source = "gltf/source.lua", destination = plugin_path("gltf/source.lua") },
    { source = "gltf/wallpaper.lua", destination = plugin_path("gltf/wallpaper.lua") },
    { source = "i18n/zh-CN.po", destination = plugin_path("i18n/zh-CN.po") },
  },
  inventories = {
    {
      destination = plugin_path("files.txt"),
      relative_to = plugin,
    },
  },
})
