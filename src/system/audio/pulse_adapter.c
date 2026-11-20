#include "pulse_adapter.h"

#include <pulse/pulseaudio.h>

#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define WW_PULSE_SONAME        "libpulse.so.0"
#define WW_PULSE_RING_FRAMES   (48000u * 2u)
#define WW_PULSE_NAME_CAPACITY 512u

struct pulse_api {
    __typeof__(&pa_threaded_mainloop_new)            threaded_mainloop_new;
    __typeof__(&pa_threaded_mainloop_free)           threaded_mainloop_free;
    __typeof__(&pa_threaded_mainloop_start)          threaded_mainloop_start;
    __typeof__(&pa_threaded_mainloop_stop)           threaded_mainloop_stop;
    __typeof__(&pa_threaded_mainloop_lock)           threaded_mainloop_lock;
    __typeof__(&pa_threaded_mainloop_unlock)         threaded_mainloop_unlock;
    __typeof__(&pa_threaded_mainloop_wait)           threaded_mainloop_wait;
    __typeof__(&pa_threaded_mainloop_signal)         threaded_mainloop_signal;
    __typeof__(&pa_threaded_mainloop_get_api)        threaded_mainloop_get_api;
    __typeof__(&pa_context_new)                      context_new;
    __typeof__(&pa_context_new_with_proplist)        context_new_with_proplist;
    __typeof__(&pa_context_set_state_callback)       context_set_state_callback;
    __typeof__(&pa_context_connect)                  context_connect;
    __typeof__(&pa_context_disconnect)               context_disconnect;
    __typeof__(&pa_context_unref)                    context_unref;
    __typeof__(&pa_context_get_state)                context_get_state;
    __typeof__(&pa_context_errno)                    context_errno;
    __typeof__(&pa_context_get_server_info)          context_get_server_info;
    __typeof__(&pa_context_get_sink_info_by_name)    context_get_sink_info_by_name;
    __typeof__(&pa_context_get_sink_input_info_list) context_get_sink_input_info_list;
    __typeof__(&pa_context_set_subscribe_callback)   context_set_subscribe_callback;
    __typeof__(&pa_context_subscribe)                context_subscribe;
    __typeof__(&pa_operation_unref)                  operation_unref;
    __typeof__(&pa_proplist_new)                     proplist_new;
    __typeof__(&pa_proplist_sets)                    proplist_sets;
    __typeof__(&pa_proplist_gets)                    proplist_gets;
    __typeof__(&pa_proplist_free)                    proplist_free;
    __typeof__(&pa_stream_new)                       stream_new;
    __typeof__(&pa_stream_set_state_callback)        stream_set_state_callback;
    __typeof__(&pa_stream_set_read_callback)         stream_set_read_callback;
    __typeof__(&pa_stream_connect_record)            stream_connect_record;
    __typeof__(&pa_stream_get_state)                 stream_get_state;
    __typeof__(&pa_stream_get_sample_spec)           stream_get_sample_spec;
    __typeof__(&pa_stream_get_channel_map)           stream_get_channel_map;
    __typeof__(&pa_stream_readable_size)             stream_readable_size;
    __typeof__(&pa_stream_peek)                      stream_peek;
    __typeof__(&pa_stream_drop)                      stream_drop;
    __typeof__(&pa_stream_disconnect)                stream_disconnect;
    __typeof__(&pa_stream_unref)                     stream_unref;
    __typeof__(&pa_strerror)                         strerror_fn;
};

static pthread_mutex_t  api_cache_mutex = PTHREAD_MUTEX_INITIALIZER;
static void*            api_cache_library;
static struct pulse_api api_cache_capture;
static struct pulse_api api_cache_observer;
static int              api_cache_capture_ready;
static int              api_cache_observer_ready;
static _Atomic uint64_t generation_counter;

struct ww_pulse_capture {
    void*                 library;
    struct pulse_api      api;
    pa_threaded_mainloop* loop;
    int                   loop_started;
    pa_context*           context;
    pa_stream*            stream;

    pthread_mutex_t state_mutex;
    float*          ring;
    size_t          ring_read;
    size_t          ring_length;
    uint64_t        generation;
    int             stream_active;
    int             failed;
    char            failure[256];

    char default_sink[WW_PULSE_NAME_CAPACITY];
    /* Output the user picked; empty means "follow the default sink". */
    char preferred_sink[WW_PULSE_NAME_CAPACITY];
    char monitor_source[WW_PULSE_NAME_CAPACITY];
    char queried_monitor[WW_PULSE_NAME_CAPACITY];
    int  query_pending;
    int  query_found_sink;
    int  query_using_preferred;
};

struct ww_pulse_playback_observer {
    void*                 library;
    struct pulse_api      api;
    pa_threaded_mainloop* loop;
    int                   loop_started;
    pa_context*           context;

    pthread_mutex_t             state_mutex;
    ww_pulse_playback_stream_t* streams;
    size_t                      stream_count;
    size_t                      stream_capacity;
    ww_pulse_playback_stream_t* resync_streams;
    size_t                      resync_stream_count;
    size_t                      resync_stream_capacity;
    int                         resync_pending;
    int                         resync_dirty;
    int                         initial_ready;
    int                         failed;
    char                        failure[256];
};

