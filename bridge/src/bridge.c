/* waywallen-bridge — IPC framing + high-level helpers.
 *
 * Handwritten companion to the auto-generated src/ipc_v3.c. Provides
 * SCM_RIGHTS fd passing on top of the generated per-message encoders
 * and a tagged union for incoming control requests.
 */
/* CLOCK_MONOTONIC + struct timespec require POSIX.1-2008 visibility
 * under -std=c11. Set the macro here so we don't drag a CMake-side
 * compile flag in just for the timing helper. Must precede any
 * system header. */
#ifndef _POSIX_C_SOURCE
#    define _POSIX_C_SOURCE 200809L
#endif

#include <waywallen-bridge/bridge.h>

#include "log_internal.h"

#include <errno.h>
#include <math.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

/* Keep in sync with waywallen's MAX_FDS_PER_MSG. 64 is generous for
 * the protocol's current needs (BindBuffers with ~8 planes) and keeps
 * the CMSG scratch buffer stack-allocatable. */
#define WW_BRIDGE_MAX_FDS 64

/* Max inline body: u16 total length minus 4-byte header. */
#define WW_BRIDGE_MAX_BODY (65535 - 4)

/* -----------------------------------------------------------------------
 * Logging
 * ----------------------------------------------------------------------- */

static ww_bridge_log_callback_t s_log_cb = NULL;
static void*                    s_log_ud = NULL;

void ww_bridge_set_log_callback(ww_bridge_log_callback_t cb, void* user_data) {
    s_log_cb = cb;
    s_log_ud = user_data;
}

__attribute__((format(printf, 2, 3), visibility("hidden"))) void
ww_bridge_logf(ww_bridge_log_level_t level, const char* fmt, ...) {
    char    buf[1024];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    (void)n;
    if (s_log_cb) {
        s_log_cb(level, buf, s_log_ud);
        return;
    }
    static const char* tags[] = { "DEBUG", "INFO", "WARN", "ERROR" };
    unsigned           idx    = (unsigned)level <= 3u ? (unsigned)level : 3u;
    fprintf(stderr, "waywallen-bridge [%s] %s\n", tags[idx], buf);
}

/* -----------------------------------------------------------------------
 * Connection
 * ----------------------------------------------------------------------- */

int ww_bridge_connect(const char* socket_path) {
    if (! socket_path) return -EINVAL;

    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) return -errno;

    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    size_t plen     = strlen(socket_path);
    if (plen >= sizeof(addr.sun_path)) {
        close(fd);
        return -ENAMETOOLONG;
    }
    memcpy(addr.sun_path, socket_path, plen + 1);

    if (connect(fd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        int err = -errno;
        close(fd);
        return err;
    }
    return fd;
}

void ww_bridge_close(int sock) {
    if (sock >= 0) close(sock);
}

/* -----------------------------------------------------------------------
 * Low-level framing
 * ----------------------------------------------------------------------- */

static int write_all(int fd, const void* buf, size_t len) {
    const uint8_t* p = (const uint8_t*)buf;
    while (len > 0) {
        ssize_t n = write(fd, p, len);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -errno;
        }
        if (n == 0) return -EPIPE;
        p += n;
        len -= (size_t)n;
    }
    return 0;
}

static int read_all(int fd, void* buf, size_t len) {
    uint8_t* p = (uint8_t*)buf;
    while (len > 0) {
        ssize_t n = read(fd, p, len);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -errno;
        }
        if (n == 0) return -ENOTCONN; /* peer closed */
        p += n;
        len -= (size_t)n;
    }
    return 0;
}

