local discover = import("wallhaven.discover")
local api = import("wallhaven.api")
local source = import("wallhaven.source")
local wallpaper = import("wallhaven.wallpaper")
local session = import("wallhaven.session")

local M = {}
local account_group = tr("Wallhaven account")

function M.info()
    return {
        name = "wallhaven",
        display_name = "Wallhaven",
        status = {
            {
                id = "wallhaven_account",
                label = tr("Status"),
                group = "account",
                group_label = account_group,
                order = 10,
            },
        },
        actions = {
            {
                id = "wallhaven_sign_in",
                kind = "form",
                label = tr("Log in to Wallhaven"),
                description = tr([[Use an API key from your Wallhaven account settings. Do not enter your Wallhaven password.]]),
                group = "account",
                group_label = account_group,
                order = 20,
                required_for_browsing = false,
                fields = {
                    {
                        key = "api_key",
                        label = tr("API key"),
                        description = tr("Copy the API key from Wallhaven Account Settings."),
                        placeholder = tr("Wallhaven API key"),
                        secret = true,
                        required = true,
                    },
                },
            },
            {
                id = "wallhaven_sign_out",
                kind = "invoke",
                label = tr("Remove API key"),
                group = "account",
                group_label = account_group,
                order = 21,
            },
        },
        capabilities = {
            discover = {
                search = true,
                details = true,
                download = true,
                sorts = {
                    { key = "trend", label = tr("Trending") },
                    { key = "recent", label = tr("Recent") },
                    { key = "popular", label = tr("Popular") },
                },
                filters = api.filters,
            },
            wallpaper = {
                apply = true,
            },
        },
    }
end

M.lifecycle = {}
M.lifecycle.load = session.load
M.lifecycle.save = session.save
M.lifecycle.check = session.check

M.actions = {}
function M.actions.status(ctx)
    local checked = session.current_check()
    local active = checked.state == "signed_in"
    return {
        status = { wallhaven_account = checked.display_value or "" },
        actions = {
            wallhaven_sign_in = { visible = not active, enabled = not active },
            wallhaven_sign_out = { visible = active, enabled = active },
        },
    }
end
function M.actions.invoke(ctx, action_id, values)
    if action_id == "wallhaven_sign_in" then
        session.sign_in(ctx, values and values.api_key)
    elseif action_id == "wallhaven_sign_out" then
        session.sign_out()
    else
        error("unsupported Wallhaven action")
    end
end

M.discover = discover
M.source = source
M.wallpaper = wallpaper

return M
