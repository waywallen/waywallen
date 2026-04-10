local M = {}

function M.info()
    return {
        name = "wallpaper_engine",
        types = {"scene"},
        version = "0.1.0",
    }
end

function M.scan(ctx)
    local entries = {}

    local workshop_dir = ctx.config("workshop_dir")
        or ctx.env("WAYWALLEN_WORKSHOP_DIR")

    if not workshop_dir then
        local home = ctx.env("HOME") or ""
        -- Try common Steam library paths
        local candidates = {
            home .. "/.steam/steam/steamapps/workshop/content/431960",
            home .. "/.local/share/Steam/steamapps/workshop/content/431960",
        }
        for _, path in ipairs(candidates) do
            if ctx.file_exists(path) then
                workshop_dir = path
                break
            end
        end
    end

    if not workshop_dir or not ctx.file_exists(workshop_dir) then
        ctx.log("wallpaper_engine: workshop dir not found")
        return entries
    end

    local dirs = ctx.list_dirs(workshop_dir)
    for _, dir in ipairs(dirs) do
        local pkg_path = dir .. "/scene.pkg"
        local json_path = dir .. "/scene.json"

        -- A valid WE scene has either scene.pkg or scene.json
        if ctx.file_exists(pkg_path) or ctx.file_exists(json_path) then
            local workshop_id = ctx.basename(dir) or dir
            local name = "Workshop " .. workshop_id

            -- Try to read project.json for the real title
            local project_path = dir .. "/project.json"
            if ctx.file_exists(project_path) then
                local content = ctx.read_file(project_path)
                if content then
                    local project = ctx.json_parse(content)
                    if project and project.title then
                        name = project.title
                    end
                end
            end

            local resource = pkg_path
            if not ctx.file_exists(pkg_path) then
                resource = json_path
            end

            -- Look for preview image
            local preview = nil
            local preview_candidates = {
                dir .. "/preview.jpg",
                dir .. "/preview.png",
                dir .. "/preview.gif",
            }
            for _, p in ipairs(preview_candidates) do
                if ctx.file_exists(p) then
                    preview = p
                    break
                end
            end

            table.insert(entries, {
                id = workshop_id,
                name = name,
                wp_type = "scene",
                resource = resource,
                preview = preview,
                metadata = {
                    scene = resource,
                    assets = workshop_dir,
                    workshop_id = workshop_id,
                },
            })
        end
    end

    ctx.log("wallpaper_engine: found " .. #entries .. " scenes in " .. workshop_dir)
    return entries
end

return M