int ww_bridge_send_frame(int sock, uint16_t opcode, const uint8_t* body, size_t body_len,
                         const int* fds, size_t n_fds) {
    if (body_len > WW_BRIDGE_MAX_BODY) return -EMSGSIZE;
    if (n_fds > WW_BRIDGE_MAX_FDS) return -E2BIG;

    uint8_t  header[4];
    uint16_t total = (uint16_t)(body_len + 4);
    header[0]      = (uint8_t)(opcode & 0xff);
    header[1]      = (uint8_t)((opcode >> 8) & 0xff);
    header[2]      = (uint8_t)(total & 0xff);
    header[3]      = (uint8_t)((total >> 8) & 0xff);

    /* Single sendmsg so SCM_RIGHTS attaches atomically to the header.
     * We pack header+body into two iovecs to avoid copying. */
    struct iovec iov[2];
    iov[0].iov_base = header;
    iov[0].iov_len  = 4;
    int iov_count   = 1;
    if (body_len > 0) {
        iov[1].iov_base = (void*)body;
        iov[1].iov_len  = body_len;
        iov_count       = 2;
    }

    /* Control message space for SCM_RIGHTS fds. */
    union {
        char           buf[CMSG_SPACE(sizeof(int) * WW_BRIDGE_MAX_FDS)];
        struct cmsghdr align;
    } cmsg_storage;

    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov    = iov;
    msg.msg_iovlen = iov_count;

    if (n_fds > 0) {
        memset(&cmsg_storage, 0, sizeof(cmsg_storage));
        msg.msg_control      = cmsg_storage.buf;
        msg.msg_controllen   = CMSG_SPACE(sizeof(int) * n_fds);
        struct cmsghdr* cmsg = CMSG_FIRSTHDR(&msg);
        cmsg->cmsg_level     = SOL_SOCKET;
        cmsg->cmsg_type      = SCM_RIGHTS;
        cmsg->cmsg_len       = CMSG_LEN(sizeof(int) * n_fds);
        memcpy(CMSG_DATA(cmsg), fds, sizeof(int) * n_fds);
    }

    /* sendmsg is all-or-nothing for the first byte (where cmsg is
     * attached). If it returns a short count on a stream socket, fall
     * back to plain write() for the remainder — but that never
     * happens in practice on a well-formed SOCK_STREAM. */
    size_t expected = 4 + body_len;
    while (1) {
        ssize_t n = sendmsg(sock, &msg, MSG_NOSIGNAL);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -errno;
        }
        if ((size_t)n == expected) return 0;
        /* Short write: finish with plain write() on the remainder. */
        size_t done      = (size_t)n;
        size_t head_left = done < 4 ? 4 - done : 0;
        size_t body_done = done < 4 ? 0 : done - 4;
        if (head_left > 0) {
            int r = write_all(sock, header + (4 - head_left), head_left);
            if (r < 0) return r;
        }
        if (body_len > body_done) {
            int r = write_all(sock, body + body_done, body_len - body_done);
            if (r < 0) return r;
        }
        return 0;
    }
}

