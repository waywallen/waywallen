export module waywallen.web:ffi;
export import rstd;
import rstd.dlopn;

using namespace rstd::prelude;
using namespace rstd::literals;

export namespace waywallen::web
{
// GTK 3 and GTK 4 have incompatible entry points and must stay in separate processes.
enum class Backend
{
    Gtk4,
    Gtk3
};

constexpr auto backend_name(Backend backend) -> ref<str> {
    return backend == Backend::Gtk4 ? "gtk4"_str : "gtk3"_str;
}
constexpr auto backend_label(Backend backend) -> ref<str> {
    return backend == Backend::Gtk4 ? "GTK 4 / WebKitGTK 6.0"_str : "GTK 3 / WebKit2GTK 4.1"_str;
}

using Object     = void*;
using Callback   = void (*)();
using AsyncReady = void (*)(Object, Object, void*);
using Source     = int (*)(void*);
using Destroy    = void (*)(void*);

struct List {
    void* data;
    List* next;
    List* previous;
};

struct Api {
    Vec<rstd::dlopn::Library> libraries;
    String                    error;
    Backend                   backend;

    explicit Api(Backend backend): backend(backend) {}

    int (*gtk_init_check)();
    Object (*gtk_window_new)();
    int (*gtk3_init_check)(int*, char***);
    Object (*gtk3_window_new)(int);
    void (*gtk_container_add)(Object, Object);
    void (*gtk_widget_show_all)(Object);
    void (*gtk_widget_destroy)(Object);
    void (*gtk_window_set_title)(Object, const char*);
    void (*gtk_window_set_default_size)(Object, int, int);
    void (*gtk_window_set_child)(Object, Object);
    void (*gtk_window_present)(Object);
    void (*gtk_window_destroy)(Object);
    Object (*g_object_new)(rstd::uintptr_t, const char*, ...);
    void (*g_object_unref)(Object);
    unsigned long (*g_signal_connect_data)(Object, const char*, Callback, void*,
                                           void (*)(void*, void*), int);
    Object (*g_main_loop_new)(Object, int);
    void (*g_main_loop_run)(Object);
    void (*g_main_loop_quit)(Object);
    void (*g_main_loop_unref)(Object);
    unsigned (*g_timeout_add)(unsigned, Source, void*);
    int (*g_source_remove)(unsigned);
    void (*g_free)(void*);
    void (*g_error_free)(Object);
    void (*g_list_free)(List*);
    Object (*g_cancellable_new)();
    void (*g_cancellable_cancel)(Object);
    Object (*g_uri_parse)(const char*, unsigned, Object*);
    const char* (*g_uri_get_scheme)(Object);
    const char* (*g_uri_get_host)(Object);
    const char* (*g_uri_get_path)(Object);
    const char* (*g_uri_get_query)(Object);
    const char* (*g_uri_get_fragment)(Object);
    const char* (*g_uri_get_userinfo)(Object);
    int (*g_uri_get_port)(Object);
    void (*g_uri_unref)(Object);
    int (*g_ascii_strcasecmp)(const char*, const char*);
    rstd::int64_t (*g_date_time_to_unix)(Object);
    Object (*webkit_network_session_new_ephemeral)();
    Object (*webkit_network_session_get_cookie_manager)(Object);
    Object (*webkit_website_data_manager_new_ephemeral)();
    Object (*webkit_website_data_manager_get_cookie_manager)(Object);
    Object (*webkit_web_context_new_with_website_data_manager)(Object);
    rstd::uintptr_t (*webkit_web_view_get_type)();
    Object (*webkit_user_content_manager_new)();
    int (*webkit_user_content_manager_register_script_message_handler)(Object, const char*,
                                                                       const char*);
    int (*webkit_user_content_manager_register_script_message_handler_in_world)(Object, const char*,
                                                                                const char*);
    Object (*webkit_javascript_result_get_js_value)(Object);
    void (*webkit_user_content_manager_add_script)(Object, Object);
    Object (*webkit_user_script_new_for_world)(const char*, int, int, const char*,
                                               const char* const*, const char* const*);
    void (*webkit_user_script_unref)(Object);
    void (*webkit_web_view_load_uri)(Object, const char*);
    const char* (*webkit_web_view_get_uri)(Object);
    Object (*webkit_web_view_get_settings)(Object);
    void (*webkit_web_view_stop_loading)(Object);
    void (*webkit_settings_set_enable_javascript)(Object, int);
    Object (*webkit_navigation_policy_decision_get_navigation_action)(Object);
    Object (*webkit_navigation_action_get_request)(Object);
    int (*webkit_navigation_action_is_user_gesture)(Object);
    const char* (*webkit_uri_request_get_uri)(Object);
    void (*webkit_policy_decision_ignore)(Object);
    void (*webkit_policy_decision_use)(Object);
    void (*webkit_cookie_manager_get_cookies)(Object, const char*, Object, AsyncReady, void*);
    List* (*webkit_cookie_manager_get_cookies_finish)(Object, Object, Object*);
    void (*webkit_web_view_evaluate_javascript)(Object, const char*, rstd::ptrdiff_t, const char*,
                                                const char*, Object, AsyncReady, void*);
    Object (*webkit_web_view_evaluate_javascript_finish)(Object, Object, Object*);
    char* (*jsc_value_to_json)(Object, unsigned);
    int (*jsc_value_is_undefined)(Object);
    const char* (*soup_cookie_get_name)(Object);
    const char* (*soup_cookie_get_value)(Object);
    const char* (*soup_cookie_get_domain)(Object);
    const char* (*soup_cookie_get_path)(Object);
    int (*soup_cookie_get_secure)(Object);
    int (*soup_cookie_get_http_only)(Object);
    Object (*soup_cookie_get_expires)(Object);
    int (*soup_cookie_get_same_site_policy)(Object);
    void (*soup_cookie_free)(Object);

