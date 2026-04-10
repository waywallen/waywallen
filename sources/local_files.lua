local M = {}

function M.info()
    return {
        name = "local_files",
        types = {"image", "video", "gif"},
        version = "0.1.0",
    }
end

function M.scan(ctx)
    local entries = {}

    local wallpaper_dir = ctx.config("wallpaper_dir")
        or (ctx.env("HOME") .. "/Pictures/Wallpapers")

    if not ctx.file_exists(wallpaper_dir) then
        ctx.log("local_files: wallpaper dir not found: " .. wallpaper_dir)
        return entries
    end

    local image_exts = {png = true, jpg = true, jpeg = true, bmp = true, webp = true, tiff = true}
    local video_exts = {mp4 = true, webm = true, mkv = true, avi = true}
    local gif_exts   = {gif = true}

    local files = ctx.glob(wallpaper_dir .. "/**/*.*")
    for _, path in ipairs(files) do
        local ext = ctx.extension(path)
        if ext then
            ext = string.lower(ext)
            local wp_type = nil
            if image_exts[ext] then wp_type = "image"
            elseif video_exts[ext] then wp_type = "video"
            elseif gif_exts[ext] then wp_type = "gif"
            end

            if wp_type then
                table.insert(entries, {
                    id = path,
                    name = ctx.filename(path) or path,
                    wp_type = wp_type,
                    resource = path,
                    preview = nil,
                    metadata = {},
                })
            end
        end
    end

    ctx.log("local_files: found " .. #entries .. " wallpapers in " .. wallpaper_dir)
    return entries
end

return M
