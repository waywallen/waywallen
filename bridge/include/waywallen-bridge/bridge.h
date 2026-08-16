/* waywallen-bridge — C library for renderer subprocesses to talk to
 * the waywallen daemon over its IPC Unix-domain socket.
 *
 * This header layers length-prefix framing + SCM_RIGHTS fd passing
 * on top of the auto-generated per-message encoders/decoders in
 * <waywallen-bridge/ipc_v3.h>.
 *
 * Wire frame (same layout as waywallen-display-v1):
 *
 *     [u16 LE opcode][u16 LE total_length][body...]
 *
 * where total_length includes the 4-byte header. Ancillary fds ride
 * along on the same sendmsg/recvmsg call.
 *
 * Error conventions: all functions return 0 on success and a negative
 * value on failure. The negative is either a negated errno, or one of
 * the WW_ERR_* codes defined in <waywallen-bridge/ipc_v3.h>.
 *
 * Thread safety: none. Each socket is single-writer, single-reader
 * from the caller's perspective.
 */
#ifndef WAYWALLEN_BRIDGE_H
#define WAYWALLEN_BRIDGE_H

#include <waywallen-bridge/ipc_v3.h>
#include <waywallen-bridge/drm_fourcc.h>
#include <waywallen-bridge/protocol_bits.h>

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Stable metadata for audio playback created by integrated renderer hosts.
 * The daemon uses process ownership for classification; these values keep
 * PulseAudio/PipeWire mixers and diagnostics consistent. */
#define WW_BRIDGE_AUDIO_APPLICATION_NAME "Waywallen Renderer"
#define WW_BRIDGE_AUDIO_APPLICATION_ID   "org.waywallen.renderer"
#define WW_BRIDGE_AUDIO_STREAM_PREFIX    "waywallen.renderer."

/* -----------------------------------------------------------------------
 * Connection
 * ----------------------------------------------------------------------- */

/* Connect to the daemon's IPC socket at `socket_path`.
 * Returns the socket fd (>=0) on success, or a negative errno on failure. */
int ww_bridge_connect(const char* socket_path);

/* Close a bridge socket. Equivalent to close(fd). */
void ww_bridge_close(int sock);

/* -----------------------------------------------------------------------
 * Logging
 *
 * The bridge logs internal events (slot allocation failures, bind
 * errors, the per-directive bind_buffers diagnostic, …) to stderr by
 * default. Renderers using rstd::log (or any other logging framework)
 * install a callback to redirect them — same pattern as
 * waywallen_display_set_log_callback.
 * ----------------------------------------------------------------------- */

typedef enum ww_bridge_log_level
{
    WW_BRIDGE_LOG_DEBUG = 0,
    WW_BRIDGE_LOG_INFO  = 1,
    WW_BRIDGE_LOG_WARN  = 2,
    WW_BRIDGE_LOG_ERROR = 3,
} ww_bridge_log_level_t;

typedef void (*ww_bridge_log_callback_t)(ww_bridge_log_level_t level, const char* msg,
                                         void* user_data);

/* Install a global log callback. Pass NULL to fall back to stderr.
 * Not thread-safe with concurrent log emission — call once at startup. */
void ww_bridge_set_log_callback(ww_bridge_log_callback_t cb, void* user_data);

/* -----------------------------------------------------------------------
 * Low-level framing
 * ----------------------------------------------------------------------- */

/* Send a pre-encoded message body. `opcode` is the message opcode,
 * `body` is the encoded bytes (use ww_*_encode into a ww_buf_t to fill),
 * `fds`/`n_fds` are optional SCM_RIGHTS ancillary fds.
 *
 * Hard limits: body_len + 4 must fit in u16 (65531 max body), n_fds <= 64.
 *
 * Returns 0 on success. */
int ww_bridge_send_frame(int sock, uint16_t opcode, const uint8_t* body, size_t body_len,
                         const int* fds, size_t n_fds);