static void copy_error(char* out, size_t capacity, const char* message) {
    if (! out || capacity == 0) return;
    snprintf(out, capacity, "%s", message ? message : "");
}

static void set_failure(ww_pulse_capture_t* self, int code, const char* message) {
    pthread_mutex_lock(&self->state_mutex);
    self->failed = code;
    snprintf(self->failure, sizeof(self->failure), "%s", message ? message : "PulseAudio error");
    pthread_mutex_unlock(&self->state_mutex);
}

static int load_api(void** target_library, struct pulse_api* target_api, int observer, char* error,
                    size_t error_capacity) {
    pthread_mutex_lock(&api_cache_mutex);
    if ((! observer && api_cache_capture_ready) || (observer && api_cache_observer_ready)) {
        *target_library = api_cache_library;
        *target_api     = observer ? api_cache_observer : api_cache_capture;
        pthread_mutex_unlock(&api_cache_mutex);
        return WW_PULSE_OK;
    }

    void* library = api_cache_library;
    if (! library) {
        library = dlopen(WW_PULSE_SONAME, RTLD_NOW | RTLD_LOCAL);
        if (! library) {
            copy_error(error, error_capacity, dlerror());
            pthread_mutex_unlock(&api_cache_mutex);
            return WW_PULSE_LIBRARY_UNAVAILABLE;
        }
        api_cache_library = library;
    }
    struct pulse_api api = { 0 };

#define LOAD(field, symbol)                                                              \
    do {                                                                                 \
        void* address = dlsym(library, #symbol);                                         \
        if (! address) {                                                                 \
            char message[256];                                                           \
            snprintf(message, sizeof(message), "missing PulseAudio symbol %s", #symbol); \
            copy_error(error, error_capacity, message);                                  \
            pthread_mutex_unlock(&api_cache_mutex);                                      \
            return WW_PULSE_MISSING_SYMBOL;                                              \
        }                                                                                \
        memcpy(&api.field, &address, sizeof(address));                                   \
    } while (0)

#define LOAD_OPTIONAL(field, symbol)                                \
    do {                                                            \
        void* address = dlsym(library, #symbol);                    \
        if (address) memcpy(&api.field, &address, sizeof(address)); \
    } while (0)

    LOAD(threaded_mainloop_new, pa_threaded_mainloop_new);
    LOAD(threaded_mainloop_free, pa_threaded_mainloop_free);
    LOAD(threaded_mainloop_start, pa_threaded_mainloop_start);
    LOAD(threaded_mainloop_stop, pa_threaded_mainloop_stop);
    LOAD(threaded_mainloop_lock, pa_threaded_mainloop_lock);
    LOAD(threaded_mainloop_unlock, pa_threaded_mainloop_unlock);
    LOAD(threaded_mainloop_wait, pa_threaded_mainloop_wait);
    LOAD(threaded_mainloop_signal, pa_threaded_mainloop_signal);
    LOAD(threaded_mainloop_get_api, pa_threaded_mainloop_get_api);
    LOAD(context_new, pa_context_new);
    LOAD_OPTIONAL(context_new_with_proplist, pa_context_new_with_proplist);
    LOAD(context_set_state_callback, pa_context_set_state_callback);
    LOAD(context_connect, pa_context_connect);
    LOAD(context_disconnect, pa_context_disconnect);
    LOAD(context_unref, pa_context_unref);
    LOAD(context_get_state, pa_context_get_state);
    LOAD(context_errno, pa_context_errno);
    LOAD(context_set_subscribe_callback, pa_context_set_subscribe_callback);
    LOAD(context_subscribe, pa_context_subscribe);
    LOAD(operation_unref, pa_operation_unref);
    LOAD_OPTIONAL(proplist_new, pa_proplist_new);
    LOAD_OPTIONAL(proplist_sets, pa_proplist_sets);
    LOAD_OPTIONAL(proplist_free, pa_proplist_free);
    LOAD(strerror_fn, pa_strerror);
    if (observer) {
        LOAD(context_get_sink_input_info_list, pa_context_get_sink_input_info_list);
        LOAD(proplist_gets, pa_proplist_gets);
    } else {
        LOAD(context_get_server_info, pa_context_get_server_info);
        LOAD(context_get_sink_info_by_name, pa_context_get_sink_info_by_name);
        LOAD(stream_new, pa_stream_new);
        LOAD(stream_set_state_callback, pa_stream_set_state_callback);
        LOAD(stream_set_read_callback, pa_stream_set_read_callback);
        LOAD(stream_connect_record, pa_stream_connect_record);
        LOAD(stream_get_state, pa_stream_get_state);
        LOAD(stream_get_sample_spec, pa_stream_get_sample_spec);
        LOAD(stream_get_channel_map, pa_stream_get_channel_map);
        LOAD(stream_readable_size, pa_stream_readable_size);
        LOAD(stream_peek, pa_stream_peek);
        LOAD(stream_drop, pa_stream_drop);
        LOAD(stream_disconnect, pa_stream_disconnect);
        LOAD(stream_unref, pa_stream_unref);
    }
#undef LOAD
#undef LOAD_OPTIONAL
    if (observer) {
        api_cache_observer       = api;
        api_cache_observer_ready = 1;
    } else {
        api_cache_capture       = api;
        api_cache_capture_ready = 1;
    }
    *target_library = library;
    *target_api     = api;
    pthread_mutex_unlock(&api_cache_mutex);
    return WW_PULSE_OK;
}

static pa_context* new_daemon_context(struct pulse_api* api, pa_threaded_mainloop* loop) {
    if (! api->context_new_with_proplist || ! api->proplist_new || ! api->proplist_sets ||
        ! api->proplist_free) {
        return api->context_new(api->threaded_mainloop_get_api(loop), "Waywallen Daemon");
    }
    pa_proplist* properties = api->proplist_new();
    if (! properties)
        return api->context_new(api->threaded_mainloop_get_api(loop), "Waywallen Daemon");
    if (api->proplist_sets(properties, PA_PROP_APPLICATION_NAME, "Waywallen Daemon") < 0 ||
        api->proplist_sets(properties, PA_PROP_APPLICATION_ID, "org.waywallen.daemon") < 0) {
        api->proplist_free(properties);
        return api->context_new(api->threaded_mainloop_get_api(loop), "Waywallen Daemon");
    }
    pa_context* context = api->context_new_with_proplist(
        api->threaded_mainloop_get_api(loop), "Waywallen Daemon", properties);
    api->proplist_free(properties);
    return context;
}

static void reset_ring(ww_pulse_capture_t* self) {
    pthread_mutex_lock(&self->state_mutex);
    self->ring_read   = 0;
    self->ring_length = 0;
    pthread_mutex_unlock(&self->state_mutex);
}

static void mark_discontinuity(ww_pulse_capture_t* self) {
    pthread_mutex_lock(&self->state_mutex);
    self->ring_read   = 0;
    self->ring_length = 0;
    self->generation  = atomic_fetch_add_explicit(&generation_counter, 1, memory_order_relaxed) + 1;
    pthread_mutex_unlock(&self->state_mutex);
}

static void destroy_stream_locked(ww_pulse_capture_t* self) {
    if (! self->stream) return;
    self->api.stream_set_state_callback(self->stream, NULL, NULL);
    self->api.stream_set_read_callback(self->stream, NULL, NULL);
    self->api.stream_disconnect(self->stream);
    self->api.stream_unref(self->stream);
    self->stream = NULL;
    pthread_mutex_lock(&self->state_mutex);
    self->stream_active = 0;
    pthread_mutex_unlock(&self->state_mutex);
}

static void on_stream_state(pa_stream* stream, void* userdata);
static void on_stream_read(pa_stream* stream, size_t bytes, void* userdata);

static int create_stream_locked(ww_pulse_capture_t* self, const char* monitor) {
    destroy_stream_locked(self);
    reset_ring(self);

    pa_sample_spec sample_spec = {
        .format   = PA_SAMPLE_FLOAT32LE,
        .rate     = 48000,
        .channels = 2,
    };
    pa_channel_map channel_map = {
        .channels = 2,
        .map      = { PA_CHANNEL_POSITION_FRONT_LEFT, PA_CHANNEL_POSITION_FRONT_RIGHT },
    };
    self->stream = self->api.stream_new(
        self->context, "waywallen.daemon.audio-response.capture", &sample_spec, &channel_map);
    if (! self->stream) return -1;

    self->api.stream_set_state_callback(self->stream, on_stream_state, self);
    self->api.stream_set_read_callback(self->stream, on_stream_read, self);
    const uint32_t frame_bytes = 2u * (uint32_t)sizeof(float);
    pa_buffer_attr attr        = {
        .maxlength = (uint32_t)-1,
        .tlength   = (uint32_t)-1,
        .prebuf    = (uint32_t)-1,
        .minreq    = (uint32_t)-1,
        .fragsize  = 1024u * frame_bytes,
    };
    if (self->api.stream_connect_record(self->stream, monitor, &attr, PA_STREAM_ADJUST_LATENCY) <
        0) {
        destroy_stream_locked(self);
        return -1;
    }
    snprintf(self->monitor_source, sizeof(self->monitor_source), "%s", monitor);
    return 0;
}

static void on_context_state(pa_context* context, void* userdata) {
    ww_pulse_capture_t*      self  = userdata;
    const pa_context_state_t state = self->api.context_get_state(context);
    if (! PA_CONTEXT_IS_GOOD(state)) {
        set_failure(self, WW_PULSE_SERVER_UNAVAILABLE, "PulseAudio context failed");
    }
    self->api.threaded_mainloop_signal(self->loop, 0);
}

static void on_stream_state(pa_stream* stream, void* userdata) {
    ww_pulse_capture_t*     self  = userdata;
    const pa_stream_state_t state = self->api.stream_get_state(stream);
    if (state == PA_STREAM_READY) {
        const pa_sample_spec* spec = self->api.stream_get_sample_spec(stream);
        const pa_channel_map* map  = self->api.stream_get_channel_map(stream);
        if (! spec || spec->format != PA_SAMPLE_FLOAT32LE || spec->rate != 48000 ||
            spec->channels != 2 || ! map || map->channels != 2 ||
            map->map[0] != PA_CHANNEL_POSITION_FRONT_LEFT ||
            map->map[1] != PA_CHANNEL_POSITION_FRONT_RIGHT) {
            set_failure(
                self, WW_PULSE_STREAM_FAILED, "PulseAudio stream negotiated an unsupported format");
            self->api.threaded_mainloop_signal(self->loop, 0);
            return;
        }
        pthread_mutex_lock(&self->state_mutex);
        if (! self->stream_active) {
            self->stream_active = 1;
            self->generation =
                atomic_fetch_add_explicit(&generation_counter, 1, memory_order_relaxed) + 1;
            self->failed     = WW_PULSE_OK;
            self->failure[0] = '\0';
        }
        pthread_mutex_unlock(&self->state_mutex);
    } else if (! PA_STREAM_IS_GOOD(state)) {
        set_failure(self, WW_PULSE_STREAM_FAILED, "PulseAudio record stream failed");
    }
    self->api.threaded_mainloop_signal(self->loop, 0);
}

static void ring_write(ww_pulse_capture_t* self, const float* samples, size_t frames) {
    if (frames == 0) return;
    if (frames > WW_PULSE_RING_FRAMES) {
        samples += (frames - WW_PULSE_RING_FRAMES) * 2u;
        frames = WW_PULSE_RING_FRAMES;
        mark_discontinuity(self);
    }

    pthread_mutex_lock(&self->state_mutex);
    const size_t overflow = self->ring_length + frames > WW_PULSE_RING_FRAMES
                                ? self->ring_length + frames - WW_PULSE_RING_FRAMES
                                : 0;
    if (overflow > 0) {
        self->ring_read   = 0;
        self->ring_length = 0;
        self->generation =
            atomic_fetch_add_explicit(&generation_counter, 1, memory_order_relaxed) + 1;
    }
    size_t write = (self->ring_read + self->ring_length) % WW_PULSE_RING_FRAMES;
    for (size_t frame = 0; frame < frames; ++frame) {
        self->ring[write * 2u]      = samples[frame * 2u];
        self->ring[write * 2u + 1u] = samples[frame * 2u + 1u];
        write                       = (write + 1u) % WW_PULSE_RING_FRAMES;
    }
    self->ring_length += frames;
    pthread_mutex_unlock(&self->state_mutex);
}

static void on_stream_read(pa_stream* stream, size_t bytes, void* userdata) {
    (void)bytes;
    ww_pulse_capture_t* self = userdata;
    for (;;) {
        const size_t readable = self->api.stream_readable_size(stream);
        if (readable == 0) return;
        if (readable == (size_t)-1) {
            set_failure(self, WW_PULSE_STREAM_FAILED, "failed to query PulseAudio readable size");
            return;
        }
        const void* data = NULL;
        size_t      size = 0;
        if (self->api.stream_peek(stream, &data, &size) < 0) {
            set_failure(self, WW_PULSE_STREAM_FAILED, "failed to read PulseAudio stream");
            return;
        }
        if (size == 0) return;
        if (data && size % (2u * sizeof(float)) != 0) {
            set_failure(
                self, WW_PULSE_STREAM_FAILED, "PulseAudio stream returned a partial stereo frame");
        } else if (data) {
            ring_write(self, data, size / (2u * sizeof(float)));
        } else {
            mark_discontinuity(self);
        }
        if (self->api.stream_drop(stream) < 0) {
            set_failure(self, WW_PULSE_STREAM_FAILED, "failed to drop PulseAudio stream data");
            return;
        }
    }
}

static void on_sink_info(pa_context* context, const pa_sink_info* info, int eol, void* userdata) {
    ww_pulse_capture_t* self = userdata;
    if (info && info->monitor_source_name) {
        snprintf(
            self->queried_monitor, sizeof(self->queried_monitor), "%s", info->monitor_source_name);
        self->query_found_sink = 1;
    }
    if (eol == 0) return;

    if (! self->query_found_sink && self->query_using_preferred && self->default_sink[0] != '\0') {
        /* The chosen output is gone (for example unplugged): follow the default
         * sink instead. The next sink event retries the chosen one. */
        self->query_using_preferred = 0;
        pa_operation* operation     = self->api.context_get_sink_info_by_name(
            context, self->default_sink, on_sink_info, self);
        if (operation) {
            self->api.operation_unref(operation);
            return;
        }
    }

    self->query_pending = 0;
    if (! self->query_found_sink || self->queried_monitor[0] == '\0') {
        set_failure(
            self, WW_PULSE_MONITOR_UNAVAILABLE, "default PulseAudio sink has no monitor source");
        self->api.threaded_mainloop_signal(self->loop, 0);
        return;
    }
    if (self->query_found_sink && self->queried_monitor[0] != '\0' &&
        strcmp(self->queried_monitor, self->monitor_source) != 0 && self->stream) {
        if (create_stream_locked(self, self->queried_monitor) < 0) {
            set_failure(self, WW_PULSE_STREAM_FAILED, "failed to switch PulseAudio monitor source");
        }
    }
    self->api.threaded_mainloop_signal(self->loop, 0);
}

static void on_server_info(pa_context* context, const pa_server_info* info, void* userdata) {
    ww_pulse_capture_t* self = userdata;
    if (! info || ! info->default_sink_name) {
        self->query_pending = 0;
        set_failure(self, WW_PULSE_MONITOR_UNAVAILABLE, "PulseAudio server has no default sink");
        self->api.threaded_mainloop_signal(self->loop, 0);
        return;
    }
    snprintf(self->default_sink, sizeof(self->default_sink), "%s", info->default_sink_name);
    self->query_using_preferred = self->preferred_sink[0] != '\0';
    pa_operation* operation     = self->api.context_get_sink_info_by_name(
        context,
        self->query_using_preferred ? self->preferred_sink : self->default_sink,
        on_sink_info,
        self);
    if (! operation) {
        self->query_pending = 0;
        self->api.threaded_mainloop_signal(self->loop, 0);
        return;
    }
    self->api.operation_unref(operation);
}

static int query_monitor_locked(ww_pulse_capture_t* self) {
    if (self->query_pending) return 0;
    self->query_pending      = 1;
    self->query_found_sink   = 0;
    self->queried_monitor[0] = '\0';
    pa_operation* operation =
        self->api.context_get_server_info(self->context, on_server_info, self);
    if (! operation) {
        self->query_pending = 0;
        return -1;
    }
    self->api.operation_unref(operation);
    return 0;
}

static void on_subscription(pa_context* context, pa_subscription_event_type_t type, uint32_t index,
                            void* userdata) {
    (void)context;
    (void)index;
    const pa_subscription_event_type_t facility = type & PA_SUBSCRIPTION_EVENT_FACILITY_MASK;
    if (facility != PA_SUBSCRIPTION_EVENT_SERVER && facility != PA_SUBSCRIPTION_EVENT_SINK) return;
    ww_pulse_capture_t* self = userdata;
    if (query_monitor_locked(self) < 0) {
        set_failure(
            self, WW_PULSE_MONITOR_UNAVAILABLE, "failed to refresh PulseAudio monitor source");
    }
}

static void destroy_locked(ww_pulse_capture_t* self) {
    destroy_stream_locked(self);
    if (self->context) {
        self->api.context_set_subscribe_callback(self->context, NULL, NULL);
        self->api.context_set_state_callback(self->context, NULL, NULL);
        self->api.context_disconnect(self->context);
        self->api.context_unref(self->context);
        self->context = NULL;
    }
}

ww_pulse_capture_t* ww_pulse_capture_open(const char* sink_name, int* error_code, char* error,
                                          size_t error_capacity) {
    if (error_code) *error_code = WW_PULSE_OK;
    copy_error(error, error_capacity, "");

    ww_pulse_capture_t* self = calloc(1, sizeof(*self));
    if (! self) {
        if (error_code) *error_code = WW_PULSE_OUT_OF_MEMORY;
        copy_error(error, error_capacity, "out of memory");
        return NULL;
    }
    if (sink_name && sink_name[0] != '\0') {
        snprintf(self->preferred_sink, sizeof(self->preferred_sink), "%s", sink_name);
    }
    pthread_mutex_init(&self->state_mutex, NULL);
    self->ring = calloc(WW_PULSE_RING_FRAMES * 2u, sizeof(float));
    if (! self->ring) {
        if (error_code) *error_code = WW_PULSE_OUT_OF_MEMORY;
        copy_error(error, error_capacity, "out of memory");
        ww_pulse_capture_close(self);
        return NULL;
    }

    int load = load_api(&self->library, &self->api, 0, error, error_capacity);
    if (load != WW_PULSE_OK) {
        if (error_code) *error_code = load;
        ww_pulse_capture_close(self);
        return NULL;
    }

    self->loop = self->api.threaded_mainloop_new();
    if (! self->loop || self->api.threaded_mainloop_start(self->loop) < 0) {
        if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to start PulseAudio mainloop");
        ww_pulse_capture_close(self);
        return NULL;
    }
    self->loop_started = 1;

    self->api.threaded_mainloop_lock(self->loop);
    self->context = new_daemon_context(&self->api, self->loop);
    if (! self->context) {
        if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to create PulseAudio context");
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_capture_close(self);
        return NULL;
    }
    self->api.context_set_state_callback(self->context, on_context_state, self);
    if (self->api.context_connect(self->context, NULL, PA_CONTEXT_NOFLAGS, NULL) < 0) {
        if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to connect to PulseAudio server");
        destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_capture_close(self);
        return NULL;
    }
    for (;;) {
        const pa_context_state_t state = self->api.context_get_state(self->context);
        if (state == PA_CONTEXT_READY) break;
        if (! PA_CONTEXT_IS_GOOD(state)) {
            char message[256];
            snprintf(message,
                     sizeof(message),
                     "PulseAudio context failed: %s",
                     self->api.strerror_fn(self->api.context_errno(self->context)));
            if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
            copy_error(error, error_capacity, message);
            destroy_locked(self);
            self->api.threaded_mainloop_unlock(self->loop);
            ww_pulse_capture_close(self);
            return NULL;
        }
        self->api.threaded_mainloop_wait(self->loop);
    }

    if (query_monitor_locked(self) < 0) {
        if (error_code) *error_code = WW_PULSE_MONITOR_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to query default PulseAudio sink");
        destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_capture_close(self);
        return NULL;
    }
    while (self->query_pending) self->api.threaded_mainloop_wait(self->loop);
    if (! self->query_found_sink || self->queried_monitor[0] == '\0') {
        if (error_code) *error_code = WW_PULSE_MONITOR_UNAVAILABLE;
        copy_error(error, error_capacity, "default PulseAudio sink has no monitor source");
        destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_capture_close(self);
        return NULL;
    }
    if (create_stream_locked(self, self->queried_monitor) < 0) {
        if (error_code) *error_code = WW_PULSE_STREAM_FAILED;
        copy_error(error, error_capacity, "failed to connect PulseAudio record stream");
        destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_capture_close(self);
        return NULL;
    }
    for (;;) {
        const pa_stream_state_t state = self->api.stream_get_state(self->stream);
        if (state == PA_STREAM_READY) break;
        if (! PA_STREAM_IS_GOOD(state)) {
            if (error_code) *error_code = WW_PULSE_STREAM_FAILED;
            copy_error(error, error_capacity, "PulseAudio record stream failed");
            destroy_locked(self);
            self->api.threaded_mainloop_unlock(self->loop);
            ww_pulse_capture_close(self);
            return NULL;
        }
        self->api.threaded_mainloop_wait(self->loop);
    }
    pthread_mutex_lock(&self->state_mutex);
    const int stream_active = self->stream_active;
    pthread_mutex_unlock(&self->state_mutex);
    if (! stream_active) {
        if (error_code) *error_code = WW_PULSE_STREAM_FAILED;
        copy_error(error, error_capacity, "PulseAudio stream format is unsupported");
        destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_capture_close(self);
        return NULL;
    }

    self->api.context_set_subscribe_callback(self->context, on_subscription, self);
    pa_operation* operation = self->api.context_subscribe(
        self->context, PA_SUBSCRIPTION_MASK_SERVER | PA_SUBSCRIPTION_MASK_SINK, NULL, NULL);
    if (operation) self->api.operation_unref(operation);
    self->api.threaded_mainloop_unlock(self->loop);
    return self;
}

void ww_pulse_capture_close(ww_pulse_capture_t* self) {
    if (! self) return;
    if (self->loop && self->library && self->loop_started) {
        self->api.threaded_mainloop_lock(self->loop);
        destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        self->api.threaded_mainloop_stop(self->loop);
        self->api.threaded_mainloop_free(self->loop);
        self->loop = NULL;
    } else if (self->loop && self->library) {
        self->api.threaded_mainloop_free(self->loop);
        self->loop = NULL;
    }
    free(self->ring);
    pthread_mutex_destroy(&self->state_mutex);
    free(self);
}

size_t ww_pulse_capture_read(ww_pulse_capture_t* self, float* samples, size_t frame_capacity,
                             uint64_t* generation) {
    if (! self || ! samples || frame_capacity == 0) return 0;
    pthread_mutex_lock(&self->state_mutex);
    if (generation) *generation = self->generation;
    const size_t frames = self->ring_length < frame_capacity ? self->ring_length : frame_capacity;
    for (size_t frame = 0; frame < frames; ++frame) {
        const size_t index       = (self->ring_read + frame) % WW_PULSE_RING_FRAMES;
        samples[frame * 2u]      = self->ring[index * 2u];
        samples[frame * 2u + 1u] = self->ring[index * 2u + 1u];
    }
    self->ring_read = (self->ring_read + frames) % WW_PULSE_RING_FRAMES;
    self->ring_length -= frames;
    pthread_mutex_unlock(&self->state_mutex);
    return frames;
}

int ww_pulse_capture_failed(ww_pulse_capture_t* self, char* error, size_t error_capacity) {
    if (! self) return WW_PULSE_SERVER_UNAVAILABLE;
    pthread_mutex_lock(&self->state_mutex);
    const int failed = self->failed;
    copy_error(error, error_capacity, self->failure);
    pthread_mutex_unlock(&self->state_mutex);
    return failed;
}

static void observer_set_failure(ww_pulse_playback_observer_t* self, int code,
                                 const char* message) {
    pthread_mutex_lock(&self->state_mutex);
    self->failed = code;
    snprintf(self->failure, sizeof(self->failure), "%s", message ? message : "PulseAudio error");
    pthread_mutex_unlock(&self->state_mutex);
}

static int32_t playback_process_id(ww_pulse_playback_observer_t* self,
                                   const pa_sink_input_info*     info) {
    if (! info->proplist) return 0;
    const char* value = self->api.proplist_gets(info->proplist, PA_PROP_APPLICATION_PROCESS_ID);
    if (! value || ! *value) return 0;
    errno                   = 0;
    char*               end = NULL;
    const unsigned long pid = strtoul(value, &end, 10);
    if (errno != 0 || end == value || *end != '\0' || pid == 0 || pid > INT32_MAX) return 0;
    return (int32_t)pid;
}

static int playback_has_nonzero_volume(const pa_sink_input_info* info) {
    for (uint8_t channel = 0; channel < info->volume.channels; ++channel) {
        if (info->volume.values[channel] > PA_VOLUME_MUTED) return 1;
    }
    return 0;
}

static int playback_resync_append(ww_pulse_playback_observer_t* self,
                                  const pa_sink_input_info*     info) {
    const ww_pulse_playback_stream_t next = {
        .index              = info->index,
        .process_id         = playback_process_id(self, info),
        .corked             = info->corked != 0,
        .muted              = info->mute != 0,
        .has_nonzero_volume = playback_has_nonzero_volume(info),
    };

    for (size_t i = 0; i < self->resync_stream_count; ++i) {
        if (self->resync_streams[i].index == info->index) {
            self->resync_streams[i] = next;
            return 0;
        }
    }
    if (self->resync_stream_count == self->resync_stream_capacity) {
        const size_t next_capacity =
            self->resync_stream_capacity == 0 ? 16u : self->resync_stream_capacity * 2u;
        void* resized =
            realloc(self->resync_streams, next_capacity * sizeof(*self->resync_streams));
        if (! resized) {
            observer_set_failure(self, WW_PULSE_OUT_OF_MEMORY, "out of memory");
            return -1;
        }
        self->resync_streams         = resized;
        self->resync_stream_capacity = next_capacity;
    }
    self->resync_streams[self->resync_stream_count++] = next;
    return 0;
}

static int playback_start_resync(ww_pulse_playback_observer_t* self);

static void on_playback_info(pa_context* context, const pa_sink_input_info* info, int eol,
                             void* userdata) {
    (void)context;
    ww_pulse_playback_observer_t* self = userdata;
    if (info && playback_resync_append(self, info) < 0) {
        self->api.threaded_mainloop_signal(self->loop, 0);
        return;
    }
    if (eol == 0) return;

    self->resync_pending = 0;
    if (eol < 0) {
        observer_set_failure(
            self, WW_PULSE_SERVER_UNAVAILABLE, "failed to enumerate PulseAudio playback streams");
        self->api.threaded_mainloop_signal(self->loop, 0);
        return;
    }
    if (self->resync_dirty) {
        playback_start_resync(self);
        return;
    }

    pthread_mutex_lock(&self->state_mutex);
    free(self->streams);
    self->streams                = self->resync_streams;
    self->stream_count           = self->resync_stream_count;
    self->stream_capacity        = self->resync_stream_capacity;
    self->resync_streams         = NULL;
    self->resync_stream_count    = 0;
    self->resync_stream_capacity = 0;
    self->initial_ready          = 1;
    pthread_mutex_unlock(&self->state_mutex);
    self->api.threaded_mainloop_signal(self->loop, 0);
}

static int playback_start_resync(ww_pulse_playback_observer_t* self) {
    if (self->resync_pending) {
        self->resync_dirty = 1;
        return 0;
    }
    free(self->resync_streams);
    self->resync_streams         = NULL;
    self->resync_stream_count    = 0;
    self->resync_stream_capacity = 0;
    self->resync_pending         = 1;
    self->resync_dirty           = 0;

    pa_operation* operation =
        self->api.context_get_sink_input_info_list(self->context, on_playback_info, self);
    if (! operation) {
        self->resync_pending = 0;
        observer_set_failure(
            self, WW_PULSE_SERVER_UNAVAILABLE, "failed to enumerate PulseAudio playback streams");
        self->api.threaded_mainloop_signal(self->loop, 0);
        return -1;
    }
    self->api.operation_unref(operation);
    return 0;
}

static void on_playback_context_state(pa_context* context, void* userdata) {
    ww_pulse_playback_observer_t* self  = userdata;
    const pa_context_state_t      state = self->api.context_get_state(context);
    if (! PA_CONTEXT_IS_GOOD(state)) {
        observer_set_failure(self, WW_PULSE_SERVER_UNAVAILABLE, "PulseAudio context failed");
    }
    self->api.threaded_mainloop_signal(self->loop, 0);
}

static void on_playback_subscription(pa_context* context, pa_subscription_event_type_t type,
                                     uint32_t index, void* userdata) {
    (void)context;
    (void)index;
    ww_pulse_playback_observer_t* self = userdata;
    if ((type & PA_SUBSCRIPTION_EVENT_FACILITY_MASK) != PA_SUBSCRIPTION_EVENT_SINK_INPUT) return;
    playback_start_resync(self);
}

static void playback_observer_destroy_locked(ww_pulse_playback_observer_t* self) {
    if (! self->context) return;
    self->api.context_set_subscribe_callback(self->context, NULL, NULL);
    self->api.context_set_state_callback(self->context, NULL, NULL);
    self->api.context_disconnect(self->context);
    self->api.context_unref(self->context);
    self->context = NULL;
}

ww_pulse_playback_observer_t* ww_pulse_playback_observer_open(int* error_code, char* error,
                                                              size_t error_capacity) {
    if (error_code) *error_code = WW_PULSE_OK;
    copy_error(error, error_capacity, "");

    ww_pulse_playback_observer_t* self = calloc(1, sizeof(*self));
    if (! self) {
        if (error_code) *error_code = WW_PULSE_OUT_OF_MEMORY;
        copy_error(error, error_capacity, "out of memory");
        return NULL;
    }
    pthread_mutex_init(&self->state_mutex, NULL);
    const int load = load_api(&self->library, &self->api, 1, error, error_capacity);
    if (load != WW_PULSE_OK) {
        if (error_code) *error_code = load;
        ww_pulse_playback_observer_close(self);
        return NULL;
    }

    self->loop = self->api.threaded_mainloop_new();
    if (! self->loop || self->api.threaded_mainloop_start(self->loop) < 0) {
        if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to start PulseAudio mainloop");
        ww_pulse_playback_observer_close(self);
        return NULL;
    }
    self->loop_started = 1;

    self->api.threaded_mainloop_lock(self->loop);
    self->context = new_daemon_context(&self->api, self->loop);
    if (! self->context) {
        if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to create PulseAudio context");
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_playback_observer_close(self);
        return NULL;
    }
    self->api.context_set_state_callback(self->context, on_playback_context_state, self);
    if (self->api.context_connect(self->context, NULL, PA_CONTEXT_NOFLAGS, NULL) < 0) {
        if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to connect to PulseAudio server");
        playback_observer_destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_playback_observer_close(self);
        return NULL;
    }
    for (;;) {
        const pa_context_state_t state = self->api.context_get_state(self->context);
        if (state == PA_CONTEXT_READY) break;
        if (! PA_CONTEXT_IS_GOOD(state)) {
            if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
            copy_error(error, error_capacity, "PulseAudio context failed");
            playback_observer_destroy_locked(self);
            self->api.threaded_mainloop_unlock(self->loop);
            ww_pulse_playback_observer_close(self);
            return NULL;
        }
        self->api.threaded_mainloop_wait(self->loop);
    }

    self->api.context_set_subscribe_callback(self->context, on_playback_subscription, self);
    pa_operation* subscribe =
        self->api.context_subscribe(self->context, PA_SUBSCRIPTION_MASK_SINK_INPUT, NULL, NULL);
    if (! subscribe) {
        if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to subscribe to PulseAudio playback streams");
        playback_observer_destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_playback_observer_close(self);
        return NULL;
    }
    self->api.operation_unref(subscribe);

    if (playback_start_resync(self) < 0) {
        if (error_code) *error_code = WW_PULSE_SERVER_UNAVAILABLE;
        copy_error(error, error_capacity, "failed to enumerate PulseAudio playback streams");
        playback_observer_destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        ww_pulse_playback_observer_close(self);
        return NULL;
    }
    for (;;) {
        pthread_mutex_lock(&self->state_mutex);
        const int ready  = self->initial_ready;
        const int failed = self->failed;
        pthread_mutex_unlock(&self->state_mutex);
        if (ready || failed != WW_PULSE_OK) break;
        self->api.threaded_mainloop_wait(self->loop);
    }
    self->api.threaded_mainloop_unlock(self->loop);

    pthread_mutex_lock(&self->state_mutex);
    const int failed = self->failed;
    char      failure[sizeof(self->failure)];
    snprintf(failure, sizeof(failure), "%s", self->failure);
    pthread_mutex_unlock(&self->state_mutex);
    if (failed != WW_PULSE_OK) {
        if (error_code) *error_code = failed;
        copy_error(error, error_capacity, failure);
        ww_pulse_playback_observer_close(self);
        return NULL;
    }
    return self;
}

void ww_pulse_playback_observer_close(ww_pulse_playback_observer_t* self) {
    if (! self) return;
    if (self->loop && self->library && self->loop_started) {
        self->api.threaded_mainloop_lock(self->loop);
        playback_observer_destroy_locked(self);
        self->api.threaded_mainloop_unlock(self->loop);
        self->api.threaded_mainloop_stop(self->loop);
        self->api.threaded_mainloop_free(self->loop);
    } else if (self->loop && self->library) {
        self->api.threaded_mainloop_free(self->loop);
    }
    free(self->streams);
    free(self->resync_streams);
    pthread_mutex_destroy(&self->state_mutex);
    free(self);
}

size_t ww_pulse_playback_observer_snapshot(ww_pulse_playback_observer_t* self,
                                           ww_pulse_playback_stream_t* streams, size_t capacity) {
    if (! self) return 0;
    pthread_mutex_lock(&self->state_mutex);
    const size_t count = self->stream_count;
    if (streams && capacity > 0) {
        const size_t copy_count = count < capacity ? count : capacity;
        memcpy(streams, self->streams, copy_count * sizeof(*streams));
    }
    pthread_mutex_unlock(&self->state_mutex);
    return count;
}

int ww_pulse_playback_observer_failed(ww_pulse_playback_observer_t* self, char* error,
                                      size_t error_capacity) {
    if (! self) return WW_PULSE_SERVER_UNAVAILABLE;
    pthread_mutex_lock(&self->state_mutex);
    const int failed = self->failed;
    copy_error(error, error_capacity, self->failure);
    pthread_mutex_unlock(&self->state_mutex);
    return failed;
}
