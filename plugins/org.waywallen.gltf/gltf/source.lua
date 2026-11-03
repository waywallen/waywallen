local M = {}

function M.scan(ctx)
    local entries, seen, sizes = {}, {}, {}
    for _, root in ipairs(ctx.libraries()) do
        local function walk(dir, depth)
            for _, path in ipairs(ctx.fs.list_files(dir)) do
                local ext = (ctx.extension(path) or ""):lower()
                if ext == "gltf" or ext == "glb" then
                    local canonical = ctx.fs.canonicalize(path)
                    if canonical and not seen[canonical] and ctx.fs.is_file(path) then
                        seen[canonical] = true
                        local filename = ctx.filename(path) or path
                        local directory = ctx.fs.parent(canonical)
                        local size = sizes[directory]
                        if size == nil then
                            size = ctx.fs.directory_size(directory)
                            sizes[directory] = size or false
                        end
                        entries[#entries + 1] = {
                            name = filename:match("(.+)%.[^.]+$") or filename,
                            wp_type = "gltf",
                            resource = path,
                            library_root = root,
                            size = size ~= false and size or nil,
                            can_remove = false,
                        }
                    end
                end
            end
            if depth > 0 then
                local dirs = ctx.list_dirs(dir)
                table.sort(dirs)
                for _, child in ipairs(dirs) do walk(child, depth - 1) end
            end
        end
        walk(root, 2)
    end
    ctx.log("gltf: found " .. #entries .. " model wallpapers")
    return entries
end

return M