int ww_bridge_recv_frame(int sock, uint16_t* opcode_out, uint8_t** body_out, size_t* body_len_out,
                         int* fds_out, size_t fds_cap, size_t* n_fds_out) {
    if (! opcode_out || ! body_out || ! body_len_out || ! n_fds_out) return -EINVAL;

    *body_out     = NULL;
    *body_len_out = 0;
    *n_fds_out    = 0;

    /* Phase 1: read the 4-byte header via recvmsg to harvest any cmsg
     * fds that attach to the first byte of the frame. The while loop
     * handles short reads without losing ancillary data. */
    uint8_t header[4];
    size_t  filled = 0;
    while (filled < 4) {
        struct iovec iov;
        iov.iov_base = header + filled;
        iov.iov_len  = 4 - filled;

        union {
            char           buf[CMSG_SPACE(sizeof(int) * WW_BRIDGE_MAX_FDS)];
            struct cmsghdr align;
        } cmsg_storage;

        struct msghdr msg;
        memset(&msg, 0, sizeof(msg));
        msg.msg_iov        = &iov;
        msg.msg_iovlen     = 1;
        msg.msg_control    = cmsg_storage.buf;
        msg.msg_controllen = sizeof(cmsg_storage.buf);

        ssize_t n;
        do {
            n = recvmsg(sock, &msg, MSG_CMSG_CLOEXEC);
        } while (n < 0 && errno == EINTR);

        if (n < 0) return -errno;
        if (n == 0) return -ENOTCONN;

        for (struct cmsghdr* cmsg = CMSG_FIRSTHDR(&msg); cmsg; cmsg = CMSG_NXTHDR(&msg, cmsg)) {
            if (cmsg->cmsg_level == SOL_SOCKET && cmsg->cmsg_type == SCM_RIGHTS) {
                size_t     payload = cmsg->cmsg_len - CMSG_LEN(0);
                size_t     got     = payload / sizeof(int);
                const int* in_fds  = (const int*)CMSG_DATA(cmsg);
                for (size_t i = 0; i < got; i++) {
                    if (*n_fds_out >= fds_cap) {
                        /* Buffer overflow: close the rest and all our held
                         * fds, return error. */
                        for (size_t j = i; j < got; j++) close(in_fds[j]);
                        for (size_t j = 0; j < *n_fds_out; j++) close(fds_out[j]);
                        *n_fds_out = 0;
                        return -E2BIG;
                    }
                    fds_out[(*n_fds_out)++] = in_fds[i];
                }
            }
        }

        filled += (size_t)n;
    }

    uint16_t opcode = (uint16_t)header[0] | ((uint16_t)header[1] << 8);
    uint16_t total  = (uint16_t)header[2] | ((uint16_t)header[3] << 8);
    if (total < 4) {
        for (size_t i = 0; i < *n_fds_out; i++) close(fds_out[i]);
        *n_fds_out = 0;
        return WW_ERR_SHORT;
    }
    size_t body_len = (size_t)(total - 4);

    /* Phase 2: read exactly body_len bytes. SCM_RIGHTS only attaches
     * to the first byte of a frame, so plain read() is safe here. */
    uint8_t* body = NULL;
    if (body_len > 0) {
        body = (uint8_t*)malloc(body_len);
        if (! body) {
            for (size_t i = 0; i < *n_fds_out; i++) close(fds_out[i]);
            *n_fds_out = 0;
            return WW_ERR_NOMEM;
        }
        int r = read_all(sock, body, body_len);
        if (r < 0) {
            free(body);
            for (size_t i = 0; i < *n_fds_out; i++) close(fds_out[i]);
            *n_fds_out = 0;
            return r;
        }
    }

    *opcode_out   = opcode;
    *body_out     = body;
    *body_len_out = body_len;
    return 0;
}

/* -----------------------------------------------------------------------
 * High-level event senders
 * ----------------------------------------------------------------------- */

/* Helper: encode + frame + send. */
#define WW_SEND_EVENT(sock, op_enum, encode_fn, msg_ptr, fds_ptr, n_fds)                     \
    do {                                                                                     \
        ww_buf_t buf;                                                                        \
        ww_buf_init(&buf);                                                                   \
        int rc = encode_fn((msg_ptr), &buf);                                                 \
        if (rc != WW_OK) {                                                                   \
            ww_buf_free(&buf);                                                               \
            return rc;                                                                       \
        }                                                                                    \
        rc = ww_bridge_send_frame((sock), (op_enum), buf.data, buf.len, (fds_ptr), (n_fds)); \
        ww_buf_free(&buf);                                                                   \
        return rc;                                                                           \
    } while (0)

int ww_bridge_send_ready(int sock, const waywallen_drm_node_t* drm_node) {
    if (! drm_node) return -EINVAL;
    ww_evt_ready_t m = { .drm_node = *drm_node };
    WW_SEND_EVENT(sock, WW_EVT_READY, ww_evt_ready_encode, &m, NULL, 0);
}

int ww_bridge_send_bind_buffers(int sock, const waywallen_buffer_pool_t* pool, const int* fds) {
    if (! pool || ! fds) return -EINVAL;
    ww_evt_bind_buffers_t m     = { .pool = *pool };
    uint32_t              n_fds = ww_evt_bind_buffers_expected_fds(&m);
    if (n_fds > WW_BRIDGE_MAX_FDS) return -E2BIG;
    WW_SEND_EVENT(sock, WW_EVT_BIND_BUFFERS, ww_evt_bind_buffers_encode, &m, fds, n_fds);
}

