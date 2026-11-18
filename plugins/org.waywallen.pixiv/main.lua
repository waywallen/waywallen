local browser = import("pixiv.browser")
local discover = import("pixiv.discover")
local session = import("pixiv.session")
local session_login = import("pixiv.session_login")

local M = {}
local account_group = tr("Pixiv account")

function M.info()
    return {
        name = "pixiv",
        display_name = "Pixiv",
        status = {
            { id = "account", label = tr("Status"), group = "account", group_label = account_group, order = 10 },
        },
        actions = {
            {
                id = "sign_in",
                kind = "browser_login",
                label = tr("Log in to Pixiv"),
                description = tr("Complete sign-in in the browser window. If unavailable, use session ID sign-in."),
                browse_description = tr("Log in to Pixiv."),
                browse_button_label = tr("Log in to Pixiv"),
                required_for_browsing = true,
                group = "account",
                group_label = account_group,
                order = 20,
            },
            {
                id = "sign_in_session", kind = "form", label = tr("Log in with session ID"),
                description = tr("Enter only the PHPSESSID in browser dev storage tab."),
                group = "account", group_label = account_group, order = 21, required_for_browsing = true,
                fields = {{ key = "session_id", label = tr("Session ID"), placeholder = "PHPSESSID",
                    description = tr("Copy the PHPSESSID value from a signed-in browser session."), secret = true, required = true }},
            },
            { id = "sign_out", kind = "invoke", label = tr("Log out"), group = "account", group_label = account_group, order = 22 },
        },
        capabilities = {
            discover = {
                search = true,
                details = true,
                download = true,
                artists = { search_text = false, sorts = {} },
                pagination = "cursor",
                filters = discover.filters,
                badges = discover.badges,
                sorts = discover.sorts,
            },
            wallpaper = { apply = true },
        },
    }
end

M.lifecycle = { load = session.load, save = session.save, check = session.check }
M.browserlogin = browser
M.discover = discover
M.actions = {}

function M.actions.status()
    local checked = session.current_check()
    local active = checked.state == "signed_in"
    return {
        status = { account = checked.display_value },
        actions = {
            sign_in = { visible = not active, enabled = not active },
            sign_in_session = { visible = not active, enabled = not active },
            sign_out = { visible = session.authenticated(), enabled = session.authenticated() },
        },
    }
end

function M.actions.invoke(ctx, action_id, values)
    if action_id == "sign_in_session" then
        local result = session_login.authorize(ctx, values and values.session_id)
        session.accept_login(ctx, result.payload, result.account_id)
    elseif action_id == "sign_out" then
        session.sign_out()
        ctx.http:clear_cookies()
    else
        error("Unsupported Pixiv action")
    end
end

M.wallpaper = {}
function M.wallpaper.apply(entry)
    return { extras = { path = entry.resource }, default_user_properties = {} }
end

M.source = {}
function M.source.remove(ctx, item)
    local path = item.resource or item.path
    if type(path) ~= "string" or path == "" then error("Pixiv item path is missing") end
    ctx.remove_remote_artifact(path)
end

return M