/* Receive a single framed message. On success:
 *   - *opcode_out      is the message opcode
 *   - *body_out        is a freshly-malloc()d buffer of length *body_len_out
 *                      (caller must free() it)
 *   - fds_out[0..*n_fds_out]  gets any SCM_RIGHTS fds that arrived (caller
 *                             owns them; call close() when done)
 *
 * `fds_cap` bounds how many fds we'll accept; exceeding it is an error.
 * Returns 0 on success, a negative errno on I/O, or WW_ERR_* on protocol
 * errors. */
int ww_bridge_recv_frame(int sock, uint16_t* opcode_out, uint8_t** body_out, size_t* body_len_out,
                         int* fds_out, size_t fds_cap, size_t* n_fds_out);

/* -----------------------------------------------------------------------
 * High-level event senders (subprocess -> daemon)
 * ----------------------------------------------------------------------- */

/* Emit `Ready`. Must be the first event after connecting. No fds.
 *
 * `drm_node` identifies the DRM render-node
 * of the GPU the renderer's Vulkan/EGL/etc. instance picked, so the
 * daemon can decide whether each subscribed display is on the same GPU
 * (zero-copy) or a different GPU (must round-trip via HOST_VISIBLE).
 * Pass `(0, 0)` when the renderer cannot resolve its render node — the
 * daemon then conservatively assumes cross-GPU and forces HOST_VISIBLE
 * placement on every subsequent buffer negotiation. */
int ww_bridge_send_ready(int sock, const waywallen_drm_node_t* drm_node);

/* Emit `BindBuffers` carrying one DMA-BUF fd per flattened buffer plane.
 * `fds` must have exactly `pool->count * pool->format.plane_count` entries. */
int ww_bridge_send_bind_buffers(int sock, const waywallen_buffer_pool_t* pool, const int* fds);

/* Emit `FrameReady` with a single acquire sync_fd (dma_fence sync_file).
 * `frame->release_point` names the timeline value the daemon will signal on
 * the producer-exported `release_syncobj` once every consumer has
 * finished sampling this frame.
 *
 * The fd is required on every path and MUST be a SYNC_FD (dma_fence
 * sync_file), produced via
 * `vkGetSemaphoreFdKHR(SYNC_FD)` on a binary semaphore created with
 * `VkExportSemaphoreCreateInfo.handleTypes = SYNC_FD`. OPAQUE_FD
 * timeline exports are NOT cross-vendor portable and MUST NOT be used
 * here. */
int ww_bridge_send_frame_ready(int sock, const waywallen_frame_t* frame, int sync_fd);

/* Emit `ReleaseSyncobj` carrying the producer's timeline drm_syncobj fd.
 * Send exactly once per connection, after `Ready` and before any
 * `FrameReady`.
 *
 * The fd is a kernel `drm_syncobj` HANDLE_TO_FD export (timeline
 * semantics — points are u64 release_point values). It is wire-
 * compatible with `vkGetSemaphoreFdKHR(OPAQUE_FD)` on radv (which is
 * implemented as drm_syncobj), but the canonical producer for this fd
 * is the bridge itself via `ww_drm_syncobj_create` /
 * `ww_drm_syncobj_export_fd` — kernel ioctls work on every driver,
 * which Vulkan's OPAQUE_FD export does not (NVIDIA's OPAQUE_FD payload
 * is a private format incompatible with drm_syncobj).
 *
 * Consumer guidance: do NOT `vkImportSemaphoreFdKHR(OPAQUE_FD)` this fd
 * on a different-vendor GPU — it will be rejected with "Failed to
 * allocate semaphore device memory" or similar. Signal release via
 * `DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL` (kernel ioctl, vendor-agnostic)
 * after the consumer's GPU work has retired. See cross_gpu.md.
 *
 * The caller retains ownership of `release_syncobj_fd` and is
 * responsible for closing it after this call returns (the kernel
 * dup'd it into SCM_RIGHTS). */
int ww_bridge_send_release_syncobj(int sock, int release_syncobj_fd);