    bool load() {
        const char* library =
            backend == Backend::Gtk4 ? "libwebkitgtk-6.0.so.4" : "libwebkit2gtk-4.1.so.0";
        auto opened = rstd::dlopn::Library::open(rstd::ffi::CStr::from_ptr(library));
        if (opened.is_err()) {
            error = rstd::format("{} is unavailable: {}: {}",
                                 backend_label(backend),
                                 library,
                                 rstd::move(opened).unwrap_err().message());
            return false;
        }
        libraries.push(rstd::move(opened).unwrap());
#define WW_LOAD_AS(member, symbol_name)                                                          \
    do {                                                                                         \
        auto value =                                                                             \
            libraries[usize()].symbol<decltype(member)>(rstd::ffi::CStr::from_ptr(symbol_name)); \
        if (value.is_err()) {                                                                    \
            error = rstd::format("{} is missing required symbol {}: {}",                         \
                                 backend_label(backend),                                         \
                                 symbol_name,                                                    \
                                 rstd::move(value).unwrap_err().message());                      \
            return false;                                                                        \
        }                                                                                        \
        member = rstd::move(value).unwrap();                                                     \
    } while (false)
#define WW_LOAD(name) WW_LOAD_AS(name, #name)
        if (backend == Backend::Gtk4) {
            WW_LOAD(gtk_init_check);
            WW_LOAD(gtk_window_new);
            WW_LOAD(gtk_window_set_child);
            WW_LOAD(gtk_window_destroy);
            WW_LOAD(webkit_network_session_new_ephemeral);
            WW_LOAD(webkit_network_session_get_cookie_manager);
            WW_LOAD(webkit_user_content_manager_register_script_message_handler);
        } else {
            WW_LOAD_AS(gtk3_init_check, "gtk_init_check");
            WW_LOAD_AS(gtk3_window_new, "gtk_window_new");
            WW_LOAD(gtk_container_add);
            WW_LOAD(gtk_widget_show_all);
            WW_LOAD(gtk_widget_destroy);
            WW_LOAD(webkit_website_data_manager_new_ephemeral);
            WW_LOAD(webkit_website_data_manager_get_cookie_manager);
            WW_LOAD(webkit_web_context_new_with_website_data_manager);
            WW_LOAD(webkit_user_content_manager_register_script_message_handler_in_world);
            WW_LOAD(webkit_javascript_result_get_js_value);
        }
        WW_LOAD(gtk_window_set_title);
        WW_LOAD(gtk_window_set_default_size);
        WW_LOAD(gtk_window_present);
        WW_LOAD(g_object_new);
        WW_LOAD(g_object_unref);
        WW_LOAD(g_signal_connect_data);
        WW_LOAD(g_main_loop_new);
        WW_LOAD(g_main_loop_run);
        WW_LOAD(g_main_loop_quit);
        WW_LOAD(g_main_loop_unref);
        WW_LOAD(g_timeout_add);
        WW_LOAD(g_source_remove);
        WW_LOAD(g_free);
        WW_LOAD(g_error_free);
        WW_LOAD(g_list_free);
        WW_LOAD(g_cancellable_new);
        WW_LOAD(g_cancellable_cancel);
        WW_LOAD(g_uri_parse);
        WW_LOAD(g_uri_get_scheme);
        WW_LOAD(g_uri_get_host);
        WW_LOAD(g_uri_get_path);
        WW_LOAD(g_uri_get_query);
        WW_LOAD(g_uri_get_fragment);
        WW_LOAD(g_uri_get_userinfo);
        WW_LOAD(g_uri_get_port);
        WW_LOAD(g_uri_unref);
        WW_LOAD(g_ascii_strcasecmp);
        WW_LOAD(g_date_time_to_unix);
        WW_LOAD(webkit_web_view_get_type);
        WW_LOAD(webkit_user_content_manager_new);
        WW_LOAD(webkit_user_content_manager_add_script);
        WW_LOAD(webkit_user_script_new_for_world);
        WW_LOAD(webkit_user_script_unref);
        WW_LOAD(webkit_web_view_load_uri);
        WW_LOAD(webkit_web_view_get_uri);
        WW_LOAD(webkit_web_view_get_settings);
        WW_LOAD(webkit_web_view_stop_loading);
        WW_LOAD(webkit_settings_set_enable_javascript);
        WW_LOAD(webkit_navigation_policy_decision_get_navigation_action);
        WW_LOAD(webkit_navigation_action_get_request);
        WW_LOAD(webkit_navigation_action_is_user_gesture);
        WW_LOAD(webkit_uri_request_get_uri);
        WW_LOAD(webkit_policy_decision_ignore);
        WW_LOAD(webkit_policy_decision_use);
        WW_LOAD(webkit_cookie_manager_get_cookies);
        WW_LOAD(webkit_cookie_manager_get_cookies_finish);
        WW_LOAD(webkit_web_view_evaluate_javascript);
        WW_LOAD(webkit_web_view_evaluate_javascript_finish);
        WW_LOAD(jsc_value_to_json);
        WW_LOAD(jsc_value_is_undefined);
        WW_LOAD(soup_cookie_get_name);
        WW_LOAD(soup_cookie_get_value);
        WW_LOAD(soup_cookie_get_domain);
        WW_LOAD(soup_cookie_get_path);
        WW_LOAD(soup_cookie_get_secure);
        WW_LOAD(soup_cookie_get_http_only);
        WW_LOAD(soup_cookie_get_expires);
        WW_LOAD(soup_cookie_get_same_site_policy);
        WW_LOAD(soup_cookie_free);
#undef WW_LOAD
#undef WW_LOAD_AS
        error.clear();
        return true;
    }

