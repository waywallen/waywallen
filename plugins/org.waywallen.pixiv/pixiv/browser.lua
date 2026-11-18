local api = import("pixiv.api")
local session = import("pixiv.session")
local M = {}

function M.begin(ctx, action_id)
    if action_id ~= "sign_in" then error("Unsupported Pixiv browser action") end
    local verifier = ctx.random.hex(32)
    return {
        key = { verifier = verifier, revision = session.revision(), expires_at = ctx.time.unix() + 300 },
        browser = {
            url = api.login_url(ctx, ctx.crypto.sha256_base64url(verifier)),
            title = "Pixiv",
            redirects = { api.custom_redirect, api.redirect },
        },
        expires_in_ms = 300000,
        title = tr("Log in to Pixiv"),
        instruction = tr("Complete sign-in in the browser window. The window will close when authorization finishes."),
    }
end

function M.handle(ctx, key, event)
    if type(key) ~= "table" or key.cancelled or key.used or key.revision ~= session.revision()
        or ctx.time.unix() >= key.expires_at then
        return { state = "failed", error = tr("This Pixiv sign-in is no longer active. Start again.") }
    end
    if event.event ~= "redirect" then return { state = "waiting" } end
    local parsed = type(event.uri) == "string" and ctx.url.parse(event.uri)
    local expected = parsed and (
        (parsed.scheme == "pixiv" and parsed.host == "account" and parsed.path == "/login" and parsed.port == nil)
        or (parsed.scheme == "https" and parsed.host == "app-api.pixiv.net" and parsed.port == 443
            and parsed.path == "/web/v1/users/auth/pixiv/callback"))
    if not expected or parsed.fragment ~= nil then return { state = "waiting" } end
    local query = ctx.url.decode(parsed.query or "")
    local code = query.code
    if query.error ~= nil or type(code) ~= "string" or #code == 0 or #code > 4096 or code:find("[%c]") then
        return { state = "failed", error = tr("Pixiv did not return an authorization code. Start again.") }
    end
    key.used = true
    local verifier = key.verifier
    key.verifier = nil
    local ok, checked = pcall(session.exchange, ctx, code, verifier)
    if not ok then return { state = "failed", error = tr("Pixiv sign-in failed. Please try again.") } end
    return { state = "succeeded", display_value = checked.display_value }
end

function M.cancel(ctx, key)
    key.cancelled = true
    key.verifier = nil
end

return M