/* Emit `FormatCaps` — the producer's modifier-negotiation declaration.
 * Send exactly once per connection, after `Ready` and before any
 * `BindBuffers`. Caller fills the generated capability struct directly.
 *
 * Validation invariant (mirrored on the daemon side):
 *   capabilities->modifiers.count == capabilities->plane_counts.count ==
 *   sum(capabilities->mod_counts.data[0..fourccs.count])
 * The helper does NOT enforce this — the renderer must construct the
 * arrays consistently or the daemon's unflatten_caps will reject. */
int ww_bridge_send_format_caps(int sock, const waywallen_producer_capabilities_t* capabilities);

/* Emit `BindFailed` — non-terminal report that the renderer could not
 * satisfy a `negotiate_buffers` request. Daemon blacklists the
 * (fourcc, modifier) pair on this renderer and re-runs the picker. */
int ww_bridge_send_bind_failed(int sock, const waywallen_bind_failure_t* failure);

/* Emit an `Error` event with a text message. */
int ww_bridge_send_error(int sock, const char* msg);

/* Emit an atomic typed renderer-state patch. The caller retains all storage. */
int ww_bridge_send_report_state(int sock, const waywallen_renderer_state_t* state);

/* Convenience: publish a typed clear color. Components are clamped to
 * `[0, 1]`; callers should deduplicate unchanged values. */
int ww_bridge_send_report_state_clear_color(int sock, float r, float g, float b, float a);

/* Convenience: replace the complete ordered runtime-tag list. An empty
 * list clears all tags. The caller retains every key/value string. */
#define WW_BRIDGE_MAX_RUNTIME_TAGS 8u
int ww_bridge_send_report_state_tags(int sock, const ww_kv_list_t* tags);

/* Replace the complete runtime optional-event subscription set. `revision`
 * starts at 1 and increases monotonically for this connection. The daemon
 * replies with `WW_EVT_IN_EVENT_SUBSCRIPTIONS_APPLIED` before it begins
 * delivering events for the new revision. An empty array unsubscribes from
 * every optional event. */
#define WW_BRIDGE_MAX_EVENT_SUBSCRIPTIONS 16u
int ww_bridge_set_event_subscriptions(int sock, const waywallen_event_subscription_t* subscription);

/* -----------------------------------------------------------------------
 * Modifier negotiation
 *
 * Producer-side bookkeeping for the `format_caps` / `negotiate_buffers`
 * dance: a pinned (fourcc, modifier, plane_count) the slot pool is
 * currently allocated against, plus the full set of (modifier,
 * plane_count) tuples the producer can switch to via re-allocation.
 * ----------------------------------------------------------------------- */

/* One advertised (fourcc, modifier, plane_count) tuple. The daemon's
 * negotiator strict-equals plane_count when intersecting producer and
 * consumer caps, so producers must report truth — see
 * waywallen/src/negotiate.rs:432. */
typedef struct ww_format_entry {
    uint32_t fourcc;
    uint64_t modifier;
    uint32_t plane_count;
} ww_format_entry_t;

/* Producer-side negotiation snapshot. Owned by the caller; the
 * `advertised` array points at producer storage that outlives the
 * negotiation calls. The pinned (fourcc, modifier, plane_count) is
 * the one the slot pool is currently allocated against; on
 * `negotiate_buffers` the producer either re-allocates to a different
 * entry from `advertised` (and updates the pinned tuple) or replies
 * `bind_failed` to push the daemon to re-pick.
 *
 * Invariants:
 *   - The pinned (fourcc, modifier, plane_count) MUST appear in
 *     `advertised`.
 *   - Entries with the same `fourcc` MUST be contiguous in
 *     `advertised` (the format_caps flatten helper walks runs).
 *   - The pinned entry SHOULD be first within its fourcc's run, and
 *     the pinned fourcc SHOULD come before non-pinned fourccs — this
 *     lets the daemon's picker land on the pinned tuple in one round
 *     instead of bouncing through `bind_failed` retries. */
typedef struct ww_negotiation_state {
    uint32_t                 fourcc;
    uint64_t                 modifier;
    uint32_t                 plane_count;
    const ww_format_entry_t* advertised;
    size_t                   advertised_count;
} ww_negotiation_state_t;

