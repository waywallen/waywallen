export module waywallen.probe:lua;
import :support;
import luato;
import wavsen.decode;
using namespace rstd::prelude;
using namespace rstd::literals;
namespace ww_probe
{
auto lua_error(ref<str> message) -> luato::Error {
    return luato::Error::binding(String::make(message));
}
auto to_lua(const Value& value, usize depth = usize()) -> luato::Result<luato::Value> {
    // Conversion must accept the values allowed by the JSON parser's depth budget.
    if (depth > rstd::json::ParseOptions {}.max_depth)
        return Err(lua_error("JSON nesting exceeds limit"_str));
    if (value.is_Null()) return Ok(luato::Value::Nil());
    if (value.is_Bool()) return Ok(luato::Value::Boolean(*value.as_bool()));
    if (value.is_String()) return Ok(luato::Value::String(String::make(*value.as_str())));
    if (value.is_Number()) {
        if (auto integer = value.as_i64()) return Ok(luato::Value::Integer(*integer));
        return Ok(luato::Value::Number(*value.as_f64()));
    }
    if (auto array = value.as_array()) {
        Vec<luato::Value> values;
        for (const auto& child : **array) {
            auto next = to_lua(child, depth + usize(1));
            if (next.is_err()) return Err(rstd::move(next).unwrap_err());
            values.push(rstd::move(next).unwrap());
        }
        return Ok(luato::Value::Array(luato::Array::from(rstd::move(values))));
    }
    auto table   = luato::Table::make();
    auto entries = (**value.as_object()).iter();
    while (auto entry = entries.next()) {
        auto next = to_lua(*entry->get<1>(), depth + usize(1));
        if (next.is_err()) return Err(rstd::move(next).unwrap_err());
        table.insert(entry->get<0>()->clone(), rstd::move(next).unwrap()).unwrap();
    }
    return Ok(luato::Value::Table(rstd::move(table)));
}
auto from_lua(const luato::Value& value) -> Value {
    if (value.is_Integer()) return integer(value.as_Integer().value);
    if (value.is_Number()) {
        auto number = rstd::json::Number::from_f64(value.as_Number().value);
        return number ? Value::Number(*number) : Value::Null();
    }
    if (value.is_String()) return string(value.as_String().value);
    if (value.is_Boolean()) return Value::Bool(value.as_Boolean().value);
    if (value.is_Table()) {
        Value result;
        for (const auto& entry : value.as_Table().value->entries())
            result[entry.key] = from_lua(entry.value);
        return result;
    }
    if (value.is_Array()) {
        Vec<Value> result;
        for (const auto& child : value.as_Array().value->values()) result.push(from_lua(child));
        return Value::Array(rstd::move(result));
    }
    return Value::Null();
}
struct ProbeFile {
    String                                     path;
    u64                                        size;
    rstd::io::BufReader<rstd::io::RangeReader> reader;
    Option<u64>                                position { Some(u64()) };

    auto read(u64 start, usize count, usize& read_ahead) -> luato::Result<Vec<u8>> {
        if (start > size || u64(count.to_primitive()) > size - start)
            return Err(lua_error("read range exceeds input"_str));
        Vec<u8> buffer;
        buffer.resize(count, u8());
        usize done {};
        while (done < count) {
            auto offset = start + u64(done.to_primitive());
            if (! position || *position != offset) {
                auto moved =
                    rstd::as<rstd::io::Seek>(reader).seek(rstd::io::SeekFrom::from_start(offset));
                if (moved.is_err()) {
                    position = None();
                    reader.discard_buffer();
                    return Err(lua_error("cannot seek probe input"_str));
                }
                position = Some(offset);
            }
            auto target =
                mut_ref<u8[]>::from_raw_parts(buffer.data() + done.to_primitive(), count - done);
            bool direct = false;
            if (reader.buffer().is_empty() && target.len() < reader.capacity()) {
                // Exhausting speculative read-ahead must not reject a valid logical read.
                auto refill = usize(rstd::min(u64(reader.capacity().to_primitive()),
                                              reader.get_ref().len() - reader.get_ref().position())
                                        .to_primitive());
                auto extra  = refill > target.len() ? refill - target.len() : usize();
                direct      = extra > read_ahead;
                if (! direct) read_ahead -= extra;
            }
            auto result = direct ? rstd::as<rstd::io::Read>(reader.get_mut()).read(target)
                                 : rstd::as<rstd::io::Read>(reader).read(target);
            if (result.is_err()) {
                auto error = rstd::move(result).unwrap_err();
                position   = None();
                reader.discard_buffer();
                if (error.kind() == rstd::io::ErrorKind { rstd::io::ErrorKind::Interrupted })
                    continue;
                return Err(lua_error("cannot read probe input"_str));
            }
            if (*result == usize() || *result > target.len()) {
                position = None();
                reader.discard_buffer();
                return Err(lua_error("cannot read probe input"_str));
            }
            done += *result;
            position = Some(start + u64(done.to_primitive()));
        }
        return Ok(rstd::move(buffer));
    }
};
struct ScriptRuntime {
    Value                                      config;
    Option<luato::State>                       state;
    rstd::collections::HashMap<String, String> code;
    Vec<ProbeFile>                             files;
    usize                                      bytes {};
    usize                                      read_ahead {};
    Box<Observer>*                             observe = nullptr;

