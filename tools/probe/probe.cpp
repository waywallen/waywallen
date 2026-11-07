module waywallen.probe;
import :support;
import :lua;
import luato;
import wavsen.decode;
import waywallen.image.definition;
using namespace rstd::prelude;
using namespace rstd::literals;
using namespace ww_probe;

namespace waywallen::probe
{
bool request(const Value& input, LuaPrograms& scripts) {
    auto id = text(input, "request_id"_str), path = text(input, "path"_str),
         kind   = text(input, "kind"_str);
    bool custom = kind.starts_with("lua:"_str);
    if (input["selection_version"_str].as_i64() != Some(i64(1)) || id.is_empty() ||
        ! local_path(path) || (! custom && kind != "media"_str && kind != "timeline"_str))
        return false;
    String      selected = String::make(path), error;
    String      status   = "ok"_Str;
    Vec<String> dependencies;
    dependencies.push(String::make(path));
    Vec<Value> observed;
    observed.push(stamp(path));
    bool output_ok = true, tracking_error = false;
    auto resolved = [&] {
        Value event;
        event["event"_str]         = string("resolved"_str);
        event["request_id"_str]    = string(id);
        event["selected_path"_str] = string(selected);
        event["dependencies"_str] =
            custom ? Value::Array(observed.clone()) : snapshots(dependencies);
        if (! output(event)) output_ok = false;
    };
    Value result;
    if (custom) {
        resolved();
        auto observer = Box<Observer>::make([&](ref<str> dependency) {
            if (! local_path(dependency)) {
                tracking_error = true;
                return;
            }
            for (const auto& known : dependencies)
                if (known == dependency) return;
            usize path_bytes = dependency.len();
            for (const auto& path : dependencies) path_bytes += path.len();
            if (dependencies.len() >= usize(512) || path_bytes > usize(512 * 1024)) {
                tracking_error = true;
                return;
            }
            dependencies.push(String::make(dependency));
            observed.push(stamp(dependency));
            resolved();
        });
        auto value    = scripts.run(input, observer);
        if (value.is_err()) {
            auto failure = rstd::move(value).unwrap_err();
            error = rstd::format("{}: {} {}", failure.source, failure.message, failure.traceback);
        } else if (value->is_Null())
            status = "unsupported"_Str;
        else {
            auto requested = text(*value, "status"_str);
            if (! requested.is_empty()) status = String::make(requested);
            if (status != "ok"_str && status != "unsupported"_str && status != "failed"_str)
                error = "invalid Lua probe status"_Str;
            const char* fields[] = { "width", "height", "size" };
            for (const char* field : fields) {
                auto name = rstd::ffi::CStr::from_ptr(field).to_str().unwrap();
                if ((*value)[name].is_Null()) continue;
                auto number = (*value)[name].as_i64();
                if (! number) {
                    auto real = (*value)[name].as_f64();
                    if (real && real->is_finite() && *real >= f64() &&
                        *real <= f64(9007199254740991.0) && real->floor() == *real)
                        number = Some(i64(static_cast<rstd::int64_t>(real->to_primitive())));
                }
                if (! number || *number < (name == "size"_str ? i64() : i64(1)) ||
                    (name != "size"_str && *number > i64(2147483647)))
                    error = "invalid Lua probe metadata"_Str;
                else
                    result[name] = integer(*number);
            }
            if (result["width"_str].is_Null() != result["height"_str].is_Null() ||
                (status == "ok"_str && result["width"_str].is_Null() &&
                 result["size"_str].is_Null()))
                error = "Lua probe returned no valid metadata"_Str;
            if (! text(*value, "error"_str).is_empty())
                error = String::make(text(*value, "error"_str));
        }
        if (tracking_error) error = "probe dependency limit exceeded"_Str;
    } else {
        if (kind == "timeline"_str) {
            auto definition = ww_image::ImageDefinition::open(
                path, error, &dependencies, { &observed, [](void* context, ref<str> input) {
                                                 static_cast<Vec<Value>*>(context)->push(
                                                     stamp(input));
                                             } });
            if (definition) {
                if (auto* candidate = definition->representative())
                    selected = candidate->path.clone();
                else
                    error = "wallpaper has no representative image"_Str;
            }
        }
        if (dependencies.len() > usize(8192)) return false;
        auto before = snapshots(dependencies);
        for (const auto& dependency : **before.as_array()) observed.push(dependency.clone());
        resolved();
        if (error.is_empty()) {
            auto info = wavsen::decode::probe_media(selected);
            if (info.is_err())
                error = rstd::move(info).unwrap_err().message;
            else {
                result["width"_str]  = integer(i64(info->width.to_primitive()));
                result["height"_str] = integer(i64(info->height.to_primitive()));
                result["size"_str]   = stamp(selected)["size"_str].clone();
            }
        }
    }
    bool changed = false;
    for (const auto& previous : observed)
        if (rstd::json::to_string(previous) !=
            rstd::json::to_string(stamp(text(previous, "path"_str))))
            changed = true;
    if (! error.is_empty()) {
        status               = "failed"_Str;
        result["width"_str]  = Value::Null();
        result["height"_str] = Value::Null();
        result["size"_str]   = Value::Null();
        result["error"_str]  = string(error);
        rstd::io::eprintln("{}: {}", path, error);
    }
    if (changed) status = "input_changed"_Str;
    result["event"_str]         = string("result"_str);
    result["request_id"_str]    = string(id);
    result["selected_path"_str] = string(selected);
    result["status"_str]        = string(status);
    result["dependencies"_str]  = snapshots(dependencies);
    return output_ok && output(result);
}

int run() {
    Value ready;
    ready["event"_str]            = string("ready"_str);
    ready["protocol_version"_str] = integer(i64(2));
    if (! output(ready)) return 1;
    LuaPrograms     scripts;
    bool            configured = false;
    auto            input      = rstd::io::stdin();
    array<u8, 8192> buffer {};
    Vec<u8>         line;
    for (;;) {
        auto count = rstd::as<rstd::io::Read>(input).read(buffer.as_mut_slice());
        if (count.is_err()) return 1;
        if (*count == usize()) return line.is_empty() ? 0 : 2;
        for (usize i {}; i < *count; ++i) {
            auto byte = buffer[i];
            if (byte == u8('\n')) {
                auto parsed = rstd::json::from_slice(line.as_slice());
                if (parsed.is_err()) return 2;
                if (! configured) {
                    if (text(*parsed, "event"_str) != "configure"_str ||
                        ! scripts.configure(*parsed))
                        return 2;
                    configured = true;
                    Value event;
                    event["event"_str] = string("configured"_str);
                    if (! output(event)) return 1;
                } else if (! request(*parsed, scripts)) {
                    rstd::io::eprintln("invalid probe request");
                    return 2;
                }
                line.clear();
            } else {
                if (line.len() >= usize(2 * 1024 * 1024)) return 2;
                line.push(byte);
            }
        }
    }
}
} // namespace waywallen::probe