int ww_bridge_send_frame_ready(int sock, const waywallen_frame_t* frame, int sync_fd) {
    if (! frame || sync_fd < 0) return -EINVAL;
    ww_evt_frame_ready_t m = { .frame = *frame };
    WW_SEND_EVENT(sock, WW_EVT_FRAME_READY, ww_evt_frame_ready_encode, &m, &sync_fd, 1);
}

int ww_bridge_send_release_syncobj(int sock, int release_syncobj_fd) {
    if (release_syncobj_fd < 0) return -EINVAL;
    ww_evt_release_syncobj_t m;
    memset(&m, 0, sizeof(m));
    WW_SEND_EVENT(
        sock, WW_EVT_RELEASE_SYNCOBJ, ww_evt_release_syncobj_encode, &m, &release_syncobj_fd, 1);
}

int ww_bridge_send_format_caps(int sock, const waywallen_producer_capabilities_t* capabilities) {
    if (! capabilities) return -EINVAL;
    ww_evt_format_caps_t m = { .capabilities = *capabilities };
    WW_SEND_EVENT(sock, WW_EVT_FORMAT_CAPS, ww_evt_format_caps_encode, &m, NULL, 0);
}

int ww_bridge_send_bind_failed(int sock, const waywallen_bind_failure_t* failure) {
    if (! failure) return -EINVAL;
    ww_evt_bind_failed_t m = { .failure = *failure };
    WW_SEND_EVENT(sock, WW_EVT_BIND_FAILED, ww_evt_bind_failed_encode, &m, NULL, 0);
}

int ww_bridge_send_render_target_capabilities(
    int sock, const waywallen_render_target_capabilities_t* capabilities) {
    if (! capabilities) return -EINVAL;
    ww_evt_render_target_capabilities_t m = { .capabilities = *capabilities };
    WW_SEND_EVENT(sock,
                  WW_EVT_RENDER_TARGET_CAPABILITIES,
                  ww_evt_render_target_capabilities_encode,
                  &m,
                  NULL,
                  0);
}

int ww_bridge_send_render_target_result(int sock, const waywallen_render_target_outcome_t* result) {
    if (! result) return -EINVAL;
    ww_evt_render_target_result_t m = { .result = *result };
    WW_SEND_EVENT(
        sock, WW_EVT_RENDER_TARGET_RESULT, ww_evt_render_target_result_encode, &m, NULL, 0);
}

int ww_bridge_render_target_directive(const waywallen_render_target_capabilities_t* caps,
                                      const waywallen_render_target_config_t*       config,
                                      waywallen_buffer_directive_t*                 out) {
    if (! caps || ! config || ! out || (caps->mode != 1 && caps->mode != 2) ||
        ! caps->reference_width || ! caps->reference_height || ! config->revision ||
        config->capabilities_revision != caps->revision || ! caps->width_alignment ||
        ! caps->height_alignment || ! config->width || ! config->height ||
        config->width < caps->min_width || config->height < caps->min_height ||
        config->width > caps->max_width || config->height > caps->max_height ||
        config->width % caps->width_alignment || config->height % caps->height_alignment)
        return -EINVAL;
    if (caps->mode == 1 &&
        (config->width > caps->reference_width || config->height > caps->reference_height))
        return -EINVAL;
    if (caps->mode == 2 &&
        (config->width > (caps->reference_width >= caps->reference_height ? 7680u : 4320u) ||
         config->height > (caps->reference_width >= caps->reference_height ? 4320u : 7680u)))
        return -EINVAL;
    /* Both aligned dimensions must admit the same content scale. */
    if ((uint64_t)(config->width - caps->width_alignment) * caps->reference_height >=
            (uint64_t)config->height * caps->reference_width ||
        (uint64_t)(config->height - caps->height_alignment) * caps->reference_width >=
            (uint64_t)config->width * caps->reference_height)
        return -EINVAL;
    *out = (waywallen_buffer_directive_t) {
        .format        = { config->fourcc, config->modifier, config->plane_count },
        .sync_mode     = config->sync_mode,
        .color         = config->color,
        .mem_hint      = config->mem_hint,
        .count         = config->count,
        .path          = config->path,
        .memory_source = config->memory_source,
    };
    return 0;
}

