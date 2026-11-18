module;
#include <cerrno>
#include <csignal>
#include <fcntl.h>
#include <unistd.h>

module waywallen.web;
import :ffi;
import rstd.json;

using namespace rstd::prelude;
using namespace rstd::literals;

namespace waywallen::web
{
namespace
{
using rstd::json::Value;
constexpr usize       max_frame    = usize(1024 * 1024);
constexpr const char* script_world = "waywallen-login";

auto string(ref<str> value) -> Value { return Value::String(String::make(value)); }
auto text(const Value& value, ref<str> key) -> ref<str> {
    auto result = value[key].as_str();
    return result ? *result : ""_str;
}
auto cstring(ref<str> value) -> rstd::ffi::CString {
    return rstd::ffi::CString::make(String::make(value).into_bytes()).unwrap();
}
auto ctext(const char* value) -> ref<str> {
    return value ? rstd::ffi::CStr::from_ptr(value).to_str().unwrap_or(""_str) : ""_str;
}
bool valid_text(ref<str> value, usize limit = max_frame) {
    return value.len() <= limit && ! value.contains("\0"_str);
}
bool output(const Value& value) {
    auto bytes = rstd::json::to_string(value);
    if (bytes.len() > max_frame) return false;
    bytes.push_ascii('\n');
    auto stream = rstd::io::stdout();
    return rstd::io::write_all(stream, bytes.as_str().as_bytes()).is_ok();
}
auto event(ref<str> name) -> Value {
    Value value;
    value["version"_str] = Value::Number(rstd::json::Number::from_u64(u64(1)));
    value["event"_str]   = string(name);
    return value;
}
auto request_id(const Value& request) -> Value {
    return Value::Number(rstd::json::Number::from_u64(request["id"_str].as_u64().unwrap_or(u64())));
}
void error(ref<str> code, ref<str> message) {
    auto value           = event("error"_str);
    value["code"_str]    = string(code);
    value["message"_str] = string(message);
    output(value);
}
bool array_limit(const Value& value, usize limit) {
    if (value.is_Null()) return true;
    auto values = value.as_array();
    return values && (**values).len() <= limit;
}
bool names_valid(const Value& value) {
    if (! array_limit(value, usize(64))) return false;
    if (auto values = value.as_array())
        for (const auto& entry : **values) {
            auto name = entry.as_str();
            if (! name || name->is_empty() || ! valid_text(*name, usize(256))) return false;
        }
    return true;
}
bool wanted(const Value& names, ref<str> name) {
    auto values = names.as_array();
    if (! values || (**values).is_empty()) return true;
    for (const auto& entry : **values)
        if (entry.as_str() == Some(name)) return true;
    return false;
}

struct Uri {
    Api&   api;
    Object value {};
    Uri(Api& api, ref<str> text): api(api) {
        if (text.is_empty() || ! valid_text(text, usize(16384))) return;
        auto   bytes = cstring(text);
        Object failure {};
        value = api.g_uri_parse(bytes.as_ptr(), 0, &failure);
        if (failure) api.g_error_free(failure);
        if (value && (! api.g_uri_get_scheme(value) || api.g_uri_get_userinfo(value))) {
            api.g_uri_unref(value);
            value = nullptr;
        }
    }
    ~Uri() {
        if (value) api.g_uri_unref(value);
    }
    Uri(const Uri&) = delete;
    bool http() const {
        if (! value || ! api.g_uri_get_host(value)) return false;
        auto scheme = api.g_uri_get_scheme(value);
        return ! api.g_ascii_strcasecmp(scheme, "https") ||
               ! api.g_ascii_strcasecmp(scheme, "http");
    }
    bool http_origin() const {
        if (! http()) return false;
        const auto path = ctext(api.g_uri_get_path(value));
        return (path.is_empty() || path == "/"_str) && ! api.g_uri_get_query(value) &&
               ! api.g_uri_get_fragment(value);
    }
    int port() const {
        int number = api.g_uri_get_port(value);
        if (number >= 0) return number;
        auto scheme = api.g_uri_get_scheme(value);
        if (! api.g_ascii_strcasecmp(scheme, "https")) return 443;
        if (! api.g_ascii_strcasecmp(scheme, "http")) return 80;
        return -1;
    }
    bool same_origin(const Uri& other) const {
        return value && other.value &&
               ! api.g_ascii_strcasecmp(api.g_uri_get_scheme(value),
                                        api.g_uri_get_scheme(other.value)) &&
               ! api.g_ascii_strcasecmp(
                   api.g_uri_get_host(value) ? api.g_uri_get_host(value) : "",
                   api.g_uri_get_host(other.value) ? api.g_uri_get_host(other.value) : "") &&
               port() == other.port();
    }
    bool same_target(const Uri& other) const {
        if (! same_origin(other)) return false;
        auto path       = ctext(api.g_uri_get_path(value));
        auto other_path = ctext(api.g_uri_get_path(other.value));
        if (path.is_empty() && http()) path = "/"_str;
        if (other_path.is_empty() && other.http()) other_path = "/"_str;
        return path == other_path;
    }
};

struct Session;
struct View {
    Session* owner;
    Object   window {};
    Object   widget {};
};
struct Pending {
    Session* owner;
    View*    view {};
    Value    request;
    String   origin;
};

struct Session {
    Api&       api;
    Object     loop {};
    Object     network {};
    Object     cookies {};
    Object     content {};
    Object     cancellable {};
    Vec<View*> views;
    Value      request;
    Vec<u8>    input;
    usize      pending {};
    bool       closing {};
    bool       failed {};
    unsigned   input_source {};

