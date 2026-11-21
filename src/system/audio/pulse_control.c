#include "pulse_control.h"

#include <pulse/pulseaudio.h>

#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define WW_PULSE_SONAME "libpulse.so.0"

/* Resolved at runtime so the daemon still starts on machines without
 * libpulse. Mirrors the approach of pulse_adapter.c. */
struct control_api {
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
    __typeof__(&pa_context_set_state_callback)       context_set_state_callback;
    __typeof__(&pa_context_connect)                  context_connect;
    __typeof__(&pa_context_disconnect)               context_disconnect;
    __typeof__(&pa_context_unref)                    context_unref;
    __typeof__(&pa_context_get_state)                context_get_state;
    __typeof__(&pa_context_errno)                    context_errno;
    __typeof__(&pa_context_get_server_info)          context_get_server_info;
    __typeof__(&pa_context_get_sink_info_list)       context_get_sink_info_list;
    __typeof__(&pa_context_get_sink_input_info_list) context_get_sink_input_info_list;
    __typeof__(&pa_context_move_sink_input_by_name)  context_move_sink_input_by_name;
    __typeof__(&pa_operation_unref)                  operation_unref;
    __typeof__(&pa_proplist_gets)                    proplist_gets;
    __typeof__(&pa_strerror)                         strerror_fn;
};

static pthread_mutex_t    api_mutex = PTHREAD_MUTEX_INITIALIZER;
static void*              api_library;
static struct control_api api_cached;
static int                api_ready;

typedef struct control {
    struct control_api    api;
    pa_threaded_mainloop* loop;
    pa_context*           context;
    int                   loop_started;
    int                   locked;
    int                   pending;
    int                   failed;

    char default_sink[WW_PULSE_CONTROL_NAME_CAPACITY];

    ww_pulse_sink_t* sinks;
    size_t           sink_capacity;
    size_t           sink_total;

    ww_pulse_playback_stream_t* streams;
    size_t                      stream_capacity;
    size_t                      stream_total;

    size_t moved;
} control_t;

static void copy_error(char* out, size_t capacity, const char* message) {
    if (! out || capacity == 0) return;
    snprintf(out, capacity, "%s", message ? message : "");
}

static int fail_with(int* error_code, char* error, size_t error_capacity, int code,
                     const char* message) {
    if (error_code) *error_code = code;
    copy_error(error, error_capacity, message);
    return -1;
}

