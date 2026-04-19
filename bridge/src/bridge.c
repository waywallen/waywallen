/* waywallen-bridge — IPC framing + high-level helpers.
 *
 * Handwritten companion to the auto-generated src/ipc_v1.c. Provides
 * SCM_RIGHTS fd passing on top of the generated per-message encoders
 * and a tagged union for incoming control requests.
 */
#include <waywallen-bridge/bridge.h>

#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/un.h>
#include <unistd.h>

/* Keep in sync with waywallen's MAX_FDS_PER_MSG. 64 is generous for
 * the protocol's current needs (BindBuffers with ~8 planes) and keeps
 * the CMSG scratch buffer stack-allocatable. */
#define WW_BRIDGE_MAX_FDS 64

/* Max inline body: u16 total length minus 4-byte header. */
#define WW_BRIDGE_MAX_BODY (65535 - 4)

/* -----------------------------------------------------------------------
 * Connection
 * ----------------------------------------------------------------------- */

int ww_bridge_connect(const char *socket_path) {
    if (!socket_path) return -EINVAL;

    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) return -errno;

    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    size_t plen = strlen(socket_path);
    if (plen >= sizeof(addr.sun_path)) {
        close(fd);
        return -ENAMETOOLONG;
    }
    memcpy(addr.sun_path, socket_path, plen + 1);

    if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
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