/* True (1) if a (fourcc, modifier) pair is anywhere in `advertised`.
 * False (0) otherwise. NULL `neg` returns 0. Replaces the linear-scan
 * "is this in our advertised set?" check producers do in their
 * NegotiateBuffers handlers. */
int ww_bridge_negotiation_contains(const ww_negotiation_state_t* neg, uint32_t fourcc,
                                   uint64_t modifier);

/* Populate a generated producer-capability struct from the negotiation state plus
 * caller-provided scratch arrays. Walks `advertised` collapsing
 * contiguous same-fourcc runs into the wire format's
 * `(fourccs[], mod_counts[])` shape; relies on the
 * "same-fourcc-contiguous" invariant above.
 *
 * Scratch sizing (caller owns and outlives `out`); all sized to
 * `neg->advertised_count` for worst-case (one fourcc per entry):
 *   - `scratch_fourccs`      [advertised_count]
 *   - `scratch_mod_counts`   [advertised_count]
 *   - `scratch_modifiers`    [advertised_count]
 *   - `scratch_plane_counts` [advertised_count]
 *
 * Caller still fills the scalar negotiation knobs (sync_caps,
 * color_caps, mem_hints, max_extent, UUIDs, drm_node) on `out`
 * after this call. */
void ww_bridge_negotiation_fill_format_caps(const ww_negotiation_state_t* neg,
                                            uint32_t* scratch_fourccs, uint32_t* scratch_mod_counts,
                                            uint64_t*                          scratch_modifiers,
                                            uint32_t*                          scratch_plane_counts,
                                            waywallen_producer_capabilities_t* out);

/* -----------------------------------------------------------------------
 * Renderer utilities
 *
 * Tiny helpers shared verbatim by every renderer subprocess. Kept in
 * the header so they're trivially inlineable across both C and C++
 * call sites.
 * ----------------------------------------------------------------------- */

/* Monotonic nanosecond timestamp for `frame_ready.ts_ns` and any other
 * place a renderer needs a steady-clock reading. Falls back to 0 on
 * the (vanishingly rare) clock_gettime failure rather than crashing —
 * the daemon treats ts_ns as advisory. */
uint64_t ww_bridge_now_ns(void);

/* -----------------------------------------------------------------------
 * Diagnostics
 * ----------------------------------------------------------------------- */

/* One labeled row of the GPU info block. Both fields are
 * caller-owned, NUL-terminated. `value == NULL` is rendered as
 * "(null)" — useful when an EGL/Vulkan/GL string accessor returns
 * NULL. `label == NULL` is treated as the empty string. */
typedef struct ww_gpu_info_field {
    const char* label;
    const char* value;
} ww_gpu_info_field_t;

/* Print a "GPU info" diagnostic block to stderr, formatted as
 *
 *     {prefix}: GPU info
 *       {label}: {value}
 *       ...
 *
 * The label column auto-aligns to the widest label across all
 * supplied fields. Caller does the GPU-API queries (eglQueryString,
 * glGetString, vkGetPhysicalDeviceProperties ...) and hands the
 * already-fetched strings to this helper, so the bridge stays free
 * of any EGL/GL/Vulkan dependency. */
void ww_bridge_log_gpu_info(const char* prefix, const ww_gpu_info_field_t* fields, size_t n_fields);

/* -----------------------------------------------------------------------
 * High-level control receive (daemon -> subprocess)
 * ----------------------------------------------------------------------- */

/* Tagged union of all incoming inbound events from the daemon. `op`
 * selects which union arm is populated. String / kv fields inside are
 * heap-allocated — call `ww_bridge_control_free` when done. */