static int load_control_api(struct control_api* target, char* error, size_t error_capacity,
                            int* error_code) {
    pthread_mutex_lock(&api_mutex);
    if (api_ready) {
        *target = api_cached;
        pthread_mutex_unlock(&api_mutex);
        return 0;
    }
    void* library = api_library;
    if (! library) {
        library = dlopen(WW_PULSE_SONAME, RTLD_NOW | RTLD_LOCAL);
        if (! library) {
            const int rc = fail_with(
                error_code, error, error_capacity, WW_PULSE_LIBRARY_UNAVAILABLE, dlerror());
            pthread_mutex_unlock(&api_mutex);
            return rc;
        }
        api_library = library;
    }
    struct control_api api = { 0 };

#define LOAD(field, symbol)                                                                     \
    do {                                                                                        \
        void* address = dlsym(library, #symbol);                                                \
        if (! address) {                                                                        \
            char message[256];                                                                  \
            snprintf(message, sizeof(message), "missing PulseAudio symbol %s", #symbol);        \
            const int rc =                                                                      \
                fail_with(error_code, error, error_capacity, WW_PULSE_MISSING_SYMBOL, message); \
            pthread_mutex_unlock(&api_mutex);                                                   \
            return rc;                                                                          \
        }                                                                                       \
        memcpy(&api.field, &address, sizeof(address));                                          \
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
    LOAD(context_set_state_callback, pa_context_set_state_callback);
    LOAD(context_connect, pa_context_connect);
    LOAD(context_disconnect, pa_context_disconnect);
    LOAD(context_unref, pa_context_unref);
    LOAD(context_get_state, pa_context_get_state);
    LOAD(context_errno, pa_context_errno);
    LOAD(context_get_server_info, pa_context_get_server_info);
    LOAD(context_get_sink_info_list, pa_context_get_sink_info_list);
    LOAD(context_get_sink_input_info_list, pa_context_get_sink_input_info_list);
    LOAD(context_move_sink_input_by_name, pa_context_move_sink_input_by_name);
    LOAD(operation_unref, pa_operation_unref);
    LOAD(proplist_gets, pa_proplist_gets);
    LOAD(strerror_fn, pa_strerror);
#undef LOAD

    api_cached = api;
    api_ready  = 1;
    *target    = api;
    pthread_mutex_unlock(&api_mutex);
    return 0;
}

static void on_context_state(pa_context* context, void* userdata) {
    (void)context;
    control_t* self = userdata;
    self->api.threaded_mainloop_signal(self->loop, 0);
}

/* Tears the session down. Safe to call at any point of control_open(). */
static void control_close(control_t* self) {
    if (! self->loop) return;
    if (self->context) {
        if (! self->locked) {
            self->api.threaded_mainloop_lock(self->loop);
            self->locked = 1;
        }
        self->api.context_set_state_callback(self->context, NULL, NULL);
        self->api.context_disconnect(self->context);
        self->api.context_unref(self->context);
        self->context = NULL;
    }
    if (self->locked) {
        self->api.threaded_mainloop_unlock(self->loop);
        self->locked = 0;
    }
    if (self->loop_started) {
        self->api.threaded_mainloop_stop(self->loop);
        self->loop_started = 0;
    }
    self->api.threaded_mainloop_free(self->loop);
    self->loop = NULL;
}

/* Connects to the sound server. On success the mainloop lock stays held until
 * control_close(); on failure everything is already cleaned up. */
static int control_open(control_t* self, int* error_code, char* error, size_t error_capacity) {
    if (load_control_api(&self->api, error, error_capacity, error_code) < 0) return -1;

    self->loop = self->api.threaded_mainloop_new();
    if (! self->loop || self->api.threaded_mainloop_start(self->loop) < 0) {
        if (self->loop) {
            self->api.threaded_mainloop_free(self->loop);
            self->loop = NULL;
        }
        return fail_with(error_code,
                         error,
                         error_capacity,
                         WW_PULSE_SERVER_UNAVAILABLE,
                         "failed to start PulseAudio mainloop");
    }
    self->loop_started = 1;

    self->api.threaded_mainloop_lock(self->loop);
    self->locked = 1;
    self->context =
        self->api.context_new(self->api.threaded_mainloop_get_api(self->loop), "Waywallen Daemon");
    if (! self->context) {
        control_close(self);
        return fail_with(error_code,
                         error,
                         error_capacity,
                         WW_PULSE_SERVER_UNAVAILABLE,
                         "failed to create PulseAudio context");
    }
    self->api.context_set_state_callback(self->context, on_context_state, self);
    if (self->api.context_connect(self->context, NULL, PA_CONTEXT_NOFLAGS, NULL) < 0) {
        control_close(self);
        return fail_with(error_code,
                         error,
                         error_capacity,
                         WW_PULSE_SERVER_UNAVAILABLE,
                         "failed to connect to PulseAudio server");
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
            control_close(self);
            return fail_with(
                error_code, error, error_capacity, WW_PULSE_SERVER_UNAVAILABLE, message);
        }
        self->api.threaded_mainloop_wait(self->loop);
    }
    return 0;
}

/* Waits until every issued operation has called back. Returns -1 if the
 * connection dropped first. The mainloop lock must be held. */
static int control_wait(control_t* self) {
    while (self->pending > 0) {
        const pa_context_state_t state = self->api.context_get_state(self->context);
        if (! PA_CONTEXT_IS_GOOD(state)) return -1;
        self->api.threaded_mainloop_wait(self->loop);
    }
    return 0;
}

static void operation_done(control_t* self) {
    if (self->pending > 0) self->pending--;
    self->api.threaded_mainloop_signal(self->loop, 0);
}

/* Issues one operation. `operation` is NULL when the call itself was refused. */
static int operation_started(control_t* self, pa_operation* operation) {
    if (! operation) return -1;
    self->api.operation_unref(operation);
    return 0;
}

static void on_server_info(pa_context* context, const pa_server_info* info, void* userdata) {
    (void)context;
    control_t* self = userdata;
    if (info && info->default_sink_name) {
        snprintf(self->default_sink, sizeof(self->default_sink), "%s", info->default_sink_name);
    }
    operation_done(self);
}

static void on_sink_info(pa_context* context, const pa_sink_info* info, int eol, void* userdata) {
    (void)context;
    control_t* self = userdata;
    if (eol != 0) {
        if (eol < 0) self->failed = 1;
        operation_done(self);
        return;
    }
    if (! info) return;
    if (self->sink_total < self->sink_capacity && self->sinks) {
        ww_pulse_sink_t* sink = &self->sinks[self->sink_total];
        snprintf(sink->name, sizeof(sink->name), "%s", info->name ? info->name : "");
        snprintf(sink->description,
                 sizeof(sink->description),
                 "%s",
                 info->description ? info->description : "");
        sink->is_default = 0;
    }
    self->sink_total++;
}

static int32_t stream_process_id(control_t* self, const pa_sink_input_info* info) {
    if (! info->proplist) return 0;
    const char* value = self->api.proplist_gets(info->proplist, PA_PROP_APPLICATION_PROCESS_ID);
    if (! value || ! *value) return 0;
    errno                   = 0;
    char*               end = NULL;
    const unsigned long pid = strtoul(value, &end, 10);
    if (errno != 0 || end == value || *end != '\0' || pid == 0 || pid > INT32_MAX) return 0;
    return (int32_t)pid;
}

static void on_stream_info(pa_context* context, const pa_sink_input_info* info, int eol,
                           void* userdata) {
    (void)context;
    control_t* self = userdata;
    if (eol != 0) {
        if (eol < 0) self->failed = 1;
        operation_done(self);
        return;
    }
    if (! info) return;
    if (self->stream_total < self->stream_capacity && self->streams) {
        int has_volume = 0;
        for (uint8_t channel = 0; channel < info->volume.channels; ++channel) {
            if (info->volume.values[channel] > PA_VOLUME_MUTED) has_volume = 1;
        }
        ww_pulse_playback_stream_t* stream = &self->streams[self->stream_total];
        stream->index                      = info->index;
        stream->process_id                 = stream_process_id(self, info);
        stream->corked                     = info->corked != 0;
        stream->muted                      = info->mute != 0;
        stream->has_nonzero_volume         = has_volume;
    }
    self->stream_total++;
}

static void on_move_done(pa_context* context, int success, void* userdata) {
    (void)context;
    control_t* self = userdata;
    if (success) self->moved++;
    operation_done(self);
}

static int connection_error(control_t* self, int* error_code, char* error, size_t error_capacity,
                            const char* what) {
    char message[256];
    snprintf(message,
             sizeof(message),
             "%s: %s",
             what,
             self->api.strerror_fn(self->api.context_errno(self->context)));
    return fail_with(error_code, error, error_capacity, WW_PULSE_SERVER_UNAVAILABLE, message);
}

int ww_pulse_list_sinks(ww_pulse_sink_t* sinks, size_t capacity, size_t* total, int* error_code,
                        char* error, size_t error_capacity) {
    if (error_code) *error_code = WW_PULSE_OK;
    copy_error(error, error_capacity, "");
    if (total) *total = 0;

    control_t self     = { 0 };
    self.sinks         = sinks;
    self.sink_capacity = sinks ? capacity : 0;
    if (control_open(&self, error_code, error, error_capacity) < 0) return -1;

    int rc       = 0;
    self.pending = 2;
    if (operation_started(
            &self, self.api.context_get_server_info(self.context, on_server_info, &self)) < 0 ||
        operation_started(
            &self, self.api.context_get_sink_info_list(self.context, on_sink_info, &self)) < 0) {
        rc = connection_error(
            &self, error_code, error, error_capacity, "failed to query PulseAudio sinks");
    } else if (control_wait(&self) < 0 || self.failed) {
        rc = connection_error(
            &self, error_code, error, error_capacity, "failed to enumerate PulseAudio sinks");
    }

    if (rc == 0) {
        const size_t filled =
            self.sink_total < self.sink_capacity ? self.sink_total : self.sink_capacity;
        for (size_t i = 0; i < filled; ++i) {
            self.sinks[i].is_default =
                self.default_sink[0] != '\0' && strcmp(self.sinks[i].name, self.default_sink) == 0;
        }
        if (total) *total = self.sink_total;
    }
    control_close(&self);
    return rc;
}

int ww_pulse_list_playback_streams(ww_pulse_playback_stream_t* streams, size_t capacity,
                                   size_t* total, int* error_code, char* error,
                                   size_t error_capacity) {
    if (error_code) *error_code = WW_PULSE_OK;
    copy_error(error, error_capacity, "");
    if (total) *total = 0;

    control_t self       = { 0 };
    self.streams         = streams;
    self.stream_capacity = streams ? capacity : 0;
    if (control_open(&self, error_code, error, error_capacity) < 0) return -1;

    int rc       = 0;
    self.pending = 1;
    if (operation_started(
            &self, self.api.context_get_sink_input_info_list(self.context, on_stream_info, &self)) <
        0) {
        rc = connection_error(
            &self, error_code, error, error_capacity, "failed to query PulseAudio streams");
    } else if (control_wait(&self) < 0 || self.failed) {
        rc = connection_error(
            &self, error_code, error, error_capacity, "failed to enumerate PulseAudio streams");
    }
    if (rc == 0 && total) *total = self.stream_total;
    control_close(&self);
    return rc;
}

int ww_pulse_move_playback_streams(const char* sink_name, const uint32_t* indices, size_t count,
                                   size_t* moved, int* error_code, char* error,
                                   size_t error_capacity) {
    if (error_code) *error_code = WW_PULSE_OK;
    copy_error(error, error_capacity, "");
    if (moved) *moved = 0;
    if (count == 0) return 0;

    control_t self = { 0 };
    if (control_open(&self, error_code, error, error_capacity) < 0) return -1;

    int rc = 0;
    /* No name means "follow the default": resolve it to a concrete sink. */
    const char* target = sink_name;
    if (! target || target[0] == '\0') {
        self.pending = 1;
        if (operation_started(
                &self, self.api.context_get_server_info(self.context, on_server_info, &self)) < 0 ||
            control_wait(&self) < 0) {
            rc = connection_error(&self,
                                  error_code,
                                  error,
                                  error_capacity,
                                  "failed to query the default PulseAudio sink");
        } else if (self.default_sink[0] == '\0') {
            rc = fail_with(error_code,
                           error,
                           error_capacity,
                           WW_PULSE_MONITOR_UNAVAILABLE,
                           "PulseAudio server has no default sink");
        }
        target = self.default_sink;
    }

    if (rc == 0) {
        self.pending = 0;
        for (size_t i = 0; i < count; ++i) {
            pa_operation* operation = self.api.context_move_sink_input_by_name(
                self.context, indices[i], target, on_move_done, &self);
            if (operation) {
                self.pending++;
                self.api.operation_unref(operation);
            }
        }
        if (control_wait(&self) < 0) {
            rc = connection_error(
                &self, error_code, error, error_capacity, "PulseAudio connection lost");
        }
    }
    if (moved) *moved = self.moved;
    control_close(&self);
    return rc;
}