int ww_bridge_send_error(int sock, const char* msg) {
    if (! msg) return -EINVAL;
    ww_evt_error_t m;
    m.message = (char*)msg; /* encoder doesn't mutate */
    WW_SEND_EVENT(sock, WW_EVT_ERROR, ww_evt_error_encode, &m, NULL, 0);
}

int ww_bridge_send_report_state(int sock, const waywallen_renderer_state_t* state) {
    if (! state) return -EINVAL;
    ww_evt_report_state_t m = { .state = *state };
    WW_SEND_EVENT(sock, WW_EVT_REPORT_STATE, ww_evt_report_state_encode, &m, NULL, 0);
}

static float clamp01_(float v) {
    if (v < 0.0f) return 0.0f;
    if (v > 1.0f) return 1.0f;
    return v;
}

int ww_bridge_send_report_state_clear_color(int sock, float r, float g, float b, float a) {
    waywallen_renderer_state_t state = {
        .fields = WW_RENDERER_STATE_FIELD_CLEAR_COLOR,
        .clear_color = {
            .r = clamp01_(r),
            .g = clamp01_(g),
            .b = clamp01_(b),
            .a = clamp01_(a),
        },
    };
    return ww_bridge_send_report_state(sock, &state);
}

int ww_bridge_send_report_state_tags(int sock, const ww_kv_list_t* tags) {
    if (! tags || tags->count > WW_BRIDGE_MAX_RUNTIME_TAGS) return -EINVAL;
    if (tags->count > 0 && ! tags->data) return -EINVAL;
    for (uint32_t i = 0; i < tags->count; ++i) {
        if (! tags->data[i].key || ! tags->data[i].value) return -EINVAL;
    }
    waywallen_renderer_state_t state = {
        .fields       = WW_RENDERER_STATE_FIELD_RUNTIME_TAGS,
        .runtime_tags = *tags,
    };
    return ww_bridge_send_report_state(sock, &state);
}

int ww_bridge_set_event_subscriptions(int                                   sock,
                                      const waywallen_event_subscription_t* subscription) {
    if (! subscription || subscription->revision == 0 ||
        subscription->kinds.count > WW_BRIDGE_MAX_EVENT_SUBSCRIPTIONS)
        return -EINVAL;
    if (subscription->kinds.count > 0 && ! subscription->kinds.data) return -EINVAL;
    for (uint32_t i = 0; i < subscription->kinds.count; ++i) {
        if (! subscription->kinds.data[i] || subscription->kinds.data[i][0] == '\0') return -EINVAL;
    }

    ww_evt_set_event_subscriptions_t m = {
        .subscription = *subscription,
    };
    WW_SEND_EVENT(
        sock, WW_EVT_SET_EVENT_SUBSCRIPTIONS, ww_evt_set_event_subscriptions_encode, &m, NULL, 0);
}

/* -----------------------------------------------------------------------
 * Diagnostics
 * ----------------------------------------------------------------------- */

void ww_bridge_log_gpu_info(const char* prefix, const ww_gpu_info_field_t* fields,
                            size_t n_fields) {
    if (! fields || n_fields == 0) return;

    /* Pass 1: widest label. */
    size_t max_label = 0;
    for (size_t i = 0; i < n_fields; i++) {
        const char* l   = fields[i].label ? fields[i].label : "";
        size_t      len = strlen(l);
        if (len > max_label) max_label = len;
    }

    ww_bridge_logf(WW_BRIDGE_LOG_INFO, "%s: GPU info", prefix ? prefix : "");
    /* Format: 2-space indent, label, colon, padding so values align,
     * then the value. NULL value renders as "(null)". */
    for (size_t i = 0; i < n_fields; i++) {
        const char* lbl = fields[i].label ? fields[i].label : "";
        const char* val = fields[i].value ? fields[i].value : "(null)";
        int         pad = (int)(max_label - strlen(lbl)) + 1;
        ww_bridge_logf(WW_BRIDGE_LOG_INFO, "  %s:%*s%s", lbl, pad, "", val);
    }
}

/* -----------------------------------------------------------------------
 * High-level control receive
 * ----------------------------------------------------------------------- */

