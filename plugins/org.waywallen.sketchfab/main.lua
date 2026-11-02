local api = import("sketchfab.api")
local discover = import("sketchfab.discover")
local session = import("sketchfab.session")
local source = import("sketchfab.source")
local wallpaper = import("sketchfab.wallpaper")

local M = {}
local account_group = tr("Sketchfab account")

function M.info()
    return {
        name = "sketchfab",
        display_name = "Sketchfab",
        status = {
            {
                id = "sketchfab_account",
                label = tr("Status"),
                group = "account",
                group_label = account_group,
                order = 10,
            },
        },
        actions = {
            {
                id = "sketchfab_sign_in",
                kind = "form",
                label = tr("Log in to Sketchfab"),
                description = tr("Use your Sketchfab API Token from https://sketchfab.com/settings/password. Do not enter your password or Epic Games token."),
                group = "account",
                group_label = account_group,
                order = 20,
                required_for_browsing = false,
                fields = {
                    {
                        key = "api_token",
                        label = tr("API Token"),
                        description = tr("Copy the API Token from Sketchfab Settings → Password & API."),
                        placeholder = tr("Sketchfab API Token"),
                        secret = true,
                        required = true,
                    },
                },
            },
            {
                id = "sketchfab_sign_out",
                kind = "invoke",
                label = tr("Remove API Token"),
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
                resolve = true,
                remote_hint = tr("Log in with a Sketchfab API Token to download models."),
                pagination = "cursor",
                sorts = {
                    { key = "relevance", label = tr("Relevance") },
                    { key = "recent", label = tr("Recently published") },
                    { key = "likes", label = tr("Most liked") },
                    { key = "views", label = tr("Most viewed") },
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
    local active = session.authenticated()
    local verified = checked.state == "signed_in"
    return {
        status = { sketchfab_account = checked.display_value },
        actions = {
            sketchfab_sign_in = { visible = not verified, enabled = not verified },
            sketchfab_sign_out = { visible = active, enabled = active },
        },
    }
end

function M.actions.invoke(ctx, action_id, values)
    if action_id == "sketchfab_sign_in" then
        session.sign_in(ctx, values and values.api_token)
    elseif action_id == "sketchfab_sign_out" then
        session.sign_out()
        ctx.http:clear_cookies()
    else
        error("Unsupported Sketchfab action")
    end
end

M.discover = discover
M.source = source
M.wallpaper = wallpaper

return M
