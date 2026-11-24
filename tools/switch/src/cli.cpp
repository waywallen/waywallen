module waywallen.switcher;
import :client;
import :output;
import rstd.argparse;
using namespace rstd::prelude;
using namespace rstd::literals;
namespace client = waywallen::switch_client;
namespace pb     = client::pb;
namespace ap     = rstd::argparse;
using client::Failure;
template<class T>
using CliResult = client::Result<T>;
using client::coro;

namespace waywallen::switch_cli
{
enum class Command
{
    Status,
    Wallpapers,
    Wallpaper,
    Apply,
    Properties,
    PropertySet,
    PropertyReset,
    Displays,
    Canvases,
    Playlists,
    PlaylistStatus,
    Activate,
    Deactivate,
    Jump,
    Next,
    Previous,
    Pause,
    Resume,
    Mute,
    Unmute,
    Stop,
    Start
};
struct Invocation {
    Command         command { Command::Status };
    client::Options connection;
    bool            json {};
    Vec<String>     arguments;
    Vec<u64>        displays;
    Vec<String>     canvases;
    String          renderer, search, type;
    u32             page {}, page_size { 100 };
    i64             playlist_id {};
};

auto usage(ref<str> message) -> Failure {
    return { client::ErrorKind::InvalidInput, String::make(message) };
}
int exit_code(client::ErrorKind kind) {
    switch (kind) {
    case client::ErrorKind::InvalidInput: return 2;
    case client::ErrorKind::Connection: return 3;
    case client::ErrorKind::Rejected: return 4;
    case client::ErrorKind::TimedOut: return 5;
    }
    return 3;
}
bool write_text(ref<str> text, bool error = false) {
    if (error) {
        auto stream = rstd::io::stderr();
        return rstd::io::write_all(stream, text.as_bytes()).is_ok();
    }
    auto stream = rstd::io::stdout();
    return rstd::io::write_all(stream, text.as_bytes()).is_ok();
}
template<class T>
void unique_push(Vec<T>& values, T value) {
    for (const auto& existing : values)
        if (existing == value) return;
    values.push(rstd::move(value));
}
template<class T>
auto get(const ap::Matches& matches, const ap::ArgKey<T>& key) -> Option<ref<T>> {
    return matches.get_one(key).unwrap();
}
struct CommandSpec {
    Command                    operation;
    Option<ap::CommandKey>     parent;
    ap::CommandKey             key;
    Vec<ap::ArgKey<String>>    arguments;
    Option<ap::ArgKey<u64>>    displays;
    Option<ap::ArgKey<String>> canvases, renderer, search, type;
    Option<ap::ArgKey<u32>>    page, page_size;
};

auto parse_args() -> CliResult<Option<Invocation>> {
    auto root = ap::Command::make("waywallen-switch"_str);
    root.about("Control the running wallpaper daemon without Qt or the UI"_str);
    root.version(LITO_PKG_VERSION ""_str);
    root.require_subcommand();
    root.after_help(R"(List commands print tab-separated rows; --json prints structured results.
IDs come from list commands, not names or paths. Omit targets to use all live targets.
Only one write is sent. A timeout does not imply rollback.
Exit codes: 0 success, 2 usage, 3 connection/protocol, 4 daemon error, 5 timeout.

Examples:
  waywallen-switch wallpaper list --search clouds
  waywallen-switch --json display list
  waywallen-switch wallpaper apply WALLPAPER_ID --display 1
  waywallen-switch playlist activate 2 --canvas CANVAS_ID
  waywallen-switch property set WALLPAPER_ID speed 0.5)"_str);
    auto json = root.add_arg(ap::Arg<bool>::flag("json"_str)
                                 .long_name("json"_str)
                                 .global()
                                 .help("Machine-readable JSON; IDs are strings"_str));
    auto timeout =
        root.add_arg(ap::Arg<u64>::value("timeout"_str, ap::from_str_parser<u64>())
                         .long_name("timeout"_str)
                         .global()
                         .value_name("SECONDS"_str)
                         .help("Connection/request deadline, 1-3600 seconds (default 10)"_str));
    auto port =
        root.add_arg(ap::Arg<u16>::value("port"_str, ap::from_str_parser<u16>())
                         .long_name("port"_str)
                         .global()
                         .value_name("PORT"_str)
                         .help("Connect to loopback directly; not supported by next/previous"_str));
    Vec<CommandSpec>   specs;
    constexpr unsigned targets = 1, renderer = 2, list = 4;
    auto               add = [&](ap::Command& group,
                                 Option<ap::CommandKey>
                                         parent,
                                 Command operation,
                                 ref<str>
                                     name,
                                 ref<str>
                                          about,
                                 unsigned options,
                                 auto... positional) {
        auto command = ap::Command::make(name);
        command.about(about);
        CommandSpec spec { operation, parent, command.key() };
        (spec.arguments.push(
             command.add_arg(ap::Arg<String>::value(positional, ap::string_parser()).required())),
         ...);
        if (options & targets) {
            spec.displays =
                Some(command.add_arg(ap::Arg<u64>::value("display"_str, ap::from_str_parser<u64>())
                                         .long_name("display"_str)
                                         .append()
                                         .help("Display ID; repeat for several targets"_str)));
            spec.canvases =
                Some(command.add_arg(ap::Arg<String>::value("canvas"_str, ap::string_parser())
                                         .long_name("canvas"_str)
                                         .append()
                                         .help("Canvas ID; repeat for several targets"_str)));
        }
        if (options & renderer)
            spec.renderer =
                Some(command.add_arg(ap::Arg<String>::value("renderer"_str, ap::string_parser())
                                         .long_name("renderer"_str)
                                         .help("Select an exact renderer plugin name"_str)));
        if (options & list) {
            spec.search =
                Some(command.add_arg(ap::Arg<String>::value("search"_str, ap::string_parser())
                                         .long_name("search"_str)
                                         .help("Free-text search performed by the daemon"_str)));
            spec.type = Some(command.add_arg(
                ap::Arg<String>::value("type"_str, ap::string_parser())
                    .long_name("type"_str)
                    .help("Wallpaper type (for example image, video or scene)"_str)));
            spec.page =
                Some(command.add_arg(ap::Arg<u32>::value("page"_str, ap::from_str_parser<u32>())
                                         .long_name("page"_str)
                                         .help("Zero-based page (default 0)"_str)));
            spec.page_size = Some(
                command.add_arg(ap::Arg<u32>::value("page-size"_str, ap::from_str_parser<u32>())
                                    .long_name("page-size"_str)
                                    .help("Entries per page, 1-1000 (default 100)"_str)));
        }
        group.add_subcommand(rstd::move(command));
        specs.push(rstd::move(spec));
    };
    auto wallpaper = ap::Command::make("wallpaper"_str);
    wallpaper.about("Find, inspect and apply wallpapers"_str).require_subcommand();
    auto group = Some(wallpaper.key());
    add(wallpaper, group, Command::Wallpapers, "list"_str, "List wallpapers"_str, list);
    add(wallpaper, group, Command::Wallpaper, "get"_str, "Show wallpaper details"_str, 0, "ID"_str);
    add(wallpaper,
        group,
        Command::Apply,
        "apply"_str,
        "Apply a wallpaper to selected targets"_str,
        targets | renderer,
        "ID"_str);
    root.add_subcommand(rstd::move(wallpaper));
    auto property = ap::Command::make("property"_str);
    property.about("Read or update wallpaper user properties"_str).require_subcommand();
    group = Some(property.key());
    add(property,
        group,
        Command::Properties,
        "list"_str,
        "Show property schema and overrides as opaque JSON strings"_str,
        0,
        "ID"_str);
    add(property,
        group,
        Command::PropertySet,
        "set"_str,
        "Save one property; empty VALUE is valid"_str,
        0,
        "ID"_str,
        "KEY"_str,
        "VALUE"_str);
    add(property,
        group,
        Command::PropertyReset,
        "reset"_str,
        "Reset one property to its default"_str,
        0,
        "ID"_str,
        "KEY"_str);
    root.add_subcommand(rstd::move(property));
    auto display = ap::Command::make("display"_str);
    display.about("Inspect displays"_str).require_subcommand();
    add(display,
        Some(display.key()),
        Command::Displays,
        "list"_str,
        "List displays and their selectable IDs"_str,
        0);
    root.add_subcommand(rstd::move(display));
    auto canvas = ap::Command::make("canvas"_str);
    canvas.about("Inspect canvases"_str).require_subcommand();
    add(canvas,
        Some(canvas.key()),
        Command::Canvases,
        "list"_str,
        "List canvases and their members"_str,
        0);
    root.add_subcommand(rstd::move(canvas));
    auto playlist = ap::Command::make("playlist"_str);
    playlist.about("Inspect and control existing playlists"_str).require_subcommand();
    group = Some(playlist.key());
    add(playlist, group, Command::Playlists, "list"_str, "List saved playlists"_str, 0);
    add(playlist, group, Command::PlaylistStatus, "status"_str, "Show active playlists"_str, 0);
    add(playlist,
        group,
        Command::Activate,
        "activate"_str,
        "Activate an existing playlist"_str,
        targets,
        "ID"_str);
    add(playlist,
        group,
        Command::Deactivate,
        "deactivate"_str,
        "Deactivate playlists without clearing auto-attach"_str,
        targets);
    add(playlist,
        group,
        Command::Jump,
        "jump"_str,
        "Jump within an existing playlist"_str,
        0,
        "ID"_str,
        "WALLPAPER_ID"_str);
    root.add_subcommand(rstd::move(playlist));
    add(root, None(), Command::Status, "status"_str, "Show daemon and playback status"_str, 0);
    add(root,
        None(),
        Command::Next,
        "next"_str,
        "Advance using the daemon's playback policy"_str,
        0);
    add(root,
        None(),
        Command::Previous,
        "previous"_str,
        "Go back using the daemon's playback policy"_str,
        0);
    add(root, None(), Command::Pause, "pause"_str, "Set global pause"_str, 0);
    add(root, None(), Command::Resume, "resume"_str, "Clear global pause"_str, 0);
    add(root, None(), Command::Mute, "mute"_str, "Set global mute"_str, 0);
    add(root, None(), Command::Unmute, "unmute"_str, "Clear global mute"_str, 0);
    add(root, None(), Command::Stop, "stop"_str, "Set global stop"_str, 0);
    add(root, None(), Command::Start, "start"_str, "Clear global stop"_str, 0);
    auto built = rstd::move(root).build();
    if (built.is_err()) return Err(usage("invalid CLI definition"_str));
    auto parser = rstd::move(built).unwrap();
    auto parsed = parser.parse_env();
    if (parsed.is_err()) {
        auto report = parser.render_error(parsed.unwrap_err());
        write_text(report.text(), true);
        return Err(usage("use --help for command options"_str));
    }
    auto outcome = rstd::move(parsed).unwrap();
    if (outcome.is_Display()) {
        const auto& request = outcome.as_Display().request;
        if (! write_text(request.text(), request.target() == ap::OutputTarget::Tag::Stderr))
            return Err(Failure { client::ErrorKind::Connection,
                                 String::make("cannot write CLI help"_str) });
        return Ok(None<Invocation>());
    }
    const auto& matches = outcome.as_Parsed().value;
    Invocation  result;
    if (auto value = get(matches, json); value.is_some()) result.json = **value;
    if (auto value = get(matches, port); value.is_some()) {
        if (**value == u16()) return Err(usage("port must be between 1 and 65535"_str));
        result.connection.port = Some(u16(**value));
    }
    if (auto value = get(matches, timeout); value.is_some()) {
        if (**value == u64() || **value > u64(3600))
            return Err(usage("timeout must be between 1 and 3600 seconds"_str));
        result.connection.timeout = client::Duration::from_secs(**value);
    }
    for (const auto& spec : specs) {
        const ap::Matches* parent = &matches;
        if (spec.parent.is_some()) {
            auto group_matches = matches.subcommand_matches(*spec.parent);
            if (group_matches.is_none()) continue;
            parent = (*group_matches).as_raw_ptr();
        }
        auto selected = parent->subcommand_matches(spec.key);
        if (selected.is_none()) continue;
        const auto& values = **selected;
        result.command     = spec.operation;
        for (const auto& key : spec.arguments) {
            auto value = get(values, key).unwrap();
            if (value->is_empty() &&
                ! (result.command == Command::PropertySet && result.arguments.len() == usize(2)))
                return Err(usage("ID and property key must not be empty"_str));
            result.arguments.push(value->clone());
        }
        if (spec.displays.is_some()) {
            auto all = values.get_many(*spec.displays).unwrap();
            if (all.is_some())
                for (auto value : *all) {
                    if (*value == u64()) return Err(usage("display ID must not be zero"_str));
                    unique_push(result.displays, u64(*value));
                }
        }
        if (spec.canvases.is_some()) {
            auto all = values.get_many(*spec.canvases).unwrap();
            if (all.is_some())
                for (auto value : *all) {
                    if (value->is_empty()) return Err(usage("canvas ID must not be empty"_str));
                    unique_push(result.canvases, value->clone());
                }
        }
        if (spec.renderer.is_some())
            if (auto value = get(values, *spec.renderer); value.is_some()) {
                if ((*value)->is_empty()) return Err(usage("renderer name must not be empty"_str));
                result.renderer = (*value)->clone();
            }
        if (spec.search.is_some())
            if (auto value = get(values, *spec.search); value.is_some())
                result.search = (*value)->clone();
        if (spec.type.is_some())
            if (auto value = get(values, *spec.type); value.is_some())
                result.type = (*value)->clone();
        if (spec.page.is_some())
            if (auto value = get(values, *spec.page); value.is_some()) result.page = **value;
        if (spec.page_size.is_some())
            if (auto value = get(values, *spec.page_size); value.is_some()) {
                if (**value == u32() || **value > u32(1000))
                    return Err(usage("page-size must be between 1 and 1000"_str));
                result.page_size = **value;
            }
        if ((result.command == Command::Next || result.command == Command::Previous) &&
            result.connection.port.is_some())
            return Err(usage("next/previous require DBus discovery; do not use --port"_str));
        if (result.command == Command::Activate || result.command == Command::Jump) {
            auto id = rstd::from_str<i64>(result.arguments[usize()].as_str());
            if (id.is_err() || *id <= i64())
                return Err(usage("playlist ID must be a positive int64"_str));
            result.playlist_id = *id;
        }
        return Ok(Some(rstd::move(result)));
    }
    return Err(usage("a command is required; use --help"_str));
}

auto targets(const Invocation& invocation) -> Vec<Box<pb::M_PresentationTarget>> {
    Vec<Box<pb::M_PresentationTarget>> result;
    for (auto display : invocation.displays) {
        auto value      = Box<pb::M_PresentationTarget>::make();
        value->o_target = Some(pb::M_PresentationTarget::C_o_target::make_display_id(display));
        result.push(rstd::move(value));
    }
    for (const auto& canvas : invocation.canvases) {
        auto value = Box<pb::M_PresentationTarget>::make();
        value->o_target =
            Some(pb::M_PresentationTarget::C_o_target::make_canvas_id(canvas.clone()));
        result.push(rstd::move(value));
    }
    return result;
}
auto make_request(const Invocation& invocation) -> pb::M_Request {
    using Payload = pb::M_Request::C_o_payload;
    pb::M_Request request;
    auto          wallpaper_id = [&] {
        return invocation.arguments[usize()].clone();
    };
    switch (invocation.command) {
    case Command::Status:
        request.o_payload = Some(Payload::make_health(Box<pb::M_HealthRequest>::make()));
        break;
    case Command::Wallpapers: {
        auto value           = Box<pb::M_WallpaperListRequest>::make();
        value->f_page        = invocation.page;
        value->f_page_size   = invocation.page_size;
        value->f_search_text = invocation.search.clone();
        value->f_wp_type     = invocation.type.clone();
        request.o_payload    = Some(Payload::make_wallpaper_list(rstd::move(value)));
        break;
    }
    case Command::Wallpaper:
    case Command::Properties: {
        auto value            = Box<pb::M_WallpaperGetRequest>::make();
        value->f_wallpaper_id = wallpaper_id();
        request.o_payload     = Some(Payload::make_wallpaper_get(rstd::move(value)));
        break;
    }
    case Command::Apply: {
        auto value             = Box<pb::M_WallpaperApplyRequest>::make();
        value->f_wallpaper_id  = wallpaper_id();
        value->f_targets       = targets(invocation);
        value->f_renderer_name = invocation.renderer.clone();
        request.o_payload      = Some(Payload::make_wallpaper_apply(rstd::move(value)));
        break;
    }
    case Command::PropertySet:
    case Command::PropertyReset: {
        auto value            = Box<pb::M_WallpaperPropertySetRequest>::make();
        value->f_wallpaper_id = wallpaper_id();
        value->f_key          = invocation.arguments[usize(1)].clone();
        using Operation       = pb::M_WallpaperPropertySetRequest::C_o_operation;
        value->o_operation =
            Some(invocation.command == Command::PropertySet
                     ? Operation::make_value(invocation.arguments[usize(2)].clone())
                     : Operation::make_reset(true));
        request.o_payload = Some(Payload::make_wallpaper_property_set(rstd::move(value)));
        break;
    }
    case Command::Displays:
        request.o_payload = Some(Payload::make_display_list(Box<pb::M_DisplayListRequest>::make()));
        break;
    case Command::Canvases:
        request.o_payload = Some(Payload::make_canvas_list(Box<pb::M_CanvasListRequest>::make()));
        break;
    case Command::Playlists:
        request.o_payload =
            Some(Payload::make_playlist_list(Box<pb::M_PlaylistListRequest>::make()));
        break;
    case Command::PlaylistStatus:
        request.o_payload =
            Some(Payload::make_playlist_status(Box<pb::M_PlaylistStatusRequest>::make()));
        break;
    case Command::Activate: {
        auto value        = Box<pb::M_PlaylistActivateRequest>::make();
        value->f_id       = invocation.playlist_id;
        value->f_targets  = targets(invocation);
        request.o_payload = Some(Payload::make_playlist_activate(rstd::move(value)));
        break;
    }
    case Command::Deactivate: {
        auto value        = Box<pb::M_PlaylistDeactivateRequest>::make();
        value->f_targets  = targets(invocation);
        request.o_payload = Some(Payload::make_playlist_deactivate(rstd::move(value)));
        break;
    }
    case Command::Jump: {
        auto value        = Box<pb::M_PlaylistJumpToRequest>::make();
        value->f_id       = invocation.playlist_id;
        value->f_entry_id = invocation.arguments[usize(1)].clone();
        request.o_payload = Some(Payload::make_playlist_jump_to(rstd::move(value)));
        break;
    }
    case Command::Pause:
    case Command::Resume: {
        auto value        = Box<pb::M_GlobalPauseSetRequest>::make();
        value->f_paused   = invocation.command == Command::Pause;
        request.o_payload = Some(Payload::make_global_pause_set(rstd::move(value)));
        break;
    }
    case Command::Mute:
    case Command::Unmute: {
        auto value        = Box<pb::M_GlobalMuteSetRequest>::make();
        value->f_muted    = invocation.command == Command::Mute;
        request.o_payload = Some(Payload::make_global_mute_set(rstd::move(value)));
        break;
    }
    case Command::Stop:
    case Command::Start: {
        auto value        = Box<pb::M_GlobalStopSetRequest>::make();
        value->f_stopped  = invocation.command == Command::Stop;
        request.o_payload = Some(Payload::make_global_stop_set(rstd::move(value)));
        break;
    }
    default: break;
    }
    return request;
}

auto execute(const Invocation& invocation) -> coro<CliResult<Value>> {
    if (invocation.command == Command::Next || invocation.command == Command::Previous) {
        auto stepped = co_await client::step(invocation.command == Command::Previous,
                                             invocation.connection.timeout);
        if (stepped.is_err()) co_return Err(rstd::move(stepped).unwrap_err());
        Value result;
        result["ok"_str] = Value::Bool(true);
        co_return Ok(rstd::move(result));
    }
    auto connected = co_await client::Client::connect(invocation.connection);
    if (connected.is_err()) co_return Err(rstd::move(connected).unwrap_err());
    auto             connection = rstd::move(connected).unwrap();
    auto             response   = co_await connection.request(make_request(invocation));
    CliResult<Value> result =
        Err(Failure { client::ErrorKind::Connection, String::make("missing wallpaper entry"_str) });
    if (response.is_err())
        result = Err(rstd::move(response).unwrap_err());
    else if (! response->o_payload->is_wallpaper_get() ||
             response->o_payload->as_wallpaper_get()->f_entry.is_some())
        result = Ok(response_value(*response, invocation.command == Command::Properties));
    if (result.is_ok() && invocation.command == Command::Status) {
        pb::M_Request request;
        request.o_payload = Some(pb::M_Request::C_o_payload::make_playlist_status(
            Box<pb::M_PlaylistStatusRequest>::make()));
        auto playlists    = co_await connection.request(rstd::move(request));
        if (playlists.is_err())
            result = Err(rstd::move(playlists).unwrap_err());
        else {
            (*result)["playback"_str] = response_value(*playlists);
            const auto& status        = connection.status();
            if (status.is_some()) {
                (*result)["paused"_str]            = Value::Bool(status->f_global_paused);
                (*result)["muted"_str]             = Value::Bool(status->f_global_muted);
                (*result)["stopped"_str]           = Value::Bool(status->f_global_stopped);
                (*result)["scan_in_progress"_str]  = Value::Bool(status->f_scan_in_progress);
                (*result)["active_task_count"_str] = number(status->f_active_task_count);
            }
        }
    }
    co_await connection.close();
    co_return rstd::move(result);
}
auto table_cell(const Value& value) -> String {
    auto    string = value.as_str();
    auto    source = string.is_some() ? String::make(*string) : rstd::json::to_string(value);
    Vec<u8> bytes;
    for (auto byte : source.as_str().as_bytes())
        bytes.push(byte < u8(32) || byte == u8(127) ? u8(' ') : byte);
    return String::from_utf8(rstd::move(bytes)).unwrap();
}
bool human_output(Command command, const Value& result) {
    ref<str>      key = ""_str;
    Vec<ref<str>> columns;
    auto          select_columns = [&](auto... names) {
        (columns.push(rstd::move(names)), ...);
    };
    switch (command) {
    case Command::Wallpapers:
        key = "wallpapers"_str;
        select_columns("id"_str, "type"_str, "name"_str);
        break;
    case Command::Displays:
        key = "displays"_str;
        select_columns(
            "id"_str, "name"_str, "width"_str, "height"_str, "canvas_id"_str, "selectable"_str);
        break;
    case Command::Canvases:
        key = "canvases"_str;
        select_columns("id"_str, "name"_str, "wallpaper_id"_str);
        break;
    case Command::Playlists:
        key = "playlists"_str;
        select_columns("id"_str, "name"_str, "source_kind"_str, "mode"_str, "item_count"_str);
        break;
    default: return output(result);
    }
    String line;
    for (auto column : columns) {
        if (! line.is_empty()) line.push_ascii('\t');
        line.push_str(column);
    }
    line.push_ascii('\n');
    if (! write_text(line.as_str())) return false;
    for (const auto& item : **result[key].as_array()) {
        line       = String {};
        bool first = true;
        for (auto column : columns) {
            if (! first) line.push_ascii('\t');
            first = false;
            line.push_str(table_cell(item[column]).as_str());
        }
        line.push_ascii('\n');
        if (! write_text(line.as_str())) return false;
    }
    return true;
}
int run() {
    auto parsed = parse_args();
    if (parsed.is_err()) {
        write_text(rstd::format("waywallen-switch: {}\n", parsed.unwrap_err().message).as_str(),
                   true);
        return exit_code(parsed.unwrap_err().kind);
    }
    auto invocation = rstd::move(*parsed);
    if (invocation.is_none()) return 0;
    auto runtime = rstd::async::RuntimeBuilder::current_thread().enable_all().build();
    if (runtime.is_err()) {
        write_text("waywallen-switch: cannot create I/O runtime\n"_str, true);
        return 3;
    }
    auto result = runtime->block_on(execute(*invocation));
    if (result.is_err()) {
        const auto& error = result.unwrap_err();
        write_text(rstd::format("waywallen-switch: {}\n", error.message).as_str(), true);
        return exit_code(error.kind);
    }
    if (invocation->json) return output(*result) ? 0 : 3;
    return human_output(invocation->command, *result) ? 0 : 3;
}
} // namespace waywallen::switch_cli