int ww_bridge_recv_control(int sock, ww_bridge_control_t* out) {
    if (! out) return -EINVAL;
    memset(out, 0, sizeof(*out));

    uint16_t opcode;
    uint8_t* body     = NULL;
    size_t   body_len = 0;
    int      fds[WW_BRIDGE_MAX_FDS];
    size_t   n_fds = 0;

    int rc = ww_bridge_recv_frame(sock, &opcode, &body, &body_len, fds, WW_BRIDGE_MAX_FDS, &n_fds);
    if (rc != 0) return rc;

    /* Control requests carry no fds. If any arrive, close them and
     * surface the protocol violation. */
    for (size_t i = 0; i < n_fds; i++) close(fds[i]);
    if (n_fds > 0) {
        free(body);
        return WW_ERR_UNKNOWN_OPCODE; /* closest available code */
    }

    out->op = (ww_event_in_op_t)opcode;
    switch (out->op) {
    case WW_EVT_IN_INIT: rc = ww_evt_in_init_decode(body, body_len, &out->u.init); break;
    case WW_EVT_IN_SETTING_CHANGED:
        rc = ww_evt_in_setting_changed_decode(body, body_len, &out->u.setting_changed);
        break;
    case WW_EVT_IN_PLAY: rc = ww_evt_in_play_decode(body, body_len, &out->u.play); break;
    case WW_EVT_IN_PAUSE: rc = ww_evt_in_pause_decode(body, body_len, &out->u.pause); break;
    case WW_EVT_IN_MUTE: rc = ww_evt_in_mute_decode(body, body_len, &out->u.mute); break;
    case WW_EVT_IN_UNMUTE: rc = ww_evt_in_unmute_decode(body, body_len, &out->u.unmute); break;
    case WW_EVT_IN_POINTER_MOTION:
        rc = ww_evt_in_pointer_motion_decode(body, body_len, &out->u.pointer_motion);
        break;
    case WW_EVT_IN_POINTER_BUTTON:
        rc = ww_evt_in_pointer_button_decode(body, body_len, &out->u.pointer_button);
        break;
    case WW_EVT_IN_POINTER_AXIS:
        rc = ww_evt_in_pointer_axis_decode(body, body_len, &out->u.pointer_axis);
        break;
    case WW_EVT_IN_MPRIS: rc = ww_evt_in_mpris_decode(body, body_len, &out->u.mpris); break;
    case WW_EVT_IN_MPRIS_V2:
        rc = ww_evt_in_mpris_v2_decode(body, body_len, &out->u.mpris_v2);
        break;
    case WW_EVT_IN_EVENT_SUBSCRIPTIONS_APPLIED:
        rc = ww_evt_in_event_subscriptions_applied_decode(
            body, body_len, &out->u.event_subscriptions_applied);
        break;
    case WW_EVT_IN_AUDIO_WINDOW:
        rc = ww_evt_in_audio_window_decode(body, body_len, &out->u.audio_window);
        break;
    case WW_EVT_IN_SHUTDOWN:
        rc = ww_evt_in_shutdown_decode(body, body_len, &out->u.shutdown);
        break;
    case WW_EVT_IN_NEGOTIATE_BUFFERS:
        rc = ww_evt_in_negotiate_buffers_decode(body, body_len, &out->u.negotiate_buffers);
        break;
    case WW_EVT_IN_REQUEST_FRAME:
        rc = ww_evt_in_request_frame_decode(body, body_len, &out->u.request_frame);
        break;
    case WW_EVT_IN_SET_LOG_LEVEL:
        rc = ww_evt_in_set_log_level_decode(body, body_len, &out->u.set_log_level);
        break;
    case WW_EVT_IN_CONFIGURE_RENDER_TARGET:
        rc = ww_evt_in_configure_render_target_decode(
            body, body_len, &out->u.configure_render_target);
        break;
    default: rc = WW_ERR_UNKNOWN_OPCODE; break;
    }

    free(body);
    return rc;
}

