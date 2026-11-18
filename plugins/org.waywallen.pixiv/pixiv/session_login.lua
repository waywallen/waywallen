local api = import("pixiv.api")
local M = {}

local WEB = "https://www.pixiv.net"
local ACCOUNTS = "https://accounts.pixiv.net"
local OAUTH = "https://oauth.secure.pixiv.net"
local redirects = { [301] = true, [302] = true, [303] = true, [307] = true, [308] = true }
local navigation = {
    ["accounts.pixiv.net"] = {
        ["/login"] = true, ["/account-selected"] = true, ["/post-redirect"] = true,
        ["/security-setting/reminder"] = true,
    },
    ["app-api.pixiv.net"] = {
        ["/web/v1/login"] = true, ["/web/v1/users/auth/pixiv/start"] = true,
        ["/web/v1/users/auth/pixiv/callback"] = true,
    },
    ["oauth.secure.pixiv.net"] = { ["/auth/authorize"] = true },
}

local function invalid()
    error("Pixiv returned invalid authorization data", 2)
end

local function object(ctx, value)
    return type(value) == "table" and ctx.json.strict.kind(value) == "object"
end

local function decode(ctx, text)
    if type(text) ~= "string" then invalid() end
    local ok, value = pcall(ctx.json.strict.decode, text)
    if not ok or not object(ctx, value) then invalid() end
    return value
end

local function opaque(value)
    if type(value) ~= "string" or #value == 0 or #value > 8192 or value:find("[%c%s]") then invalid() end
    return value
end

local function cookie(value)
    if type(value) ~= "string" or #value == 0 or #value > 8192
        or value:find('[%c%s",;\\]') or value:find("[^!-~]") or value:lower():match("^phpsessid=") then
        error("Enter only the PHPSESSID value from your Pixiv browser session", 0)
    end
    return value
end

