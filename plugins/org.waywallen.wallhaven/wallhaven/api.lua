local M = {}
local session = import("wallhaven.session")

local API = "https://wallhaven.cc/api/v1"

local SORTS = {
    trend = { sorting = "toplist", topRange = "1M" },
    recent = { sorting = "date_added" },
    popular = { sorting = "favorites" },
}

local RESOLUTION_TAGS = {
    ["1280 x 720"] = "1280x720",
    ["1920 x 1080"] = "1920x1080",
    ["2560 x 1440"] = "2560x1440",
    ["3840 x 2160"] = "3840x2160",
    ["2560 x 1080"] = "2560x1080",
    ["3440 x 1440"] = "3440x1440",
    ["5120 x 1440"] = "5120x1440",
    ["3840 x 1080"] = "3840x1080",
    ["7680 x 2160"] = "7680x2160",
    ["1080 x 1920"] = "1080x1920",
    ["720 x 1280"] = "720x1280",
    ["1440 x 2560"] = "1440x2560",
}

local QUERY_TAGS = {
    Abstract = "abstract",
    Animal = "animal",
    Anime = "anime",
    Cartoon = "cartoon",
    CG = "cg",
    Cyberpunk = "cyberpunk",
    Fantasy = "fantasy",
    Game = "game",
    Girls = "girl",
    Guys = "guy",
    Landscape = "landscape",
    Medieval = "medieval",
    Memes = "meme",
    MMD = "mmd",
    Music = "music",
    Nature = "nature",
    ["Pixel art"] = "pixel art",
    Relaxing = "relaxing",
    Retro = "retro",
    ["Sci-Fi"] = "sci-fi",
    Sports = "sports",
    Technology = "technology",
    Vehicle = "vehicle",
}

M.filters = {
    {
        id = "purity",
        title = tr("Purity"),
        type = "multi_select",
        description = tr("NSFW wallpapers require a Wallhaven API key."),
        options = {
            { value = "SFW", label = tr("Safe for work") },
            { value = "Sketchy", label = tr("Questionable") },
            { value = "NSFW", label = tr("Not safe for work") },
        },
    },
    {
        id = "topics",
        title = tr("Topics"),
        type = "multi_select",
        match_modes = { "any", "all" },
        default_match_mode = "any",
        options = {
            { value = "Abstract", label = tr("Abstract") },
            { value = "Animal", label = tr("Animal") },
            { value = "Anime", label = tr("Anime") },
            { value = "Cartoon", label = tr("Cartoon") },
            { value = "CG", label = tr("CG") },
            { value = "Cyberpunk", label = tr("Cyberpunk") },
            { value = "Fantasy", label = tr("Fantasy") },
            { value = "Game", label = tr("Game") },
            { value = "Girls", label = tr("Girls") },
            { value = "Guys", label = tr("Guys") },
            { value = "Landscape", label = tr("Landscape") },
            { value = "Medieval", label = tr("Medieval") },
            { value = "Memes", label = tr("Memes") },
            { value = "MMD", label = tr("MMD") },
            { value = "Music", label = tr("Music") },
            { value = "Nature", label = tr("Nature") },
            { value = "Pixel art", label = tr("Pixel art") },
            { value = "Relaxing", label = tr("Relaxing") },
            { value = "Retro", label = tr("Retro") },
            { value = "Sci-Fi", label = tr("Sci-Fi") },
            { value = "Sports", label = tr("Sports") },
            { value = "Technology", label = tr("Technology") },
            { value = "Vehicle", label = tr("Vehicle") },
        },
    },
    {
        id = "resolution",
        title = tr("Resolution"),
        type = "multi_select",
        options = {
            { value = "1280 x 720", label = tr("1280 x 720") },
            { value = "1920 x 1080", label = tr("1920 x 1080") },
            { value = "2560 x 1440", label = tr("2560 x 1440") },
            { value = "3840 x 2160", label = tr("3840 x 2160") },
            { value = "2560 x 1080", label = tr("2560 x 1080") },
            { value = "3440 x 1440", label = tr("3440 x 1440") },
            { value = "5120 x 1440", label = tr("5120 x 1440") },
            { value = "3840 x 1080", label = tr("3840 x 1080") },
            { value = "7680 x 2160", label = tr("7680 x 2160") },
            { value = "1080 x 1920", label = tr("1080 x 1920") },
            { value = "720 x 1280", label = tr("720 x 1280") },
            { value = "1440 x 2560", label = tr("1440 x 2560") },
        },
    },
}

