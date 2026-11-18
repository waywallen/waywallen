local M = {}

M.origin = "https://app-api.pixiv.net"
M.redirect = "https://app-api.pixiv.net/web/v1/users/auth/pixiv/callback"
M.custom_redirect = "pixiv://account/login"

local OAUTH = "https://oauth.secure.pixiv.net/auth/token"
local CLIENT_ID = "MOBrBDS8blbauoSck0ZfDbtuzpyT"
local CLIENT_SECRET = "lsACyCD94FhDUtGTXi3QzcFE2uU1hqtDaKeqrdwj"
local HASH_SALT = "28c1fdd170a5204386cb1313c7077b34f83e4aaf4aa829ce78c231e05b0bae2c"
local cooldown_until = 0

local function headers(ctx, token)
    local date = ctx.time.utc_iso8601()
    local result = {
        ["User-Agent"] = "PixivAndroidApp/5.0.166 (Android 10.0; Pixel C)",
        ["App-OS"] = "Android",
        ["App-OS-Version"] = "10.0",
        ["App-Version"] = "5.0.166",
        ["X-Client-Time"] = date,
        ["X-Client-Hash"] = ctx.crypto.md5_hex(date .. HASH_SALT),
    }
    if token then result.Authorization = "Bearer " .. token end
    return result
end

local function request(ctx, method, url, token, parameters, options)
    if ctx.time.unix() < cooldown_until then
        error("Pixiv rate limit; try again later")
    end
    options = options or {}
    local http = options.http or ctx.http
    local timeout = options.timeout or 7
    local sent, response = pcall(function()
        local builder = method == "POST" and http:post(url) or http:get(url)
        builder = builder:headers(headers(ctx, token)):timeout(timeout):response_limit(4 * 1024 * 1024)
        if parameters then
            builder = method == "POST" and builder:form(parameters) or builder:query(parameters)
        end
        return builder:send()
    end)
    if not sent then error("Pixiv request failed; try again later") end
    local status = response:status()
    if status == 429 then
        cooldown_until = ctx.time.unix() + 30
        response:close()
        error("Pixiv rate limit; try again later")
    end
    local read, text = pcall(function()
        return response:text({ strict_utf8 = true, timeout_ms = math.max(1, math.floor(timeout * 1000)) })
    end)
    response:close()
    if not read then error("Pixiv response could not be read") end
    local decoded, payload = pcall(ctx.json.strict.decode, text)
    if not decoded or ctx.json.strict.kind(payload) ~= "object" then
        error("Pixiv returned invalid JSON")
    end
    return payload, status
end

function M.login_url(ctx, challenge)
    return M.origin .. "/web/v1/login?" .. ctx.url.encode_query_pairs({
        { "code_challenge", challenge },
        { "code_challenge_method", "S256" },
        { "client", "pixiv-android" },
    })
end

function M.token(ctx, grant, options)
    local parameters = {
        client_id = CLIENT_ID,
        client_secret = CLIENT_SECRET,
        grant_type = grant.grant_type,
        include_policy = "true",
    }
    if grant.grant_type == "authorization_code" then
        parameters.code = grant.code
        parameters.code_verifier = grant.verifier
        parameters.redirect_uri = options and options.redirect_uri or M.redirect
    elseif grant.grant_type == "refresh_token" then
        parameters.refresh_token = grant.refresh_token
    else
        error("Unsupported Pixiv token grant")
    end
    return request(ctx, "POST", options and options.url or OAUTH, nil, parameters, options)
end

function M.idp_urls(ctx, options)
    local payload, status = request(ctx, "GET", M.origin .. "/idp-urls", nil, nil, options)
    return M.require_ok(ctx, payload, status)
end

function M.prepare_get(ctx, path, query, cursor)
    if cursor ~= nil and cursor ~= "" then
        local address = M.cursor(ctx, cursor, path)
        if not address then error("Pixiv returned an invalid next page") end
        return { url = address }
    end
    return { url = M.origin .. path, query = query }
end

function M.get(ctx, resource, token)
    return request(ctx, "GET", resource.url, token, resource.query)
end

function M.auth_error(payload, status)
    if status == 401 then return true end
    if status ~= 400 or type(payload.error) ~= "table" then return false end
    local message = payload.error.message
    return type(message) == "string" and message:lower():find("oauth", 1, true) ~= nil
end

function M.require_ok(ctx, payload, status)
    if status == 404 then error("Pixiv illustration was not found") end
    if status == 401 then error("Pixiv session expired; log in again") end
    if status == 403 then error("Your Pixiv account cannot access this illustration") end
    if status < 200 or status >= 300 then error("Pixiv HTTP error " .. tostring(status)) end
    if payload.error ~= nil and payload.error ~= false and payload.error ~= ctx.json.strict.null then
        error("Pixiv rejected this request")
    end
    return payload
end

function M.optional_id(value)
    if type(value) == "number" and value > 0 and value == math.floor(value) and value <= 9007199254740991 then
        value = string.format("%.0f", value)
    end
    if type(value) ~= "string" or #value == 0 or #value > 20 or not value:match("^[1-9]%d*$") then
        return nil
    end
    return value
end

function M.id(value)
    local id = M.optional_id(value)
    if not id then error("Invalid Pixiv identifier") end
    return id
end

function M.cursor(ctx, value, path, current)
    if value == nil or value == ctx.json.strict.null then return nil end
    if type(value) ~= "string" or #value == 0 or #value > 4096 or value:find("[%c]") then
        error("Pixiv returned an invalid next page")
    end
    local parsed = ctx.url.parse(value)
    if not parsed or parsed.scheme ~= "https" or parsed.host ~= "app-api.pixiv.net"
        or parsed.port ~= 443 or parsed.path ~= path or parsed.fragment ~= nil
        or value:sub(1, #M.origin + 1) ~= M.origin .. "/" then
        error("Pixiv returned an invalid next page")
    end
    if value == current then error("Pixiv repeated the current page") end
    return value
end

return M
