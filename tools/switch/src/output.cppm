export module waywallen.switcher:output;
export import :client;
export import rstd.json;
using namespace rstd::prelude;
using namespace rstd::literals;

export namespace waywallen::switch_cli
{
namespace pb = switch_client::pb;
using rstd::json::Value;
auto text(ref<str> value) -> Value { return Value::String(String::make(value)); }
template<class T>
auto id(T value) -> Value {
    return text(rstd::format("{}", value).as_str());
}
template<class T>
auto number(T value) -> Value {
    return Value::Number(rstd::json::Number::from_u64(u64(value.to_primitive())));
}
bool output(const Value& value) {
    auto line = rstd::json::to_string(value);
    line.push_ascii('\n');
    auto stream = rstd::io::stdout();
    return rstd::io::write_all(stream, line.as_str().as_bytes()).is_ok();
}
auto strings(const Vec<String>& values) -> Value {
    Vec<Value> result;
    for (const auto& value : values) result.push(text(value.as_str()));
    return Value::Array(rstd::move(result));
}
auto wallpaper(const pb::M_WallpaperEntry& entry) -> Value {
    Value result;
    result["id"_str]             = text(entry.f_id.as_str());
    result["name"_str]           = text(entry.f_name.as_str());
    result["type"_str]           = text(entry.f_wp_type.as_str());
    result["resource"_str]       = text(entry.f_resource.as_str());
    result["preview"_str]        = text(entry.f_preview.as_str());
    result["size"_str]           = Value::Number(rstd::json::Number::from_i64(entry.f_size));
    result["width"_str]          = number(entry.f_width);
    result["height"_str]         = number(entry.f_height);
    result["hidden"_str]         = Value::Bool(entry.f_hidden);
    result["tags"_str]           = strings(entry.f_tags);
    result["description"_str]    = text(entry.f_description.as_str());
    result["external_id"_str]    = text(entry.f_external_id.as_str());
    result["web_url"_str]        = text(entry.f_web_url.as_str());
    result["schema_json"_str]    = text(entry.f_user_properties_schema.as_str());
    result["overrides_json"_str] = text(entry.f_user_property_overrides.as_str());
    Value metadata               = Value::Object(rstd::json::Map {});
    for (auto [key, value] : entry.f_metadata.iter())
        metadata[key->as_str()] = text(value->as_str());
    result["metadata"_str] = rstd::move(metadata);
    return result;
}
auto target(const pb::M_PresentationTarget& value) -> Value {
    Value result;
    if (value.o_target.is_some()) {
        if (value.o_target->is_display_id())
            result["display_id"_str] = id(value.o_target->as_display_id());
        else
            result["canvas_id"_str] = text(value.o_target->as_canvas_id().as_str());
    }
    return result;
}
auto playlist_status(const pb::M_PlaylistStatusResponse& value) -> Value {
    Value result;
    result["auto_attach_id"_str] = id(value.f_auto_attach_id);
    Vec<Value> displays;
    for (const auto& entry : value.f_displays) {
        Value display;
        display["display_id"_str]     = id(entry->f_display_id);
        display["playlist_id"_str]    = id(entry->f_active_id);
        display["current_id"_str]     = text(entry->f_current_id.as_str());
        display["position"_str]       = number(entry->f_position);
        display["count"_str]          = number(entry->f_count);
        display["remaining_secs"_str] = number(entry->f_remaining_secs);
        display["waiting"_str]        = Value::Bool(entry->f_waiting);
        display["error"_str]          = text(entry->f_error.as_str());
        displays.push(rstd::move(display));
    }
    result["displays"_str] = Value::Array(rstd::move(displays));
    return result;
}
auto response_value(const pb::M_Response& response, bool properties_only = false) -> Value {
    const auto& payload = *response.o_payload;
    Value       result;
    if (payload.is_wallpaper_list()) {
        const auto& list = *payload.as_wallpaper_list();
        Vec<Value>  items;
        for (const auto& entry : list.f_wallpapers) items.push(wallpaper(*entry));
        result["wallpapers"_str] = Value::Array(rstd::move(items));
        result["count"_str]      = number(list.f_count);
    } else if (payload.is_wallpaper_get()) {
        const auto& entry = **payload.as_wallpaper_get()->f_entry;
        if (properties_only) {
            result["wallpaper_id"_str]   = text(entry.f_id.as_str());
            result["schema_json"_str]    = text(entry.f_user_properties_schema.as_str());
            result["overrides_json"_str] = text(entry.f_user_property_overrides.as_str());
        } else
            result["wallpaper"_str] = wallpaper(entry);
    } else if (payload.is_wallpaper_apply()) {
        const auto& applied        = *payload.as_wallpaper_apply();
        result["wallpaper_id"_str] = text(applied.f_wallpaper_id.as_str());
        result["renderer_id"_str]  = text(applied.f_renderer_id.as_str());
        result["name"_str]         = text(applied.f_name.as_str());
        result["deferred"_str]     = Value::Bool(applied.f_deferred);
        Vec<Value> selected;
        for (const auto& entry : applied.f_targets) selected.push(target(*entry));
        result["targets"_str] = Value::Array(rstd::move(selected));
        Vec<Value> stopped;
        for (const auto& entry : applied.f_stopped_playlists) {
            Value item;
            item["playlist_id"_str]   = id(entry->f_playlist_id);
            item["playlist_name"_str] = text(entry->f_playlist_name.as_str());
            item["all_displays"_str]  = Value::Bool(entry->f_all_displays);
            Vec<Value> ids;
            for (auto display : entry->f_display_ids) ids.push(id(display));
            item["display_ids"_str] = Value::Array(rstd::move(ids));
            stopped.push(rstd::move(item));
        }
        result["stopped_playlists"_str] = Value::Array(rstd::move(stopped));
    } else if (payload.is_display_list()) {
        Vec<Value> displays;
        for (const auto& entry : payload.as_display_list()->f_displays) {
            Value display;
            display["id"_str]           = id(entry->f_display_id);
            display["name"_str]         = text(entry->f_name.as_str());
            display["alias"_str]        = text(entry->f_alias.as_str());
            display["width"_str]        = number(entry->f_width);
            display["height"_str]       = number(entry->f_height);
            display["settings_key"_str] = text(entry->f_settings_key.as_str());
            display["canvas_id"_str]    = text(entry->f_canvas_id.as_str());
            display["selectable"_str]   = Value::Bool(entry->f_selectable_target);
            display["paused"_str]       = Value::Bool(entry->f_effective_paused);
            display["role"_str] =
                Value::Number(rstd::json::Number::from_i64(i64(entry->f_role.to_primitive())));
            displays.push(rstd::move(display));
        }
        result["displays"_str] = Value::Array(rstd::move(displays));
    } else if (payload.is_canvas_list()) {
        Vec<Value> canvases;
        for (const auto& entry : payload.as_canvas_list()->f_canvases) {
            Value canvas;
            canvas["id"_str]           = text(entry->f_canvas_id.as_str());
            canvas["name"_str]         = text(entry->f_name.as_str());
            canvas["wallpaper_id"_str] = text(entry->f_wallpaper_id.as_str());
            Vec<Value> members;
            for (const auto& member : entry->f_members) {
                Value item;
                item["settings_key"_str] = text(member->f_settings_key.as_str());
                Vec<Value> ids;
                for (auto display : member->f_display_ids) ids.push(id(display));
                item["display_ids"_str] = Value::Array(rstd::move(ids));
                members.push(rstd::move(item));
            }
            canvas["members"_str] = Value::Array(rstd::move(members));
            canvases.push(rstd::move(canvas));
        }
        result["canvases"_str] = Value::Array(rstd::move(canvases));
    } else if (payload.is_playlist_list()) {
        Vec<Value> playlists;
        for (const auto& entry : payload.as_playlist_list()->f_playlists) {
            Value playlist;
            playlist["id"_str]            = id(entry->f_id);
            playlist["name"_str]          = text(entry->f_name.as_str());
            playlist["source_kind"_str]   = text(entry->f_source_kind.as_str());
            playlist["mode"_str]          = text(entry->f_mode == i32(1)   ? "sequential"_str
                                                 : entry->f_mode == i32(2) ? "shuffle"_str
                                                 : entry->f_mode == i32(3) ? "random"_str
                                                                           : "unknown"_str);
            playlist["interval_secs"_str] = number(entry->f_interval_secs);
            playlist["item_count"_str]    = number(entry->f_item_count);
            playlists.push(rstd::move(playlist));
        }
        result["playlists"_str] = Value::Array(rstd::move(playlists));
    } else if (payload.is_playlist_status())
        return playlist_status(*payload.as_playlist_status());
    else if (payload.is_health()) {
        result["service"_str] = text(payload.as_health()->f_service.as_str());
        result["state"_str]   = text(payload.as_health()->f_state.as_str());
        result["os_name"_str] = text(payload.as_health()->f_os_name.as_str());
    } else if (payload.is_global_pause_set())
        result["paused"_str] = Value::Bool(payload.as_global_pause_set()->f_paused);
    else if (payload.is_global_mute_set())
        result["muted"_str] = Value::Bool(payload.as_global_mute_set()->f_muted);
    else if (payload.is_global_stop_set())
        result["stopped"_str] = Value::Bool(payload.as_global_stop_set()->f_stopped);
    else
        result["ok"_str] = Value::Bool(true);
    return result;
}
} // namespace waywallen::switch_cli
