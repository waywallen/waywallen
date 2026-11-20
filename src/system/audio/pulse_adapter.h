#ifndef WAYWALLEN_PULSE_ADAPTER_H
#define WAYWALLEN_PULSE_ADAPTER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ww_pulse_capture           ww_pulse_capture_t;
typedef struct ww_pulse_playback_observer ww_pulse_playback_observer_t;

typedef struct ww_pulse_playback_stream {
    uint32_t index;
    int32_t  process_id;
    int      corked;
    int      muted;
    int      has_nonzero_volume;
} ww_pulse_playback_stream_t;

enum ww_pulse_error
{
    WW_PULSE_OK                  = 0,
    WW_PULSE_LIBRARY_UNAVAILABLE = 1,
    WW_PULSE_MISSING_SYMBOL      = 2,
    WW_PULSE_SERVER_UNAVAILABLE  = 3,
    WW_PULSE_MONITOR_UNAVAILABLE = 4,
    WW_PULSE_STREAM_FAILED       = 5,
    WW_PULSE_OUT_OF_MEMORY       = 6,
};

/* Records the monitor of `sink_name`. A NULL or empty name, or a sink that
 * does not exist, records the monitor of the current default sink instead. */
ww_pulse_capture_t* ww_pulse_capture_open(const char* sink_name, int* error_code, char* error,
                                          size_t error_capacity);
void                ww_pulse_capture_close(ww_pulse_capture_t* capture);

/* Copies interleaved stereo F32LE frames from the callback-owned fixed ring.
 * Returns the number of frames copied. */
size_t ww_pulse_capture_read(ww_pulse_capture_t* capture, float* samples, size_t frame_capacity,
                             uint64_t* generation);
int    ww_pulse_capture_failed(ww_pulse_capture_t* capture, char* error, size_t error_capacity);

ww_pulse_playback_observer_t* ww_pulse_playback_observer_open(int* error_code, char* error,
                                                              size_t error_capacity);
void ww_pulse_playback_observer_close(ww_pulse_playback_observer_t* observer);

/* Returns the total stream count. Up to `capacity` records are copied. */
size_t ww_pulse_playback_observer_snapshot(ww_pulse_playback_observer_t* observer,
                                           ww_pulse_playback_stream_t* streams, size_t capacity);
int    ww_pulse_playback_observer_failed(ww_pulse_playback_observer_t* observer, char* error,
                                         size_t error_capacity);

#ifdef __cplusplus
}
#endif

#endif