local function trusted(ctx, value, base, paths)
    if type(value) ~= "string" or #value == 0 or #value > 32768 or value:find("[%c%s]") then invalid() end
    local address = base and ctx.url.join(base, value) or ctx.url.join(value, value)
    local parsed = address and ctx.url.parse(address)
    local allowed = parsed and (paths or navigation)[parsed.host]
    if not parsed or parsed.scheme ~= "https" or parsed.port ~= 443 or parsed.fragment ~= nil
        or not allowed or not allowed[parsed.path] then invalid() end
    local origin = "https://" .. parsed.host
    if address:sub(1, #origin + 1) ~= origin .. "/" then invalid() end
    return { url = address, origin = origin, path = parsed.path }
end

local function callback_code(ctx, value)
    if type(value) ~= "string" or #value > 32768 then invalid() end
    local address = ctx.url.join(value, value)
    local parsed = address and ctx.url.parse(address)
    if not parsed or parsed.scheme ~= "pixiv" then return nil end
    if parsed.host ~= "account" or parsed.path ~= "/login" or parsed.port ~= nil
        or parsed.fragment ~= nil or address:sub(1, 21) ~= "pixiv://account/login" then invalid() end
    local code, via, counts = nil, nil, {}
    for _, pair in ipairs(ctx.url.decode_query_pairs(parsed.query or "")) do
        counts[pair[1]] = (counts[pair[1]] or 0) + 1
        if pair[1] == "code" then code = pair[2] end
        if pair[1] == "via" then via = pair[2] end
    end
    if counts.code ~= 1 or (counts.via or 0) > 1 or (via ~= nil and via ~= "login") then invalid() end
    return opaque(code)
end

function M.parse_login_page(ctx, html)
    local result = {}
    for _, element in ipairs(ctx.html.query(html, "#app-mount-point, #init-config")) do
        local attrs = element.attrs
        if attrs.id == "app-mount-point" then
            if result.props then invalid() end
            result.name = attrs["data-page-name"]
            result.props = decode(ctx, attrs["data-props"] or "{}")
        elseif attrs.id == "init-config" then
            if result.config then invalid() end
            result.config = decode(ctx, attrs.value or "{}")
        end
    end
    return result
end

function M.parse_account(ctx, html)
    local account
    for _, element in ipairs(ctx.html.query(html, "meta[name='global-data'], meta#meta-global-data, script#__NEXT_DATA__")) do
        local candidate
        if element.tag == "meta" then
            candidate = decode(ctx, element.attrs.content).userData
        else
            local next_data = decode(ctx, element.text)
            local props = object(ctx, next_data.props) and next_data.props
            local page = props and object(ctx, props.pageProps) and props.pageProps
            local state = page and decode(ctx, page.serverSerializedPreloadedState)
            candidate = state and object(ctx, state.userData) and state.userData.self
        end
        if not object(ctx, candidate) or type(candidate.name) ~= "string" or #candidate.name > 512 then
            error("The Pixiv session is not signed in. Obtain a fresh session ID", 0)
        end
        local id = api.id(candidate.id)
        if account and account ~= id then invalid() end
        account = id
    end
    if not account then error("The Pixiv session is not signed in. Obtain a fresh session ID", 0) end
    return account
end

local function account_matches(config, expected)
    if type(config) ~= "table" or api.id(config["pixivAccount.userId"]) ~= expected then
        error("The Pixiv account changed during authorization. Start again", 0)
    end
end

local function browser_headers(extra)
    local result = {
        ["User-Agent"] = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/130.0.0.0 Safari/537.36",
        ["Referer"] = WEB .. "/", ["Accept-Language"] = "zh-CN,zh;q=0.9",
        ["Cache-Control"] = "no-store, no-cache", ["Pragma"] = "no-cache",
    }
    for key, value in pairs(extra or {}) do result[key] = value end
    return result
end

local function interaction()
    error("This Pixiv session needs browser interaction. Use browser sign-in or obtain a fresh session ID", 0)
end

local function confirm_reminder(ctx, page, account, read)
    if page.name ~= "SecuritySettingReminderPage" then interaction() end
    account_matches(page.config, account)
    local props = page.props
    if not object(ctx, props) or not object(ctx, props.statuses) then invalid() end
    -- Pixiv hides the confirmation button while any setting is critical.
    local confirmable = { perfect = true, hint = true, recommended = true, warning = true }
    for _, status in pairs(props.statuses) do
        if object(ctx, status) and status.type == "critical" then
            error("Pixiv requires a security settings review. Complete it in your browser, then try again", 0)
        end
        if not object(ctx, status) or not confirmable[status.type] then interaction() end
    end
    local target = trusted(ctx, props.returnTo, page.target.url,
        { ["accounts.pixiv.net"] = { ["/post-redirect"] = true } })
    if type(props.ref) ~= "string" or #props.ref > 8192 or props.ref:find("[%c]") then invalid() end
    local response = read(ACCOUNTS .. "/ajax/security-setting/reminder/finish", "POST", browser_headers({
        Origin = ACCOUNTS, Referer = page.target.url, Accept = "application/json",
        ["Content-Type"] = "application/x-www-form-urlencoded",
    }), { action = "confirmed", ref = props.ref, tt = opaque(page.config["pixivAccount.tt"]) })
    if response.status ~= 200 then interaction() end
    local payload = decode(ctx, response.body)
    if payload.error ~= false
        or (payload.reauthenticationRequired ~= nil and payload.reauthenticationRequired ~= false) then interaction() end
    local body = payload.body
    if object(ctx, body) then
        local errors = body.errors
        if errors ~= nil and errors ~= ctx.json.strict.null then
            if not object(ctx, errors) then invalid() end
            if next(errors) ~= nil then interaction() end
        end
    elseif type(body) ~= "table" or ctx.json.strict.kind(body) ~= "array" or next(body) ~= nil then
        invalid()
    end
    return target.url
end

local function authorize(ctx, web, token_http, remaining)
    local function read(address, method, headers, form)
        local timeout = remaining()
        local sent, response = pcall(function()
            local builder = method == "POST" and web:post(address) or web:get(address)
            builder = builder:headers(headers):timeout(timeout):response_limit(4 * 1024 * 1024)
            if form ~= nil then builder = builder:form(form) end
            return builder:send()
        end)
        if not sent then
            remaining()
            error("Pixiv session sign-in request failed. Check your network and try again", 0)
        end
        local status = response:status()
        local locations = response:header_values("location")
        if redirects[status] then
            response:close()
            if #locations ~= 1 then invalid() end
            return { status = status, location = locations[1] }
        end
        local ok, body = pcall(function()
            return response:text({ strict_utf8 = true, timeout_ms = math.max(1, math.floor(remaining() * 1000)) })
        end)
        response:close()
        remaining()
        if not ok then error("Pixiv authorization response could not be read", 0) end
        return { status = status, body = body }
    end
    local function navigate(address, method, headers, form)
        local target = trusted(ctx, address)
        method, headers = method or "GET", browser_headers(headers)
        for _ = 1, 8 do
            local result = read(target.url, method, headers, form)
            if redirects[result.status] then
                local code = callback_code(ctx, result.location)
                if code then
                    if target.origin ~= api.origin or target.path ~= "/web/v1/users/auth/pixiv/callback" then invalid() end
                    return { code = code }
                end
                target = trusted(ctx, result.location, target.url)
                if result.status == 301 or result.status == 302 or result.status == 303 then
                    method, form, headers = "GET", nil, browser_headers()
                end
            else
                if result.status ~= 200 then error("Pixiv rejected the session. Obtain a fresh session ID", 0) end
                local page = M.parse_login_page(ctx, result.body)
                page.target = target
                return page
            end
        end
        error("Pixiv authorization exceeded the redirect limit", 0)
    end
    local home = read(WEB .. "/", "GET", browser_headers())
    if home.status ~= 200 then error("Pixiv rejected the session. Obtain a fresh session ID", 0) end
    local account = M.parse_account(ctx, home.body)
    local verifier = ctx.random.hex(32)
    local selection = navigate(api.login_url(ctx, ctx.crypto.sha256_base64url(verifier)))
    if selection.code or selection.name ~= "SelectAccountPage" or selection.target.origin ~= ACCOUNTS
        or selection.target.path ~= "/login" then interaction() end
    account_matches(selection.config, account)
    if not object(ctx, selection.props) then invalid() end
    local return_to = trusted(ctx, selection.props.continueWithCurrentAccountUrl, selection.target.url)
    local form_headers = { Origin = ACCOUNTS, Referer = selection.target.url,
        ["Content-Type"] = "application/x-www-form-urlencoded" }
    local confirmation = navigate(ACCOUNTS .. "/account-selected", "POST", form_headers,
        { return_to = return_to.url, tt = opaque(selection.config["pixivAccount.tt"]) })
    if not confirmation.code and confirmation.target.origin == ACCOUNTS
        and confirmation.target.path == "/security-setting/reminder" then
        confirmation = navigate(confirm_reminder(ctx, confirmation, account, read))
    end
    if confirmation.code or confirmation.target.origin ~= ACCOUNTS or confirmation.target.path ~= "/post-redirect" then interaction() end
    account_matches(confirmation.config, account)
    local start = trusted(ctx, confirmation.config["pixivAccount.returnTo"], confirmation.target.url,
        { ["app-api.pixiv.net"] = { ["/web/v1/users/auth/pixiv/start"] = true } })
    form_headers.Referer = ACCOUNTS .. "/"
    local callback = navigate(start.url, "POST", form_headers, {})
    if not callback.code then interaction() end
    local idp = api.idp_urls(ctx, { http = token_http, timeout = remaining() })
    local endpoint = trusted(ctx, idp["auth-token"], nil,
        { ["oauth.secure.pixiv.net"] = { ["/auth/token"] = true } })
    local redirect = trusted(ctx, idp["auth-token-redirect-uri"], nil,
        { ["app-api.pixiv.net"] = { ["/web/v1/users/auth/pixiv/callback"] = true } })
    local final_home = read(WEB .. "/", "GET", browser_headers())
    if final_home.status ~= 200 or M.parse_account(ctx, final_home.body) ~= account then
        error("The Pixiv account changed during authorization. Start again", 0)
    end
    cookie(web:cookie(WEB .. "/", "PHPSESSID"))
    local exchanged, payload, status = pcall(api.token, ctx, {
        grant_type = "authorization_code", code = callback.code, verifier = verifier,
    }, { http = token_http, timeout = remaining(), url = endpoint.url, redirect_uri = redirect.url })
    verifier = nil
    if not exchanged then error("Pixiv authorization could not be confirmed. Start again", 0) end
    remaining()
    if status < 200 or status >= 300 then error("Pixiv rejected the authorization. Start again", 0) end
    return { payload = payload, account_id = account }
end

function M.authorize(ctx, value)
    cookie(value)
    local web, token_http
    local deadline = ctx.time.now() + 22000
    local function remaining()
        local millis = deadline - ctx.time.now()
        if millis <= 0 then error("Pixiv session sign-in timed out. Try again", 0) end
        return math.min(7, millis / 1000)
    end
    local ok, result = pcall(function()
        web, token_http = ctx.new_http_session(), ctx.new_http_session()
        web:set_cookie(WEB .. "/", "PHPSESSID=" .. value .. "; Domain=pixiv.net; Path=/; Secure; HttpOnly")
        return authorize(ctx, web, token_http, remaining)
    end)
    value = nil
    if web then pcall(function() web:clear_cookies() end) end
    if token_http then pcall(function() token_http:clear_cookies() end) end
    if not ok then error(result, 0) end
    return result
end

return M