void ww_bridge_control_free(ww_bridge_control_t* msg) {
    if (! msg) return;
    switch (msg->op) {
    case WW_EVT_IN_INIT: ww_evt_in_init_free(&msg->u.init); break;
    case WW_EVT_IN_SETTING_CHANGED: ww_evt_in_setting_changed_free(&msg->u.setting_changed); break;
    case WW_EVT_IN_PLAY: ww_evt_in_play_free(&msg->u.play); break;
    case WW_EVT_IN_PAUSE: ww_evt_in_pause_free(&msg->u.pause); break;
    case WW_EVT_IN_MUTE: ww_evt_in_mute_free(&msg->u.mute); break;
    case WW_EVT_IN_UNMUTE: ww_evt_in_unmute_free(&msg->u.unmute); break;
    case WW_EVT_IN_POINTER_MOTION: ww_evt_in_pointer_motion_free(&msg->u.pointer_motion); break;
    case WW_EVT_IN_POINTER_BUTTON: ww_evt_in_pointer_button_free(&msg->u.pointer_button); break;
    case WW_EVT_IN_POINTER_AXIS: ww_evt_in_pointer_axis_free(&msg->u.pointer_axis); break;
    case WW_EVT_IN_MPRIS: ww_evt_in_mpris_free(&msg->u.mpris); break;
    case WW_EVT_IN_MPRIS_V2: ww_evt_in_mpris_v2_free(&msg->u.mpris_v2); break;
    case WW_EVT_IN_EVENT_SUBSCRIPTIONS_APPLIED:
        ww_evt_in_event_subscriptions_applied_free(&msg->u.event_subscriptions_applied);
        break;
    case WW_EVT_IN_AUDIO_WINDOW: ww_evt_in_audio_window_free(&msg->u.audio_window); break;
    case WW_EVT_IN_SHUTDOWN: ww_evt_in_shutdown_free(&msg->u.shutdown); break;
    case WW_EVT_IN_NEGOTIATE_BUFFERS:
        ww_evt_in_negotiate_buffers_free(&msg->u.negotiate_buffers);
        break;
    case WW_EVT_IN_REQUEST_FRAME: ww_evt_in_request_frame_free(&msg->u.request_frame); break;
    case WW_EVT_IN_SET_LOG_LEVEL: ww_evt_in_set_log_level_free(&msg->u.set_log_level); break;
    case WW_EVT_IN_CONFIGURE_RENDER_TARGET:
        ww_evt_in_configure_render_target_free(&msg->u.configure_render_target);
        break;
    default: break;
    }
    memset(msg, 0, sizeof(*msg));
}

uint64_t ww_bridge_now_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) return 0;
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

int ww_bridge_negotiation_contains(const ww_negotiation_state_t* neg, uint32_t fourcc,
                                   uint64_t modifier) {
    if (! neg || ! neg->advertised) return 0;
    for (size_t i = 0; i < neg->advertised_count; ++i) {
        const ww_format_entry_t* e = &neg->advertised[i];
        if (e->fourcc == fourcc && e->modifier == modifier) return 1;
    }
    return 0;
}

void ww_bridge_negotiation_fill_format_caps(const ww_negotiation_state_t* neg,
                                            uint32_t* scratch_fourccs, uint32_t* scratch_mod_counts,
                                            uint64_t*                          scratch_modifiers,
                                            uint32_t*                          scratch_plane_counts,
                                            waywallen_producer_capabilities_t* out) {
    if (! neg || ! out) return;

    const uint32_t n            = (uint32_t)neg->advertised_count;
    uint32_t       fourcc_count = 0;

    /* Walk advertised, collapsing contiguous same-fourcc runs into
     * (fourccs[], mod_counts[]) and copying flat parallel arrays
     * for modifiers/plane_counts. */
    for (uint32_t i = 0; i < n; ++i) {
        const ww_format_entry_t* e = &neg->advertised[i];
        scratch_modifiers[i]       = e->modifier;
        scratch_plane_counts[i]    = e->plane_count;

        if (fourcc_count == 0 || scratch_fourccs[fourcc_count - 1] != e->fourcc) {
            scratch_fourccs[fourcc_count]    = e->fourcc;
            scratch_mod_counts[fourcc_count] = 1;
            ++fourcc_count;
        } else {
            ++scratch_mod_counts[fourcc_count - 1];
        }
    }

    out->fourccs      = (ww_array_u32_t) { .count = fourcc_count, .data = scratch_fourccs };
    out->mod_counts   = (ww_array_u32_t) { .count = fourcc_count, .data = scratch_mod_counts };
    out->modifiers    = (ww_array_u64_t) { .count = n, .data = scratch_modifiers };
    out->plane_counts = (ww_array_u32_t) { .count = n, .data = scratch_plane_counts };
}

