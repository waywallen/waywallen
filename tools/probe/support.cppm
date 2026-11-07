export module waywallen.probe:support;
export import rstd;
export import rstd.json;
using namespace rstd::prelude;
using namespace rstd::literals;
export namespace ww_probe
{
using rstd::json::Value;
using rstd::path::Path;
using Observer = dyn<FnMut<void(ref<str>)>>;
bool local_path(ref<str> path) {
    return path.starts_with("/"_str) && path.len() <= usize(16384) && ! path.contains("\0"_str);
}
auto string(ref<str> value) -> Value { return Value::String(String::make(value)); }
auto integer(i64 value) -> Value { return Value::Number(rstd::json::Number::from_i64(value)); }
auto text(const Value& value, ref<str> key) -> ref<str> {
    auto field = value[key].as_str();
    return field ? *field : ""_str;
}
bool output(const Value& value) {
    auto bytes = rstd::json::to_string(value);
    bytes.push_ascii('\n');
    auto stream = rstd::io::stdout();
    return rstd::io::write_all(stream, bytes.as_str().as_bytes()).is_ok();
}
auto stamp(ref<str> path) -> Value {
    Value value;
    value["path"_str] = string(path);
    auto meta         = rstd::fs::metadata(ref<Path>(path));
    value["kind"_str] = string(meta.is_err()    ? "missing"_str
                               : meta->is_dir() ? "directory"_str
                                                : "file"_str);
    value["size"_str] = meta.is_ok() && meta->len() <= u64(0x7fffffffffffffffULL)
                            ? integer(i64(static_cast<rstd::int64_t>(meta->len().to_primitive())))
                            : Value::Null();
    if (meta.is_ok()) {
        auto modified = meta->modified();
        if (modified.is_ok()) {
            auto unix    = modified->as_unix_time();
            auto seconds = unix.seconds.to_primitive();
            if (seconds >= -9223372036LL && seconds <= 9223372035LL)
                value["mtime_ns"_str] =
                    integer(i64(seconds * 1000000000LL + unix.nanoseconds.to_primitive()));
        }
        value["device"_str] = Value::Number(rstd::json::Number::from_u64(meta->dev()));
        value["inode"_str]  = Value::Number(rstd::json::Number::from_u64(meta->ino()));
    }
    return value;
}
auto snapshots(const Vec<String>& paths) -> Value {
    Vec<Value> result;
    for (const auto& path : paths) result.push(stamp(path));
    return Value::Array(rstd::move(result));
}
} // namespace ww_probe