    explicit Session(Api& api, Value value): api(api), request(rstd::move(value)) {}

    void stop(bool failure = false) {
        if (closing) return;
        closing = true;
        failed  = failure;
        api.g_cancellable_cancel(cancellable);
        for (auto* view : views) {
            if (view->widget) api.webkit_web_view_stop_loading(view->widget);
            if (view->window) {
                auto window  = view->window;
                view->window = nullptr;
                view->widget = nullptr;
                api.destroy(window);
            }
        }
        if (! failed) output(event("closed"_str));
        api.g_main_loop_quit(loop);
    }
    void fail(ref<str> code, ref<str> message) {
        if (closing) return;
        error(code, message);
        stop(true);
    }
    bool redirect(ref<str> uri) {
        if (closing) return false;
        Uri actual(api, uri);
        if (! actual.value) return false;
        auto redirects = request["redirects"_str].as_array();
        if (! redirects) return false;
        for (const auto& entry : **redirects) {
            Uri expected(api, *entry.as_str());
            if (! actual.same_target(expected)) continue;
            auto value       = event("redirect"_str);
            value["uri"_str] = string(uri);
            if (! output(value)) stop(true);
            return true;
        }
        return false;
    }
    View* for_origin(ref<str> origin) {
        Uri requested(api, origin);
        if (! requested.http_origin()) return nullptr;
        for (auto* view : views) {
            if (! view->widget) continue;
            Uri current(api, ctext(api.webkit_web_view_get_uri(view->widget)));
            if (current.same_origin(requested)) return view;
        }
        return nullptr;
    }
    bool allowed_script_origin(ref<str> origin) {
        Uri  target(api, origin);
        auto scripts = request["scripts"_str].as_array();
        if (! scripts) return false;
        for (const auto& script : **scripts)
            for (const auto& entry : **script["origins"_str].as_array()) {
                Uri allowed(api, *entry.as_str());
                if (target.same_origin(allowed)) return true;
            }
        return false;
    }
    static int policy(Object, Object decision, int type, void* data) {
        auto* view = static_cast<View*>(data);
        auto& self = *view->owner;
        if (self.closing) {
            self.api.webkit_policy_decision_ignore(decision);
            return 1;
        }
        if (type != 0 && type != 1) return 0;
        auto action  = self.api.webkit_navigation_policy_decision_get_navigation_action(decision);
        auto request = self.api.webkit_navigation_action_get_request(action);
        auto uri     = ctext(self.api.webkit_uri_request_get_uri(request));
        Uri  parsed(self.api, uri);
        bool capture = false;
        if (parsed.value) {
            auto targets = self.request["redirects"_str].as_array();
            if (targets)
                for (const auto& target : **targets) {
                    Uri expected(self.api, *target.as_str());
                    if (parsed.same_target(expected)) capture = true;
                }
        }
        if (capture) {
            self.api.webkit_policy_decision_ignore(decision);
            self.redirect(uri);
            return 1;
        }
        if (! parsed.http() && uri != "about:blank"_str) {
            self.api.webkit_policy_decision_ignore(decision);
            return 1;
        }
        return 0;
    }
    static int window_close(Object, void* data) {
        auto* view = static_cast<View*>(data);
        auto& self = *view->owner;
        if (! self.views.is_empty() && self.views[usize()] == view) {
            self.stop();
            return 1;
        } else {
            if (view->widget) self.api.webkit_web_view_stop_loading(view->widget);
            view->widget = nullptr;
            view->window = nullptr;
        }
        return 0;
    }
    static int window_delete(Object window, Object, void* data) {
        return window_close(window, data);
    }
    static void ready_to_show(Object, void* data) {
        auto* view = static_cast<View*>(data);
        if (! view->owner->closing && view->window) view->owner->api.present(view->window);
    }
    static Object create(Object, Object, void* data) {
        auto* view = static_cast<View*>(data);
        if (view->owner->closing || view->owner->views.len() >= usize(8)) return nullptr;
        return view->owner->new_view(view->widget)->widget;
    }
    static void process_terminated(Object, int, void* data) {
        auto* view = static_cast<View*>(data);
        view->owner->fail("web_process"_str, "The browser web process terminated"_str);
    }
    static void script_message(Object, Object value, void* data) {
        auto& self = *static_cast<Session*>(data);
        if (self.closing || self.views.is_empty()) return;
        // Each script posts its origin together with the payload from an isolated world.
        char* encoded = self.api.jsc_value_to_json(self.api.message_value(value), 0);
        if (! encoded) return;
        auto bytes = ctext(encoded);
        if (bytes.len() <= max_frame) {
            auto parsed = rstd::json::from_str(bytes);
            if (parsed.is_ok() && self.allowed_script_origin(text(*parsed, "origin"_str)) &&
                self.for_origin(text(*parsed, "origin"_str))) {
                auto message          = event("script_message"_str);
                message["origin"_str] = (*parsed)["origin"_str].clone();
                message["value"_str]  = (*parsed)["value"_str].clone();
                if (! output(message)) self.stop(true);
            }
        } else
            self.fail("output_limit"_str, "Browser script message exceeds protocol limit"_str);
        self.api.g_free(encoded);
    }
    View* new_view(Object related = nullptr) {
        auto* view   = new View { this };
        view->window = api.new_window();
        view->widget = api.new_web_view(network, content, related);
        views.push(static_cast<View*>(view));
        auto title = cstring(text(request, "title"_str).is_empty() ? "Waywallen sign-in"_str
                                                                   : text(request, "title"_str));
        api.gtk_window_set_title(view->window, title.as_ptr());
        api.gtk_window_set_default_size(view->window, 960, 720);
        api.attach(view->window, view->widget);
        api.webkit_settings_set_enable_javascript(api.webkit_web_view_get_settings(view->widget),
                                                  1);
        api.on_close(view->window,
                     reinterpret_cast<Callback>(window_close),
                     reinterpret_cast<Callback>(window_delete),
                     view);
        api.g_signal_connect_data(
            view->widget, "decide-policy", reinterpret_cast<Callback>(policy), view, nullptr, 0);
        api.g_signal_connect_data(
            view->widget, "create", reinterpret_cast<Callback>(create), view, nullptr, 0);
        api.g_signal_connect_data(view->widget,
                                  "ready-to-show",
                                  reinterpret_cast<Callback>(ready_to_show),
                                  view,
                                  nullptr,
                                  0);
        api.g_signal_connect_data(view->widget,
                                  "web-process-terminated",
                                  reinterpret_cast<Callback>(process_terminated),
                                  view,
                                  nullptr,
                                  0);
        return view;
    }
    bool configure() {
        Uri url(api, text(request, "url"_str));
        if (! url.http() || ! valid_text(text(request, "title"_str), usize(4096))) return false;
        if (! array_limit(request["redirects"_str], usize(32)) ||
            ! array_limit(request["cookie_requests"_str], usize(32)) ||
            ! array_limit(request["scripts"_str], usize(16)))
            return false;
        if (auto redirects = request["redirects"_str].as_array())
            for (const auto& target : **redirects) {
                auto uri = target.as_str();
                if (! uri || ! Uri(api, *uri).value) return false;
            }
        if (auto requests = request["cookie_requests"_str].as_array())
            for (const auto& entry : **requests) {
                Uri uri(api, text(entry, "uri"_str));
                if (! uri.http() || ! names_valid(entry["names"_str])) return false;
            }
        if (auto scripts = request["scripts"_str].as_array())
            for (const auto& script : **scripts) {
                if (! valid_text(text(script, "source"_str), usize(256 * 1024)) ||
                    ! script["origins"_str].as_array() ||
                    (**script["origins"_str].as_array()).is_empty() ||
                    ! array_limit(script["origins"_str], usize(32)))
                    return false;
                for (const auto& origin : **script["origins"_str].as_array()) {
                    auto string = origin.as_str();
                    if (! string || ! Uri(api, *string).http_origin()) return false;
                }
            }
        return true;
    }
    bool add_scripts() {
        auto scripts = request["scripts"_str].as_array();
        if (! scripts || (**scripts).is_empty()) return true;
        api.g_signal_connect_data(content,
                                  "script-message-received::waywallen",
                                  reinterpret_cast<Callback>(script_message),
                                  this,
                                  nullptr,
                                  0);
        if (! api.register_messages(content, "waywallen", script_world)) return false;
        for (const auto& script : **scripts) {
            Vec<rstd::ffi::CString> patterns;
            for (const auto& origin : **script["origins"_str].as_array()) {
                auto base = String::make(*origin.as_str());
                if (base->ends_with("/"_str)) base.truncate(base.len() - usize(1));
                auto pattern = rstd::format("{}/*", base);
                patterns.push(cstring(pattern));
            }
            Vec<const char*> allowed;
            for (const auto& pattern : patterns) allowed.push(pattern.as_ptr());
            allowed.push(nullptr);
            auto wrapped =
                rstd::format("(() => {{ const send = "
                             "window.webkit.messageHandlers.waywallen.postMessage.bind(window."
                             "webkit.messageHandlers.waywallen); "
                             "globalThis.waywallen = Object.freeze({{ postMessage(value) {{ "
                             "send({{ origin: location.origin, value }}); }} }});\n{}\n}})();",
                             text(script, "source"_str));
            auto source = cstring(wrapped);
            auto value  = api.webkit_user_script_new_for_world(
                source.as_ptr(), 1, 1, script_world, allowed.as_ptr().as_raw_ptr(), nullptr);
            if (! value) return false;
            api.webkit_user_content_manager_add_script(content, value);
            api.webkit_user_script_unref(value);
        }
        return true;
    }
    static void cookies_ready(Object, Object result, void* data) {
        auto*  pending = static_cast<Pending*>(data);
        auto&  self    = *pending->owner;
        Object failure {};
        auto*  values =
            self.api.webkit_cookie_manager_get_cookies_finish(self.cookies, result, &failure);
        if (! self.closing && ! failure) {
            Vec<Value> list;
            for (auto* entry = values; entry; entry = entry->next) {
                auto cookie = entry->data;
                auto name   = ctext(self.api.soup_cookie_get_name(cookie));
                if (! wanted(pending->request["names"_str], name)) continue;
                Value value;
                value["name"_str]      = string(name);
                value["value"_str]     = string(ctext(self.api.soup_cookie_get_value(cookie)));
                value["domain"_str]    = string(ctext(self.api.soup_cookie_get_domain(cookie)));
                value["path"_str]      = string(ctext(self.api.soup_cookie_get_path(cookie)));
                value["secure"_str]    = Value::Bool(self.api.soup_cookie_get_secure(cookie));
                value["http_only"_str] = Value::Bool(self.api.soup_cookie_get_http_only(cookie));
                value["same_site"_str] = Value::Number(rstd::json::Number::from_i64(
                    i64(self.api.soup_cookie_get_same_site_policy(cookie))));
                auto expires           = self.api.soup_cookie_get_expires(cookie);
                if (expires)
                    value["expires"_str] = Value::Number(
                        rstd::json::Number::from_i64(i64(self.api.g_date_time_to_unix(expires))));
                list.push(rstd::move(value));
            }
            auto message           = event("cookies"_str);
            message["id"_str]      = request_id(pending->request);
            message["uri"_str]     = pending->request["uri"_str].clone();
            message["cookies"_str] = Value::Array(rstd::move(list));
            if (! output(message))
                self.fail("output_limit"_str, "Browser cookie result exceeds protocol limit"_str);
        } else if (! self.closing)
            self.fail("cookies"_str, "Could not read browser cookies"_str);
        for (auto* entry = values; entry; entry = entry->next)
            self.api.soup_cookie_free(entry->data);
        self.api.g_list_free(values);
        if (failure) self.api.g_error_free(failure);
        --self.pending;
        delete pending;
    }
    void get_cookies(const Value& request) {
        Uri uri(api, text(request, "uri"_str));
        if (! uri.http() || ! names_valid(request["names"_str]) || pending >= usize(32)) {
            fail("invalid_command"_str, "Invalid browser cookie request"_str);
            return;
        }
        auto* task = new Pending { this, nullptr, request.clone(), {} };
        ++pending;
        auto url = cstring(text(request, "uri"_str));
        api.webkit_cookie_manager_get_cookies(
            cookies, url.as_ptr(), cancellable, cookies_ready, task);
    }
    static void cookies_changed(Object, void* data) {
        auto& self = *static_cast<Session*>(data);
        if (self.closing || self.pending >= usize(16)) return;
        if (auto requests = self.request["cookie_requests"_str].as_array())
            for (const auto& request : **requests) self.get_cookies(request);
    }
    static void javascript_ready(Object object, Object result, void* data) {
        auto*  pending = static_cast<Pending*>(data);
        auto&  self    = *pending->owner;
        Object failure {};
        auto value = self.api.webkit_web_view_evaluate_javascript_finish(object, result, &failure);
        if (! self.closing && ! failure && pending->view->widget) {
            Uri expected(self.api, pending->origin);
            Uri actual(self.api, ctext(self.api.webkit_web_view_get_uri(pending->view->widget)));
            if (! expected.same_origin(actual))
                self.fail("navigation_changed"_str,
                          "The browser document changed during script evaluation"_str);
            else {
                auto message         = event("javascript"_str);
                message["id"_str]    = request_id(pending->request);
                message["value"_str] = Value::Null();
                char* encoded        = value ? self.api.jsc_value_to_json(value, 0) : nullptr;
                if (encoded && ctext(encoded).len() <= max_frame) {
                    auto parsed = rstd::json::from_str(ctext(encoded));
                    if (parsed.is_ok())
                        message["value"_str] = rstd::move(parsed).unwrap();
                    else
                        self.fail("javascript"_str, "Browser script result is not JSON"_str);
                } else if (encoded)
                    self.fail("output_limit"_str,
                              "Browser script result exceeds protocol limit"_str);
                if (encoded) self.api.g_free(encoded);
                if (! self.closing && ! output(message))
                    self.fail("output_limit"_str,
                              "Browser script result exceeds protocol limit"_str);
            }
        } else if (! self.closing)
            self.fail("javascript"_str, "Browser script evaluation failed"_str);
        if (value) self.api.g_object_unref(value);
        if (failure) self.api.g_error_free(failure);
        --self.pending;
        delete pending;
    }
    void command(const Value& value) {
        if (closing) return;
        if (! value.as_object() || value["version"_str].as_i64() != Some(i64(1))) {
            fail("protocol_version"_str, "Unsupported browser protocol version"_str);
            return;
        }
        auto kind = text(value, "command"_str);
        if (kind == "close"_str)
            stop();
        else if (kind == "get_cookies"_str)
            get_cookies(value);
        else if (kind == "evaluate"_str) {
            auto  origin = text(value, "origin"_str);
            auto* view   = for_origin(origin);
            if (! view || ! valid_text(text(value, "source"_str), usize(256 * 1024)) ||
                pending >= usize(32)) {
                fail("invalid_command"_str,
                     "Script target origin is not open in this browser session"_str);
                return;
            }
            auto* task = new Pending { this, view, value.clone(), String::make(origin) };
            ++pending;
            auto source = cstring(text(value, "source"_str));
            api.webkit_web_view_evaluate_javascript(view->widget,
                                                    source.as_ptr(),
                                                    -1,
                                                    script_world,
                                                    nullptr,
                                                    cancellable,
                                                    javascript_ready,
                                                    task);
        } else if (kind == "navigate"_str) {
            Uri uri(api, text(value, "url"_str));
            if (! uri.http())
                fail("invalid_command"_str, "Browser navigation requires an HTTP or HTTPS URL"_str);
            else {
                auto url = cstring(text(value, "url"_str));
                api.webkit_web_view_load_uri(views[usize()]->widget, url.as_ptr());
            }
        } else
            fail("invalid_command"_str, "Unknown browser command"_str);
    }
    static int read_input(void* data) {
        auto& self = *static_cast<Session*>(data);
        if (self.closing) return 0;
        char     bytes[8192];
        unsigned reads = 0;
        for (;;) {
            if (++reads > 8) return 1;
            auto count = ::read(STDIN_FILENO, bytes, sizeof(bytes));
            if (! count) {
                if (! self.input.is_empty())
                    self.fail("invalid_json"_str, "Incomplete browser protocol frame"_str);
                else
                    self.stop();
                return 0;
            }
            if (count < 0) {
                if (errno == EINTR) continue;
                if (errno == EAGAIN || errno == EWOULDBLOCK) return 1;
                self.fail("input"_str, "Could not read browser commands"_str);
                return 0;
            }
            for (decltype(count) i = 0; i < count; ++i) {
                if (bytes[i] == '\n') {
                    auto parsed = rstd::json::from_slice(self.input.as_slice());
                    self.input.clear();
                    if (parsed.is_err())
                        self.fail("invalid_json"_str, "Invalid browser protocol JSON"_str);
                    else
                        self.command(*parsed);
                    if (self.closing) return 0;
                } else {
                    if (self.input.len() >= max_frame) {
                        self.fail("input_limit"_str, "Browser protocol frame exceeds 1 MiB"_str);
                        return 0;
                    }
                    self.input.push(u8(static_cast<unsigned char>(bytes[i])));
                }
            }
        }
    }
    int run() {
        if (! configure()) {
            error("invalid_request"_str, "Invalid browser open request"_str);
            return 2;
        }
        if (! api.initialize()) {
            error("display_unavailable"_str,
                  rstd::format("{} could not connect to the graphical session; check DISPLAY or "
                               "WAYLAND_DISPLAY",
                               backend_label(api.backend)));
            return 1;
        }
        loop        = api.g_main_loop_new(nullptr, 0);
        network     = api.new_session();
        cookies     = api.cookie_manager(network);
        content     = api.webkit_user_content_manager_new();
        cancellable = api.g_cancellable_new();
        if (! add_scripts()) {
            error("scripts"_str, "Could not install browser scripts"_str);
            return 1;
        }
        auto* view = new_view();
        api.g_signal_connect_data(
            cookies, "changed", reinterpret_cast<Callback>(cookies_changed), this, nullptr, 0);
        int flags = ::fcntl(STDIN_FILENO, F_GETFL);
        if (flags < 0 || ::fcntl(STDIN_FILENO, F_SETFL, flags | O_NONBLOCK) < 0) {
            error("input"_str, "Could not configure browser command input"_str);
            return 1;
        }
        input_source = api.g_timeout_add(20, read_input, this);
        auto url     = cstring(text(request, "url"_str));
        api.webkit_web_view_load_uri(view->widget, url.as_ptr());
        api.present(view->window);
        if (! output(event("ready"_str))) stop(true);
        if (! closing) api.g_main_loop_run(loop);
        return failed ? 1 : 0;
    }
};

auto first_request() -> Option<Value> {
    Vec<u8> line;
    char    byte;
    for (;;) {
        auto count = ::read(STDIN_FILENO, &byte, 1);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) {
            error("input"_str, "Browser open request was not received"_str);
            return None();
        }
        if (byte == '\n') break;
        if (line.len() >= max_frame) {
            error("input_limit"_str, "Browser protocol frame exceeds 1 MiB"_str);
            return None();
        }
        line.push(u8(static_cast<unsigned char>(byte)));
    }
    auto parsed = rstd::json::from_slice(line.as_slice());
    if (parsed.is_err() || ! parsed->as_object()) {
        error("invalid_json"_str, "Invalid browser protocol JSON"_str);
        return None();
    }
    if ((*parsed)["version"_str].as_i64() != Some(i64(1))) {
        error("protocol_version"_str, "Unsupported browser protocol version"_str);
        return None();
    }
    if (text(*parsed, "command"_str) != "open"_str) {
        error("invalid_command"_str, "The first browser command must be open"_str);
        return None();
    }
    return Some(rstd::move(parsed).unwrap());
}

auto availability(Option<Backend> backend, bool available, ref<str> failure = ""_str) -> Value {
    Value value;
    value["version"_str]   = Value::Number(rstd::json::Number::from_u64(u64(1)));
    value["available"_str] = Value::Bool(available);
    if (backend) value["backend"_str] = string(backend_name(*backend));
    if (! available) value["error"_str] = string(failure);
    return value;
}

auto probe(Backend backend) -> Value {
    auto flag    = rstd::format("--backend={}", backend_name(backend));
    auto command = rstd::process::Command::make("/proc/self/exe"_str);
    command.arg(flag.as_str()).arg("--check"_str).set_stdin(rstd::process::Stdio::null());
    auto result = command.output();
    if (result.is_err())
        return availability(Some(backend),
                            false,
                            rstd::format("{} probe could not be started: {}",
                                         backend_label(backend),
                                         rstd::move(result).unwrap_err()));
    if (result->stdout_buf.len() <= max_frame) {
        auto parsed = rstd::json::from_slice(result->stdout_buf.as_slice());
        if (parsed.is_ok() && (*parsed)["version"_str] == u64(1) &&
            (*parsed)["available"_str].as_bool().is_some() &&
            (*parsed)["backend"_str].as_str() == Some(backend_name(backend))) {
            if (result->status.success() && (*parsed)["available"_str] == true)
                return rstd::move(parsed).unwrap();
            if ((*parsed)["available"_str] == false && (*parsed)["error"_str].as_str())
                return rstd::move(parsed).unwrap();
        }
    }
    return availability(Some(backend),
                        false,
                        rstd::format("{} probe exited without a valid availability result",
                                     backend_label(backend)));
}
} // namespace