typedef struct ww_bridge_control {
    ww_event_in_op_t op;
    union {
        ww_evt_in_init_t                        init;
        ww_evt_in_setting_changed_t             setting_changed;
        ww_evt_in_play_t                        play;
        ww_evt_in_pause_t                       pause;
        ww_evt_in_mute_t                        mute;
        ww_evt_in_unmute_t                      unmute;
        ww_evt_in_pointer_motion_t              pointer_motion;
        ww_evt_in_pointer_button_t              pointer_button;
        ww_evt_in_pointer_axis_t                pointer_axis;
        ww_evt_in_mpris_t                       mpris;
        ww_evt_in_event_subscriptions_applied_t event_subscriptions_applied;
        ww_evt_in_audio_window_t                audio_window;
        ww_evt_in_shutdown_t                    shutdown;
        ww_evt_in_negotiate_buffers_t           negotiate_buffers;
        ww_evt_in_request_frame_t               request_frame;
    } u;
} ww_bridge_control_t;

/* Receive the next control message. Blocks until a full frame is
 * available or the peer closes. Returns 0 on success. */
int ww_bridge_recv_control(int sock, ww_bridge_control_t* out);

/* Free any heap allocations inside a decoded control message. Safe to
 * call on a zero-initialized struct. */
void ww_bridge_control_free(ww_bridge_control_t* msg);

/* -----------------------------------------------------------------------
 * Init handshake
 * ----------------------------------------------------------------------- */

/* Renderer IPC compatibility version this build of the bridge handles.
 * Bump when the daemon/renderer wire contract changes; `ww_bridge_recv_init`
 * validates the value sent by the daemon matches and returns -EPROTO
 * otherwise. */
#define WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION 3u
#define WW_BRIDGE_SUPPORTED_SPAWN_VERSION    11u

/* Receive the daemon's typed `init` request and copy it into `out`.
 *
 * Behaviour:
 *   - Blocks until the next control frame arrives.
 *   - If the message is anything other than `WW_EVT_IN_INIT`, the body
 *     is freed and -EPROTO is returned.
 *   - If `protocol_version != WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION` or
 *     `spawn_version != WW_BRIDGE_SUPPORTED_SPAWN_VERSION`, decoded
 *     values remain available to the caller and the function returns
 *     -EPROTO. Heap fields must still be released via
 *     `waywallen_renderer_init_free`.
 *   - On success returns 0; ownership of every heap allocation
 *     transfers to the caller. */
int ww_bridge_recv_init(int sock, waywallen_renderer_init_t* out);

/* Emit an `init_nack` event back to the daemon (subprocess →
 * daemon). Used when `ww_bridge_recv_init` returns -EPROTO due to a
 * version mismatch or when the renderer cannot satisfy the typed
 * payload. The daemon kills the child and propagates `reason` to
 * the spawn caller.
 *
 * `reason` may be NULL (encoded as the empty string). Returns 0 on
 * success or a negative errno / WW_ERR_* on failure. */
int ww_bridge_send_init_nack(int sock, const waywallen_init_rejection_t* rejection);

/* -----------------------------------------------------------------------
 * Runtime optional events
 * ----------------------------------------------------------------------- */

#define WW_BRIDGE_AUDIO_SAMPLE_RATE   48000u
#define WW_BRIDGE_AUDIO_CHANNELS      2u
#define WW_BRIDGE_AUDIO_WINDOW_FRAMES 4096u
#define WW_BRIDGE_AUDIO_SAMPLE_COUNT  (WW_BRIDGE_AUDIO_CHANNELS * WW_BRIDGE_AUDIO_WINDOW_FRAMES)
#define WW_BRIDGE_AUDIO_END_OF_STREAM 1u

typedef struct ww_bridge_audio_window {
    uint64_t subscription_revision;
    uint64_t generation;
    uint64_t sequence;
    uint64_t captured_at_ns;
    uint64_t end_sample_frame;
    uint32_t sample_rate_hz;
    uint32_t channels;
    uint32_t frames;
    uint32_t flags;
    float    samples[WW_BRIDGE_AUDIO_SAMPLE_COUNT];
} ww_bridge_audio_window_t;

/* Copy and validate a complete PCM window or an explicit end marker. `ctrl`
 * retains its allocations and must still be released with
 * `ww_bridge_control_free`. */
int ww_bridge_audio_window_from_control(const ww_bridge_control_t* ctrl,
                                        ww_bridge_audio_window_t*  out);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* WAYWALLEN_BRIDGE_H */