/* -----------------------------------------------------------------------
 * Init handshake
 * ----------------------------------------------------------------------- */

int ww_bridge_recv_init(int sock, waywallen_renderer_init_t* out) {
    if (! out) return -EINVAL;
    memset(out, 0, sizeof(*out));

    ww_bridge_control_t ctl;
    int                 rc = ww_bridge_recv_control(sock, &ctl);
    if (rc != 0) return rc;

    if (ctl.op != WW_EVT_IN_INIT) {
        ww_bridge_control_free(&ctl);
        return -EPROTO;
    }

    /* Move the generated payload out of the control union. */
    *out = ctl.u.init.config;

    /* Zero the union members we just stole so `ww_bridge_control_free`
     * is safe even if a future refactor calls it. */
    memset(&ctl.u.init.config, 0, sizeof(ctl.u.init.config));

    if (out->protocol_version != WW_BRIDGE_SUPPORTED_PROTOCOL_VERSION ||
        out->spawn_version != WW_BRIDGE_SUPPORTED_SPAWN_VERSION) {
        return -EPROTO;
    }
    return 0;
}

int ww_bridge_send_init_nack(int sock, const waywallen_init_rejection_t* rejection) {
    if (! rejection) return -EINVAL;
    ww_evt_init_nack_t m = { .rejection = *rejection };
    WW_SEND_EVENT(sock, WW_EVT_INIT_NACK, ww_evt_init_nack_encode, &m, NULL, 0);
}

int ww_bridge_audio_window_from_control(const ww_bridge_control_t* ctrl,
                                        ww_bridge_audio_window_t*  out) {
    if (! ctrl || ! out) return -EINVAL;
    memset(out, 0, sizeof(*out));
    if (ctrl->op != WW_EVT_IN_AUDIO_WINDOW) return -EINVAL;

    const waywallen_audio_window_t* in          = &ctrl->u.audio_window.window;
    const uint32_t                  known_flags = WW_BRIDGE_AUDIO_END_OF_STREAM;
    if ((in->flags & ~known_flags) != 0) {
        return -EPROTO;
    }
    const int ended = (in->flags & WW_BRIDGE_AUDIO_END_OF_STREAM) != 0;
    if (ended) {
        if (in->samples.count != 0 || in->frames != 0 || in->format.sample_rate_hz != 0 ||
            in->format.channels != 0)
            return -EPROTO;
    } else if (in->format.sample_rate_hz != WW_BRIDGE_AUDIO_SAMPLE_RATE ||
               in->format.channels != WW_BRIDGE_AUDIO_CHANNELS ||
               in->frames != WW_BRIDGE_AUDIO_WINDOW_FRAMES ||
               in->samples.count != WW_BRIDGE_AUDIO_SAMPLE_COUNT || ! in->samples.data) {
        return -EPROTO;
    }
    for (uint32_t i = 0; i < in->samples.count; ++i) {
        if (! isfinite(in->samples.data[i])) {
            return -EPROTO;
        }
    }

    out->subscription_revision = in->subscription_revision;
    out->generation            = in->generation;
    out->sequence              = in->sequence;
    out->captured_at_ns        = in->captured_at_ns;
    out->end_sample_frame      = in->end_sample_frame;
    out->sample_rate_hz        = in->format.sample_rate_hz;
    out->channels              = in->format.channels;
    out->frames                = in->frames;
    out->flags                 = in->flags;
    if (in->samples.count > 0) memcpy(out->samples, in->samples.data, sizeof(out->samples));
    return 0;
}
