local api = import("pixiv.api")
local M = {}

local FORMAT = "pixiv-session-v1\n"
local fields = { "access_token", "refresh_token", "expires_at", "user_id", "name" }
local state = {}
local revision = 0
local retry_after = 0
local last_check = { state = "signed_out", display_value = tr("Not signed in") }

local function valid_token(value)
    return type(value) == "string" and #value > 0 and #value <= 16384 and not value:find("[^!-~]")
end

local function remember(value)
    last_check = value
    return value
end

local function checked()
    return remember({ state = "signed_in", display_value = state.name })
end

local function replace(value, advance_revision)
    state = value
    retry_after = 0
    if advance_revision ~= false then revision = revision + 1 end
end

local function restore(blob)
    if type(blob) ~= "string" or #blob > 65536 or blob:sub(1, #FORMAT) ~= FORMAT then return nil end
    local result, position = {}, #FORMAT + 1
    for _, field in ipairs(fields) do
        local delimiter = blob:find(":", position, true)
        if not delimiter then return nil end
        local digits = blob:sub(position, delimiter - 1)
        if #digits > 5 or not digits:match("^%d+$") then return nil end
        local length = tonumber(digits)
        if length > 16384 or delimiter + length > #blob then return nil end
        result[field] = blob:sub(delimiter + 1, delimiter + length)
        position = delimiter + length + 1
    end
    if position ~= #blob + 1 or not valid_token(result.access_token) or not valid_token(result.refresh_token)
        or not result.user_id:match("^[1-9]%d*$") or #result.user_id > 20
        or #result.name == 0 or #result.name > 256 or result.name:find("[%c]") then return nil end
    result.expires_at = tonumber(result.expires_at)
    if not result.expires_at or result.expires_at < 0 or result.expires_at > 9007199254740991
        or result.expires_at ~= math.floor(result.expires_at) then return nil end
    return result
end

function M.load(blob)
    replace(restore(blob) or {})
    if state.refresh_token then checked() else remember({ state = "signed_out", display_value = tr("Not signed in") }) end
end

function M.save()
    if not state.refresh_token then return "" end
    local result = { FORMAT }
    for _, field in ipairs(fields) do
        local value = tostring(state[field])
        table.insert(result, tostring(#value) .. ":" .. value)
    end
    return table.concat(result)
end

function M.authenticated()
    return state.refresh_token ~= nil
end

function M.account_id()
    if not state.refresh_token then error("Log in to Pixiv to browse illustrations") end
    return state.user_id
end

function M.revision()
    return revision
end

function M.current_check()
    return last_check
end

local function accept(ctx, payload, refreshing, expected_account)
    payload = type(payload.response) == "table" and payload.response or payload
    if not valid_token(payload.access_token) or not valid_token(payload.refresh_token) then
        error("Pixiv returned invalid login credentials")
    end
    local lifetime = tonumber(payload.expires_in)
    if not lifetime or lifetime ~= math.floor(lifetime) or lifetime <= 0 or lifetime > 31536000 then
        error("Pixiv returned an invalid session expiry")
    end
    local user = payload.user
    if type(user) ~= "table" or type(user.name) ~= "string" or #user.name == 0
        or #user.name > 256 or user.name:find("[%c]") then error("Pixiv returned an invalid account") end
    local user_id = api.id(user.id)
    if refreshing and user_id ~= state.user_id then error("Pixiv refreshed a different account") end
    if expected_account and user_id ~= expected_account then error("Pixiv authorization returned a different account") end
    replace({
        access_token = payload.access_token,
        refresh_token = payload.refresh_token,
        expires_at = ctx.time.unix() + lifetime,
        user_id = user_id,
        name = user.name,
    }, not refreshing)
    return checked()
end

function M.accept_login(ctx, payload, expected_account)
    return accept(ctx, payload, false, expected_account)
end

function M.exchange(ctx, code, verifier)
    local payload, status = api.token(ctx, { grant_type = "authorization_code", code = code, verifier = verifier })
    if status < 200 or status >= 300 then error("Pixiv rejected the login; please try again") end
    return M.accept_login(ctx, payload)
end

local function refresh(ctx)
    if not state.refresh_token then error("Log in to Pixiv to browse illustrations") end
    if ctx.time.unix() < retry_after then error("Could not refresh the Pixiv session; try again later") end
    local payload, status = api.token(ctx, { grant_type = "refresh_token", refresh_token = state.refresh_token })
    if status == 400 or status == 401 then
        replace({}, false)
        remember({ state = "expired", display_value = tr("Session expired") })
        error("Pixiv session expired; log in again")
    end
    if status < 200 or status >= 300 then error("Could not refresh the Pixiv session; try again later") end
    return accept(ctx, payload, true)
end

function M.check(ctx)
    if not state.refresh_token then return last_check end
    if ctx.time.unix() + 60 < state.expires_at then return checked() end
    if ctx.time.unix() < retry_after then return last_check end
    local ok, result = pcall(refresh, ctx)
    if ok then return result end
    if not state.refresh_token then return last_check end
    retry_after = ctx.time.unix() + 15
    return remember({
        state = "error",
        display_value = tr("Could not refresh the Pixiv session"),
        error = tr("Could not refresh the Pixiv session. Try again later or log in again."),
    })
end

function M.get(ctx, path, query, cursor)
    local resource = api.prepare_get(ctx, path, query, cursor)
    if not state.refresh_token then error("Log in to Pixiv to browse illustrations") end
    if ctx.time.unix() + 60 >= state.expires_at then refresh(ctx) end
    local payload, status = api.get(ctx, resource, state.access_token)
    if api.auth_error(payload, status) then
        refresh(ctx)
        payload, status = api.get(ctx, resource, state.access_token)
    end
    return api.require_ok(ctx, payload, status)
end

function M.sign_out()
    replace({})
    remember({ state = "signed_out", display_value = tr("Not signed in") })
end

return M
