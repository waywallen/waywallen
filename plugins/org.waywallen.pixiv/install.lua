local plugin = "share/waywallen/plugins/org.waywallen.pixiv"
local files = {
    "plugin.toml",
    "main.lua",
    "pixiv/api.lua",
    "pixiv/browser.lua",
    "pixiv/session.lua",
    "pixiv/session_login.lua",
    "pixiv/map.lua",
    "pixiv/discover.lua",
    "i18n/zh-CN.po",
}
local installed = {}
for _, path in ipairs(files) do
    installed[#installed + 1] = { source = path, destination = plugin .. "/" .. path }
end
lito.install({
    files = installed,
    inventories = { { destination = plugin .. "/files.txt", relative_to = plugin } },
})
