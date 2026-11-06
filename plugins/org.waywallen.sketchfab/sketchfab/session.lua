local api = import("sketchfab.api")
local M = {}

local FORMAT_VERSION = "sketchfab-session-v1"
local MAX_TOKEN_LENGTH = 4096
local token = ""
local revision = 0
local verified_until = 0
local last_check = { state = "signed_out", display_value = tr("Guest mode") }

local function valid_token(value)
    return type(value) == "string" and #value > 0 and #value <= MAX_TOKEN_LENGTH
        and not value:find("[^!-~]")
end

local function replace_token(value)
    if token ~= value then
        token = value
        revision = revision + 1
    end
    verified_until = 0
end

local function remember(check)
    last_check = check
    return check
end

local function restore(blob)
    local prefix = FORMAT_VERSION .. "\n"
    if type(blob) ~= "string" or #blob > #prefix + 5 + MAX_TOKEN_LENGTH
        or blob:sub(1, #prefix) ~= prefix then
        return ""
    end
    local separator = blob:find("\n", #prefix + 1, true)
    if not separator then
        return ""
    end
    local length = blob:sub(#prefix + 1, separator - 1)
    local value = blob:sub(separator + 1)
    if not length:match("^%d+$") or tonumber(length) ~= #value or not valid_token(value) then
        return ""
    end
    return value
end

local function profile_name(profile)
    if type(profile) ~= "table" or type(profile.uid) ~= "string" or #profile.uid ~= 32
        or not profile.uid:match("^[a-fA-F0-9]+$") or type(profile.username) ~= "string"
        or #profile.username == 0 or #profile.username > 256 or profile.username:find("[%c]") then
        return nil
    end
    return profile.username
end

function M.load(blob)
    replace_token(restore(blob))
    if token == "" then
        remember({ state = "signed_out", display_value = tr("Guest mode") })
    else
        remember({ state = "error", display_value = tr("API Token not verified") })
    end
end

function M.save()
    if token == "" then
        return ""
    end
    return FORMAT_VERSION .. "\n" .. tostring(#token) .. "\n" .. token
end

function M.check(ctx)
    if token == "" or ctx.time.unix() < verified_until then
        return last_check
    end
    local ok, profile, status = pcall(api.me, ctx, token)
    if ok and status == 401 then
        replace_token("")
        return remember({ state = "expired", display_value = tr("API Token expired") })
    end
    local username = ok and profile_name(profile)
    if not username then
        return remember({
            state = "error",
            display_value = tr("API Token verification failed"),
            error = tr("Could not verify the Sketchfab API Token. Try again later."),
        })
    end
    verified_until = ctx.time.unix() + 60
    return remember({ state = "signed_in", display_value = tr("Signed in as %1", username) })
end

function M.current_check()
    return last_check
end

function M.headers()
    if token == "" then
        return {}
    end
    return { Authorization = "Token " .. token }
end

function M.authenticated()
    return token ~= ""
end

function M.revision()
    return revision
end

function M.sign_in(ctx, value)
    local candidate = type(value) == "string" and value:match("^%s*(.-)%s*$") or ""
    if not valid_token(candidate) then
        error("Enter a valid Sketchfab API Token")
    end
    local ok, profile, status = pcall(api.me, ctx, candidate)
    if ok and status == 401 then
        error("Sketchfab rejected this API Token")
    end
    local username = ok and profile_name(profile)
    if not username then
        error("Sketchfab API Token verification failed; try again later")
    end
    replace_token(candidate)
    verified_until = ctx.time.unix() + 60
    remember({ state = "signed_in", display_value = tr("Signed in as %1", username) })
end

function M.sign_out()
    replace_token("")
    remember({ state = "signed_out", display_value = tr("Guest mode") })
end

return M