int run(int argc, char** argv) {
    ::signal(SIGPIPE, SIG_IGN);
    bool            check = false, selected = false;
    Option<Backend> backend;
    for (int i = 1; i < argc; ++i) {
        const auto argument = ctext(argv[i]);
        if (argument == "--check"_str && ! check)
            check = true;
        else if (! selected &&
                 (argument == "--backend=auto"_str || argument == "--backend=gtk4"_str ||
                  argument == "--backend=gtk3"_str)) {
            selected = true;
            if (argument == "--backend=gtk4"_str)
                backend = Some(Backend::Gtk4);
            else if (argument == "--backend=gtk3"_str)
                backend = Some(Backend::Gtk3);
        } else {
            error("arguments"_str,
                  "Usage: waywallen-web-helper [--backend=auto|gtk4|gtk3] [--check]"_str);
            return 2;
        }
    }
    Option<Value> request;
    if (! check) {
        request = first_request();
        if (! request) return 2;
    }
    if (! backend) {
        // A failed load can still register GTK globals; keep candidate probes isolated.
        String failures;
        for (auto candidate : { Backend::Gtk4, Backend::Gtk3 }) {
            auto result = probe(candidate);
            if (result["available"_str] == true) {
                backend = Some(candidate);
                if (check) {
                    output(result);
                    return 0;
                }
                break;
            }
            if (! failures.is_empty()) failures.push_str("; "_str);
            failures.push_str(text(result, "error"_str));
        }
        if (! backend) {
            if (check)
                output(availability(None(), false, failures));
            else
                error("runtime_unavailable"_str, failures);
            return 1;
        }
    }
    // Never retry another backend here. GObject registrations live until process exit.
    auto* api       = new Api(*backend);
    bool  available = api->load();
    if (check) {
        output(availability(backend, available, api->error));
        return available ? 0 : 1;
    }
    if (! available) {
        error("runtime_unavailable"_str, api->error);
        return 1;
    }
    auto* session = new Session(*api, request.take().unwrap());
    return session->run();
}
} // namespace waywallen::web