local function append_query(query, term)
    if not term or term == "" then
        return query
    end
    if query == "" then
        return term
    end
    return query .. " " .. term
end

local function request_json(ctx, url, query)
    local rsp = ctx.http:get(url)
        :headers(session.headers())
        :query(query)
        :timeout(20)
        :send()
    if rsp:status() == 401 or rsp:status() == 403 then
        session.sign_out()
        error("Wallhaven API key was rejected; log in again")
    end
    if not rsp:ok() then
        error("wallhaven http " .. tostring(rsp:status()))
    end
    return rsp:json()
end

function M.search(ctx, params)
    local sort = SORTS[params.sort] or SORTS.trend
    local query = params.query or ""
    local resolutions = {}
    local general, anime, people = true, true, true
    local category_selected = false
    local sfw, sketchy, nsfw = false, false, false
    local purity_selected = false
    local topic_mode = params.filter_match_modes and params.filter_match_modes.topics
    local topics = {}

    for _, tag in ipairs(params.tags or {}) do
        if tag == "SFW" then
            sfw = true
            purity_selected = true
        elseif tag == "Sketchy" then
            sketchy = true
            purity_selected = true
        elseif tag == "NSFW" then
            nsfw = true
            purity_selected = true
        elseif not topic_mode and tag == "Anime" then
            if not category_selected then
                general, anime, people = false, false, false
                category_selected = true
            end
            anime = true
        elseif not topic_mode and (tag == "Girls" or tag == "Guys") then
            if not category_selected then
                general, anime, people = false, false, false
                category_selected = true
            end
            people = true
        end
        local resolution = RESOLUTION_TAGS[tag]
        if resolution then
            table.insert(resolutions, resolution)
        end
        local topic = QUERY_TAGS[tag]
        if topic and topic_mode then
            if topic:find("%s") then
                topic = '"' .. topic:gsub('\\', '\\\\'):gsub('"', '\\"') .. '"'
            end
            table.insert(topics, "+" .. topic)
        else
            query = append_query(query, topic)
        end
    end

    if #topics > 0 then
        if query:match("%f[%w]id:%d+") then
            error("Wallhaven exact tag searches cannot be combined with Topics")
        end
        local expression = "(" .. table.concat(topics, topic_mode == "any" and " | " or " ") .. ")"
        query = query == "" and expression or "(" .. query .. ") " .. expression
    end

    if not purity_selected then
        sfw = true
    end
    if nsfw and not session.authenticated() then
        error("Log in to Wallhaven with an API key to browse NSFW wallpapers")
    end

    local q = {
        q = query,
        categories = (general and "1" or "0") .. (anime and "1" or "0") .. (people and "1" or "0"),
        purity = (sfw and "1" or "0") .. (sketchy and "1" or "0") .. (nsfw and "1" or "0"),
        sorting = sort.sorting,
        order = "desc",
        page = tostring(params.page or 1),
    }
    if sort.topRange then
        q.topRange = sort.topRange
    end
    if #resolutions > 0 then
        q.resolutions = table.concat(resolutions, ",")
    end

    return request_json(ctx, API .. "/search", q)
end

function M.wallpaper(ctx, id)
    local payload = request_json(ctx, API .. "/w/" .. tostring(id), {})
    return payload.data or {}
end

return M