    bool initialize() const {
        return backend == Backend::Gtk4 ? gtk_init_check() : gtk3_init_check(nullptr, nullptr);
    }
    Object new_window() const {
        return backend == Backend::Gtk4 ? gtk_window_new() : gtk3_window_new(0);
    }
    void attach(Object window, Object child) const {
        if (backend == Backend::Gtk4)
            gtk_window_set_child(window, child);
        else
            gtk_container_add(window, child);
    }
    void present(Object window) const {
        if (backend == Backend::Gtk3) gtk_widget_show_all(window);
        gtk_window_present(window);
    }
    void destroy(Object window) const {
        if (backend == Backend::Gtk4)
            gtk_window_destroy(window);
        else
            gtk_widget_destroy(window);
    }
    void on_close(Object window, Callback gtk4_callback, Callback gtk3_callback, void* data) const {
        g_signal_connect_data(window,
                              backend == Backend::Gtk4 ? "close-request" : "delete-event",
                              backend == Backend::Gtk4 ? gtk4_callback : gtk3_callback,
                              data,
                              nullptr,
                              0);
    }
    Object new_session() const {
        return backend == Backend::Gtk4 ? webkit_network_session_new_ephemeral()
                                        : webkit_website_data_manager_new_ephemeral();
    }
    Object cookie_manager(Object session) const {
        return backend == Backend::Gtk4 ? webkit_network_session_get_cookie_manager(session)
                                        : webkit_website_data_manager_get_cookie_manager(session);
    }
    Object new_web_view(Object session, Object content, Object related) const {
        if (related)
            return g_object_new(webkit_web_view_get_type(),
                                "related-view",
                                related,
                                "user-content-manager",
                                content,
                                nullptr);
        if (backend == Backend::Gtk4)
            return g_object_new(webkit_web_view_get_type(),
                                "network-session",
                                session,
                                "user-content-manager",
                                content,
                                nullptr);
        auto context = webkit_web_context_new_with_website_data_manager(session);
        auto view    = g_object_new(webkit_web_view_get_type(),
                                    "web-context",
                                    context,
                                    "user-content-manager",
                                    content,
                                    nullptr);
        g_object_unref(context);
        return view;
    }
    bool register_messages(Object manager, const char* name, const char* world) const {
        return backend == Backend::Gtk4
                   ? webkit_user_content_manager_register_script_message_handler(
                         manager, name, world)
                   : webkit_user_content_manager_register_script_message_handler_in_world(
                         manager, name, world);
    }
    Object message_value(Object value) const {
        return backend == Backend::Gtk4 ? value : webkit_javascript_result_get_js_value(value);
    }
};
} // namespace waywallen::web