    auto read(ref<str> path, i64 offset, i64 length) -> luato::Result<Vec<u8>> {
        if (! local_path(path) || offset < i64() || length < i64() || length > i64(8 * 1024 * 1024))
            return Err(lua_error("invalid read range"_str));
        auto count = usize(static_cast<rstd::size_t>(length.to_primitive()));
        if (bytes + count > usize(16 * 1024 * 1024))
            return Err(lua_error("probe read budget exceeded"_str));
        bytes += count;
        ProbeFile* file = nullptr;
        for (auto& cached : files)
            if (cached.path == path) {
                file = &cached;
                break;
            }
        if (! file) {
            (*observe)->operator()(path);
            if (files.len() >= usize(512)) return Err(lua_error("probe input limit exceeded"_str));
            auto opened = rstd::fs::File::open(ref<Path>(path));
            if (opened.is_err()) return Err(lua_error("cannot open probe input"_str));
            auto meta = opened->metadata();
            if (meta.is_err() || ! meta->is_file())
                return Err(lua_error("probe input is not a regular file"_str));
            auto size  = meta->len();
            auto range = rstd::io::ReadRange::make(
                rstd::io::SharedReadAt::make(rstd::move(opened).unwrap()), u64(), size);
            if (range.is_err()) return Err(lua_error("invalid probe input range"_str));
            files.push(ProbeFile { String::make(path),
                                   size,
                                   rstd::io::BufReader<rstd::io::RangeReader>(
                                       rstd::move(range).unwrap().into_reader()) });
            file = &files[files.len() - usize(1)];
        }
        auto start = u64(static_cast<rstd::uint64_t>(offset.to_primitive()));
        return file->read(start, count, read_ahead);
    }
    auto read_text(ref<str> path, i64 offset, i64 length) -> luato::Result<String> {
        auto data = read(path, offset, length);
        if (data.is_err()) return Err(rstd::move(data).unwrap_err());
        auto value = String::from_utf8(rstd::move(data).unwrap());
        if (value.is_err()) return Err(lua_error("probe text must be UTF-8"_str));
        return Ok(rstd::move(value).unwrap());
    }
    auto whole(ref<str> path) -> luato::Result<String> {
        if (! local_path(path)) return Err(lua_error("probe requires an absolute local path"_str));
        (*observe)->operator()(path);
        auto metadata = stamp(path);
        auto size     = metadata["size"_str].as_i64();
        if (! size) return Err(lua_error("cannot stat probe input"_str));
        return read_text(path, i64(), *size);
    }
    auto load(luato::ModuleRequest request) -> luato::Result<luato::LuaModuleSource> {
        String relative;
        if (request.requested == "__probe"_str)
            relative = String::make(text(config, "entry"_str));
        else {
            auto requested = request.requested.as_str();
            if (requested.starts_with("/"_str) || requested.contains(".."_str) ||
                requested.contains("\\"_str))
                return Err(lua_error("unsafe probe module"_str));
            if (auto local = requested.strip_prefix("./"_str)) {
                auto importer =
                    request.importer_path.as_str().strip_prefix(text(config, "root"_str));
                if (importer)
                    if (auto parent = importer->rsplit_once("/"_str))
                        relative = String::make(
                            parent->get<0>().strip_prefix("/"_str).unwrap_or(parent->get<0>()));
                if (! relative.is_empty()) relative.push_ascii('/');
                relative.push_str(*local);
            } else {
                for (auto byte : requested.as_bytes())
                    relative.push_ascii(byte == u8('.') ? u8('/') : byte);
            }
            if (! relative.as_str().ends_with(".lua"_str)) relative.push_str(".lua"_str);
        }
        bool allowed = false;
        for (const auto& file : **config["files"_str].as_array())
            if (file.as_str() == Some(relative.as_str())) allowed = true;
        if (! allowed) return Err(lua_error("probe module is not declared in files.txt"_str));
        auto path =
            rstd::path::PathBuf::from(text(config, "root"_str)).join(ref<Path>(relative.as_str()));
        auto full = path.as_path().to_string_lossy();
        (*observe)->operator()(full);
        auto resolved = rstd::fs::canonicalize(path.as_path());
        auto prefix   = String::make(text(config, "root"_str));
        prefix.push_ascii('/');
        if (resolved.is_err() ||
            ! resolved->as_path().to_string_lossy().as_str().starts_with(prefix))
            return Err(lua_error("probe module escapes plugin directory"_str));
        auto before = rstd::json::to_string(stamp(full));
        auto source = whole(full);
        if (source.is_err()) return Err(rstd::move(source).unwrap_err());
        if (source->len() > usize(1024 * 1024))
            return Err(lua_error("probe module is too large"_str));
        code.insert(full.clone(), rstd::move(before));
        return Ok(luato::LuaModuleSource { relative.clone(),
                                           rstd::format("{}:{}", text(config, "key"_str), relative),
                                           rstd::move(full),
                                           Vec<u8>::from(source->as_str().as_bytes()) });
    }
    auto create() -> luato::Result<empty> {
        auto created = luato::State::create(luato::StateOptions::restricted(u64(2000000)));
        if (created.is_err()) return Err(rstd::move(created).unwrap_err());
        state         = Some(rstd::move(created).unwrap());
        auto resolver = state->set_module_resolver(
            luato::ModuleResolverSpec::make([this](luato::ModuleRequest request) {
                return load(rstd::move(request));
            }));
        if (resolver.is_err()) return resolver;
        auto fs = luato::ModuleSpec("fs"_Str);
        fs.function("read"_Str, [this](String path) {
            return whole(path);
        });
        fs.function("read_at"_Str, [this](String path, i64 offset, i64 length) {
            return read_text(path, offset, length);
        });
        fs.function("u32_le"_Str, [this](String path, i64 offset) -> luato::Result<i64> {
            auto bytes = read(path, offset, i64(4));
            if (bytes.is_err()) return Err(rstd::move(bytes).unwrap_err());
            rstd::uint64_t value = 0;
            const auto*    data  = bytes->data();
            for (usize index {}; index < usize(4); ++index)
                value |= static_cast<rstd::uint64_t>(data[index.to_primitive()])
                         << (index.to_primitive() * 8);
            return Ok(i64(static_cast<rstd::int64_t>(value)));
        });
        fs.function("stat"_Str, [this](String path) -> luato::Result<luato::Value> {
            if (! local_path(path))
                return Err(lua_error("probe requires an absolute local path"_str));
            (*observe)->operator()(path);
            return to_lua(stamp(path));
        });
        fs.function("exists"_Str, [this](String path) -> luato::Result<bool> {
            if (! local_path(path))
                return Err(lua_error("probe requires an absolute local path"_str));
            (*observe)->operator()(path);
            return Ok(rstd::fs::metadata(ref<Path>(path.as_str())).is_ok());
        });
        fs.function("list_dir"_Str, [this](String path) -> luato::Result<luato::Array> {
            if (! local_path(path))
                return Err(lua_error("probe requires an absolute local path"_str));
            (*observe)->operator()(path);
            auto directory = rstd::fs::read_dir(ref<Path>(path.as_str()));
            if (directory.is_err()) return Err(lua_error("cannot list probe directory"_str));
            Vec<luato::Value> entries;
            while (auto entry = directory->next()) {
                if (entries.len() >= usize(8192))
                    return Err(lua_error("probe directory exceeds limit"_str));
                if (entry->is_ok())
                    entries.push(
                        luato::Value::String((*entry)->path().as_path().to_string_lossy()));
            }
            rstd::slice_::sort_by(entries.as_mut_slice().as_mut_ref(),
                                  [](const auto& a, const auto& b) {
                                      return a.as_String().value <=> b.as_String().value;
                                  });
            return Ok(luato::Array::from(rstd::move(entries)));
        });
        fs.function("parent"_Str, [](String path) {
            auto p = rstd::path::PathBuf::from(path);
            p.pop();
            return p.as_path().to_string_lossy();
        });
        fs.function("join"_Str, [](String root, String path) {
            return rstd::path::PathBuf::from(root)
                .join(ref<Path>(path.as_str()))
                .as_path()
                .to_string_lossy();
        });
        auto registered = state->register_native_require_module(
            luato::NativeRequireModuleSpec("waywallen.fs"_Str, "builtin:fs"_Str, rstd::move(fs)));
        if (registered.is_err()) return registered;
        auto json = luato::ModuleSpec("json"_Str);
        json.function("parse"_Str, [](String source) -> luato::Result<luato::Value> {
            auto parsed = rstd::json::from_str(source);
            if (parsed.is_err()) return Err(lua_error("invalid JSON"_str));
            return to_lua(*parsed);
        });
        registered = state->register_native_require_module(luato::NativeRequireModuleSpec(
            "waywallen.json"_Str, "builtin:json"_Str, rstd::move(json)));
        if (registered.is_err()) return registered;
        auto media = luato::ModuleSpec("media"_Str);
        media.function("probe"_Str, [this](String path) -> luato::Result<luato::Table> {
            if (! local_path(path))
                return Err(lua_error("probe requires an absolute local path"_str));
            (*observe)->operator()(path);
            auto result = wavsen::decode::probe_media(path);
            if (result.is_err()) return Err(lua_error(result.unwrap_err().message));
            auto table = luato::Table::make();
            table.set("width"_Str, i64(result->width.to_primitive())).unwrap();
            table.set("height"_Str, i64(result->height.to_primitive())).unwrap();
            return Ok(rstd::move(table));
        });
        registered = state->register_native_require_module(luato::NativeRequireModuleSpec(
            "waywallen.media"_Str, "builtin:media"_Str, rstd::move(media)));
        if (registered.is_err()) return registered;
        auto log = luato::ModuleSpec("log"_Str);
        log.function("info"_Str, [](String message) {
            rstd::io::eprintln("{}", message);
        });
        return state->register_native_require_module(luato::NativeRequireModuleSpec(
            "waywallen.log"_Str, "builtin:log"_Str, rstd::move(log)));
    }
    auto run(const Value& input, Box<Observer>& observer) -> luato::Result<Value> {
        observe    = &observer;
        bytes      = usize();
        read_ahead = usize(16 * 1024 * 1024);
        files.clear();
        struct Reset {
            ScriptRuntime& runtime;
            ~Reset() {
                runtime.observe = nullptr;
                runtime.files.clear();
            }
        } reset { *this };
        bool changed = false;
        auto entries = code.iter();
        while (auto file = entries.next()) {
            auto path = file->get<0>()->as_str();
            (*observe)->operator()(path);
            if (*file->get<1>() != rstd::json::to_string(stamp(path))) changed = true;
        }
        if (changed) {
            state = None();
            code.clear();
        }
        if (! state) {
            auto result = create();
            if (result.is_err()) return Err(rstd::move(result).unwrap_err());
        }
        auto item = to_lua(input);
        if (item.is_err()) return Err(rstd::move(item).unwrap_err());
        Vec<luato::Value> arguments;
        arguments.push(rstd::move(item).unwrap());
        auto result = state->call_module(
            "__probe"_str, "probe"_str, luato::Array::from(rstd::move(arguments)));
        if (result.is_err()) return Err(rstd::move(result).unwrap_err());
        if (result->is_none()) return Ok(Value::Null());
        if (! (**result).is_Table()) return Err(lua_error("probe must return a table or nil"_str));
        return Ok(from_lua(**result));
    }
};
} // namespace ww_probe
export namespace ww_probe
{
class LuaPrograms {
    Vec<Box<ScriptRuntime>> programs;

public:
    bool configure(const Value& input) {
        auto entries = input["programs"_str].as_array();
        if (! entries || (*entries)->len() > usize(256)) return false;
        for (const auto& entry : **entries) {
            if (entry["version"_str].as_i64() != Some(i64(1)) ||
                text(entry, "key"_str).is_empty() ||
                ! text(entry, "root"_str).starts_with("/"_str) || ! entry["files"_str].is_Array())
                return false;
            auto runtime    = Box<ScriptRuntime>::make();
            runtime->config = entry.clone();
            programs.push(rstd::move(runtime));
        }
        return true;
    }
    auto run(const Value& input, Box<Observer>& observer) -> luato::Result<Value> {
        auto kind = text(input, "kind"_str);
        for (auto& program : programs) {
            auto prefix = rstd::format("lua:{}:", text(program->config, "key"_str));
            if (kind.starts_with(prefix)) return program->run(input, observer);
        }
        return Err(lua_error("probe provider is not configured"_str));
    }
};
} // namespace ww_probe
