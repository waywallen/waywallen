#ifndef WAYWALLEN_PULSE_CONTROL_H
#define WAYWALLEN_PULSE_CONTROL_H

#include "pulse_adapter.h"

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define WW_PULSE_CONTROL_NAME_CAPACITY 256u

typedef struct ww_pulse_sink {
    char name[WW_PULSE_CONTROL_NAME_CAPACITY];
    char description[WW_PULSE_CONTROL_NAME_CAPACITY];
    int  is_default;
} ww_pulse_sink_t;

/* One-shot helpers: each call opens a short-lived PulseAudio context, performs
 * its query or request, and closes it again. They are meant for infrequent,
 * user-driven operations and may block while the sound server answers.
 *
 * All of them return 0 on success. On failure they return -1, set
 * `*error_code` to a `ww_pulse_error` value and write a message into `error`. */

/* Lists the output devices (sinks). Up to `capacity` records are copied and
 * `*total` receives the real count, which may be larger than `capacity`. */
int ww_pulse_list_sinks(ww_pulse_sink_t* sinks, size_t capacity, size_t* total, int* error_code,
                        char* error, size_t error_capacity);

/* Lists playback streams (sink inputs). Up to `capacity` records are copied
 * and `*total` receives the real count. */
int ww_pulse_list_playback_streams(ww_pulse_playback_stream_t* streams, size_t capacity,
                                   size_t* total, int* error_code, char* error,
                                   size_t error_capacity);

/* Moves the playback streams with the given indices to `sink_name`. A NULL or
 * empty name selects the server's current default sink. `*moved` receives the
 * number of streams the server accepted. A stream that disappears or cannot be
 * moved is skipped and is not an error. */
int ww_pulse_move_playback_streams(const char* sink_name, const uint32_t* indices, size_t count,
                                   size_t* moved, int* error_code, char* error,
                                   size_t error_capacity);

#ifdef __cplusplus
}
#endif

#endif