static int write_all(int fd, const void *buf, size_t len) {
    const uint8_t *p = (const uint8_t *)buf;
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

static int read_all(int fd, void *buf, size_t len) {
    uint8_t *p = (uint8_t *)buf;
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

int ww_bridge_send_frame(int sock,
                         uint16_t opcode,
                         const uint8_t *body,
                         size_t body_len,
                         const int *fds,
                         size_t n_fds) {
    if (body_len > WW_BRIDGE_MAX_BODY) return -EMSGSIZE;
    if (n_fds > WW_BRIDGE_MAX_FDS) return -E2BIG;

    uint8_t header[4];
    uint16_t total = (uint16_t)(body_len + 4);
    header[0] = (uint8_t)(opcode & 0xff);
    header[1] = (uint8_t)((opcode >> 8) & 0xff);
    header[2] = (uint8_t)(total & 0xff);
    header[3] = (uint8_t)((total >> 8) & 0xff);

    /* Single sendmsg so SCM_RIGHTS attaches atomically to the header.
     * We pack header+body into two iovecs to avoid copying. */
    struct iovec iov[2];
    iov[0].iov_base = header;
    iov[0].iov_len = 4;
    int iov_count = 1;
    if (body_len > 0) {
        iov[1].iov_base = (void *)body;
        iov[1].iov_len = body_len;
        iov_count = 2;
    }

    /* Control message space for SCM_RIGHTS fds. */
    union {
        char buf[CMSG_SPACE(sizeof(int) * WW_BRIDGE_MAX_FDS)];
        struct cmsghdr align;
    } cmsg_storage;

    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = iov;
    msg.msg_iovlen = iov_count;

    if (n_fds > 0) {
        msg.msg_control = cmsg_storage.buf;
        msg.msg_controllen = CMSG_SPACE(sizeof(int) * n_fds);
        struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
        cmsg->cmsg_level = SOL_SOCKET;
        cmsg->cmsg_type = SCM_RIGHTS;
        cmsg->cmsg_len = CMSG_LEN(sizeof(int) * n_fds);
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
        size_t done = (size_t)n;
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

int ww_bridge_recv_frame(int sock,
                         uint16_t *opcode_out,
                         uint8_t **body_out,
                         size_t *body_len_out,
                         int *fds_out,
                         size_t fds_cap,
                         size_t *n_fds_out) {
    if (!opcode_out || !body_out || !body_len_out || !n_fds_out) return -EINVAL;

    *body_out = NULL;
    *body_len_out = 0;
    *n_fds_out = 0;

    /* Phase 1: read the 4-byte header via recvmsg to harvest any cmsg
     * fds that attach to the first byte of the frame. The while loop
     * handles short reads without losing ancillary data. */
    uint8_t header[4];
    size_t filled = 0;
    while (filled < 4) {
        struct iovec iov;
        iov.iov_base = header + filled;
        iov.iov_len = 4 - filled;

        union {
            char buf[CMSG_SPACE(sizeof(int) * WW_BRIDGE_MAX_FDS)];
            struct cmsghdr align;
        } cmsg_storage;

        struct msghdr msg;
        memset(&msg, 0, sizeof(msg));
        msg.msg_iov = &iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_storage.buf;
        msg.msg_controllen = sizeof(cmsg_storage.buf);

        ssize_t n;
        do {
            n = recvmsg(sock, &msg, MSG_CMSG_CLOEXEC);
        } while (n < 0 && errno == EINTR);

        if (n < 0) return -errno;
        if (n == 0) return -ENOTCONN;

        for (struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg); cmsg;
             cmsg = CMSG_NXTHDR(&msg, cmsg)) {
            if (cmsg->cmsg_level == SOL_SOCKET && cmsg->cmsg_type == SCM_RIGHTS) {
                size_t payload = cmsg->cmsg_len - CMSG_LEN(0);
                size_t got = payload / sizeof(int);
                const int *in_fds = (const int *)CMSG_DATA(cmsg);
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
    uint8_t *body = NULL;
    if (body_len > 0) {
        body = (uint8_t *)malloc(body_len);
        if (!body) {
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

    *opcode_out = opcode;
    *body_out = body;
    *body_len_out = body_len;
    return 0;
}


/* -----------------------------------------------------------------------
 * High-level event senders
 * ----------------------------------------------------------------------- */

/* Helper: encode + frame + send. */
#define WW_SEND_EVENT(sock, op_enum, encode_fn, msg_ptr, fds_ptr, n_fds) \
    do {                                                                 \
        ww_buf_t buf;                                                    \
        ww_buf_init(&buf);                                               \
        int rc = encode_fn((msg_ptr), &buf);                             \
        if (rc != WW_OK) {                                               \
            ww_buf_free(&buf);                                           \
            return rc;                                                   \
        }                                                                \
        rc = ww_bridge_send_frame((sock), (op_enum), buf.data, buf.len,  \
                                  (fds_ptr), (n_fds));                   \
        ww_buf_free(&buf);                                               \
        return rc;                                                       \
    } while (0)

int ww_bridge_send_ready(int sock) {
    ww_evt_ready_t m = { 0 };
    WW_SEND_EVENT(sock, WW_EVT_READY, ww_evt_ready_encode, &m, NULL, 0);
}

int ww_bridge_send_bind_buffers(int sock,
                                const ww_evt_bind_buffers_t *m,
                                const int *fds) {
    if (!m || !fds) return -EINVAL;
    WW_SEND_EVENT(sock, WW_EVT_BIND_BUFFERS, ww_evt_bind_buffers_encode,
                  m, fds, m->count);
}

int ww_bridge_send_frame_ready(int sock,
                               const ww_evt_frame_ready_t *m,
                               int sync_fd) {
    if (!m || sync_fd < 0) return -EINVAL;
    WW_SEND_EVENT(sock, WW_EVT_FRAME_READY, ww_evt_frame_ready_encode,
                  m, &sync_fd, 1);
}

int ww_bridge_send_error(int sock, const char *msg) {
    if (!msg) return -EINVAL;
    ww_evt_error_t m;
    m.msg = (char *)msg; /* encoder doesn't mutate */
    WW_SEND_EVENT(sock, WW_EVT_ERROR, ww_evt_error_encode, &m, NULL, 0);
}


/* -----------------------------------------------------------------------
 * High-level control receive
 * ----------------------------------------------------------------------- */

int ww_bridge_recv_control(int sock, ww_bridge_control_t *out) {
    if (!out) return -EINVAL;
    memset(out, 0, sizeof(*out));

    uint16_t opcode;
    uint8_t *body = NULL;
    size_t body_len = 0;
    int fds[WW_BRIDGE_MAX_FDS];
    size_t n_fds = 0;

    int rc = ww_bridge_recv_frame(sock, &opcode, &body, &body_len,
                                  fds, WW_BRIDGE_MAX_FDS, &n_fds);
    if (rc != 0) return rc;

    /* Control requests carry no fds. If any arrive, close them and
     * surface the protocol violation. */
    for (size_t i = 0; i < n_fds; i++) close(fds[i]);
    if (n_fds > 0) {
        free(body);
        return WW_ERR_UNKNOWN_OPCODE; /* closest available code */
    }

    out->op = (ww_request_op_t)opcode;
    switch (out->op) {
    case WW_REQ_HELLO:
        rc = ww_req_hello_decode(body, body_len, &out->u.hello);
        break;
    case WW_REQ_LOAD_SCENE:
        rc = ww_req_load_scene_decode(body, body_len, &out->u.load_scene);
        break;
    case WW_REQ_PLAY:
        rc = ww_req_play_decode(body, body_len, &out->u.play);
        break;
    case WW_REQ_PAUSE:
        rc = ww_req_pause_decode(body, body_len, &out->u.pause);
        break;
    case WW_REQ_MOUSE:
        rc = ww_req_mouse_decode(body, body_len, &out->u.mouse);
        break;
    case WW_REQ_SET_FPS:
        rc = ww_req_set_fps_decode(body, body_len, &out->u.set_fps);
        break;
    case WW_REQ_SHUTDOWN:
        rc = ww_req_shutdown_decode(body, body_len, &out->u.shutdown);
        break;
    default:
        rc = WW_ERR_UNKNOWN_OPCODE;
        break;
    }

    free(body);
    return rc;
}

void ww_bridge_control_free(ww_bridge_control_t *msg) {
    if (!msg) return;
    switch (msg->op) {
    case WW_REQ_HELLO:      ww_req_hello_free(&msg->u.hello); break;
    case WW_REQ_LOAD_SCENE: ww_req_load_scene_free(&msg->u.load_scene); break;
    case WW_REQ_PLAY:       ww_req_play_free(&msg->u.play); break;
    case WW_REQ_PAUSE:      ww_req_pause_free(&msg->u.pause); break;
    case WW_REQ_MOUSE:      ww_req_mouse_free(&msg->u.mouse); break;
    case WW_REQ_SET_FPS:    ww_req_set_fps_free(&msg->u.set_fps); break;
    case WW_REQ_SHUTDOWN:   ww_req_shutdown_free(&msg->u.shutdown); break;
    default: break;
    }
    memset(msg, 0, sizeof(*msg));
}
