// waywallen-mpv-renderer — libmpv + OpenGL ES/EGL video renderer subprocess
// for the waywallen daemon. Spawned for wallpapers of type "video".

#include <waywallen-bridge/bridge.h>

#include <mpv/client.h>
#include <mpv/render.h>
#include <mpv/render_gl.h>

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <GLES2/gl2ext.h>

#include <gbm.h>
#include <fcntl.h>

#ifndef DRM_FORMAT_MOD_LINEAR
#define DRM_FORMAT_MOD_LINEAR 0ULL
#endif

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <thread>

#include <sys/prctl.h>
#include <sys/socket.h>
#include <unistd.h>

namespace {

constexpr uint32_t SLOT_COUNT          = 3;
// DRM_FORMAT_ABGR8888 == fourcc('A','B','2','4'): memory order R,G,B,A.
// This is what Mesa reports for a GL_RGBA8 texture exported as DMA-BUF.
constexpr uint32_t DRM_FORMAT_ABGR8888 = 0x34324241u;

struct Options {
    std::string ipc_path;
    std::string video_path;
    uint32_t    width { 1280 };
    uint32_t    height { 720 };
    bool        loop_file { true };
    bool        hwdec { true };
};

[[noreturn]] void die(const std::string& msg) {
    std::fprintf(stderr, "waywallen-mpv-renderer: %s\n", msg.c_str());
    std::exit(1);
}

Options parse_args(int argc, char** argv) {
    Options o;
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        auto next = [&]() -> std::string {
            if (i + 1 >= argc) return {};
            return argv[++i];
        };
        if (a == "--ipc") {
            o.ipc_path = next();
        } else if (a == "--width") {
            o.width = static_cast<uint32_t>(std::strtoul(next().c_str(), nullptr, 10));
        } else if (a == "--height") {
            o.height = static_cast<uint32_t>(std::strtoul(next().c_str(), nullptr, 10));
        } else if (a == "--video" || a == "--path") {
            o.video_path = next();
        } else if (a == "--no-hwdec") {
            o.hwdec = false;
        } else if (a == "--no-loop") {
            o.loop_file = false;
        } else {
            // Swallow unknown --key value pairs forwarded from daemon
            // source-plugin metadata. Notably drops --fps: pacing is now
            // fully driven by libmpv's render-update callback.
            if (!a.empty() && a.rfind("--", 0) == 0 && i + 1 < argc
                && std::string(argv[i + 1]).rfind("--", 0) != 0) {
                ++i;
            }
        }
    }
    return o;
}

uint64_t now_ns() {
    const auto t = std::chrono::steady_clock::now().time_since_epoch();
    return static_cast<uint64_t>(
        std::chrono::duration_cast<std::chrono::nanoseconds>(t).count());
}


// ---------------------------------------------------------------------------
// EGL / GLES
// ---------------------------------------------------------------------------

struct GlFns {
    PFNEGLGETPLATFORMDISPLAYEXTPROC          eglGetPlatformDisplayEXT { nullptr };
    PFNEGLCREATEIMAGEKHRPROC                 eglCreateImageKHR { nullptr };
    PFNEGLDESTROYIMAGEKHRPROC                eglDestroyImageKHR { nullptr };
    PFNEGLCREATESYNCKHRPROC                  eglCreateSyncKHR { nullptr };
    PFNEGLDESTROYSYNCKHRPROC                 eglDestroySyncKHR { nullptr };
    PFNEGLDUPNATIVEFENCEFDANDROIDPROC        eglDupNativeFenceFDANDROID { nullptr };
    PFNGLEGLIMAGETARGETTEXTURE2DOESPROC      glEGLImageTargetTexture2DOES { nullptr };
};

struct Slot {
    GLuint         texture { 0 };
    GLuint         fbo { 0 };
    EGLImageKHR    egl_image { EGL_NO_IMAGE_KHR };
    struct gbm_bo* bo { nullptr };
    int            dmabuf_fd { -1 };
    uint64_t       drm_modifier { 0 };
    uint32_t       size { 0 };
    uint32_t       stride { 0 };
    uint32_t       offset { 0 };
};

struct GlCtx {
    int                drm_fd { -1 };
    struct gbm_device* gbm { nullptr };
    EGLDisplay         display { EGL_NO_DISPLAY };
    EGLContext         context { EGL_NO_CONTEXT };
    GlFns              fns;
    Slot               slots[SLOT_COUNT];
};

void* must_egl_proc(const char* name) {
    void* p = reinterpret_cast<void*>(eglGetProcAddress(name));
    if (!p) die(std::string("eglGetProcAddress missing: ") + name);
    return p;
}

bool egl_has_ext(const char* exts, const char* e) {
    return exts && std::strstr(exts, e) != nullptr;
}

void open_render_node(GlCtx& gl) {
    const char* nodes[] = {
        "/dev/dri/renderD128",
        "/dev/dri/renderD129",
    };
    for (const char* n : nodes) {
        int fd = ::open(n, O_RDWR | O_CLOEXEC);
        if (fd >= 0) {
            gl.drm_fd = fd;
            break;
        }
    }
    if (gl.drm_fd < 0) die("no /dev/dri/renderD* could be opened");
    gl.gbm = gbm_create_device(gl.drm_fd);
    if (!gl.gbm) die("gbm_create_device failed");
}

void init_egl(GlCtx& gl, const Options& opt) {
    open_render_node(gl);

    gl.fns.eglGetPlatformDisplayEXT =
        reinterpret_cast<PFNEGLGETPLATFORMDISPLAYEXTPROC>(
            must_egl_proc("eglGetPlatformDisplayEXT"));

    // Headless rendering — no window, no output device needed. Mesa's
    // surfaceless platform is what lets us run a pure DMA-BUF producer.
    gl.display = gl.fns.eglGetPlatformDisplayEXT(
        EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, nullptr);
    if (gl.display == EGL_NO_DISPLAY) {
        die("eglGetPlatformDisplay(SURFACELESS_MESA) failed; Mesa required");
    }

    EGLint major = 0, minor = 0;
    if (!eglInitialize(gl.display, &major, &minor)) {
        die("eglInitialize failed");
    }

    const char* exts = eglQueryString(gl.display, EGL_EXTENSIONS);
    if (!egl_has_ext(exts, "EGL_KHR_surfaceless_context"))
        die("EGL_KHR_surfaceless_context missing");
    if (!egl_has_ext(exts, "EGL_KHR_image_base"))
        die("EGL_KHR_image_base missing");
    if (!egl_has_ext(exts, "EGL_EXT_image_dma_buf_import")
        || !egl_has_ext(exts, "EGL_EXT_image_dma_buf_import_modifiers"))
        die("EGL DMA-BUF import (modifiers) extensions missing");
    if (!egl_has_ext(exts, "EGL_KHR_fence_sync")
        || !egl_has_ext(exts, "EGL_ANDROID_native_fence_sync"))
        die("EGL fence-sync extensions missing");

    if (!eglBindAPI(EGL_OPENGL_ES_API)) die("eglBindAPI(GLES) failed");

    EGLint config_attrs[] = {
        EGL_SURFACE_TYPE,    EGL_PBUFFER_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES3_BIT,
        EGL_NONE,
    };
    EGLConfig config;
    EGLint    n_configs = 0;
    if (!eglChooseConfig(gl.display, config_attrs, &config, 1, &n_configs)
        || n_configs < 1) {
        die("eglChooseConfig: no GLES3 pbuffer config");
    }

    EGLint ctx_attrs[] = {
        EGL_CONTEXT_MAJOR_VERSION, 3,
        EGL_CONTEXT_MINOR_VERSION, 0,
        EGL_NONE,
    };
    gl.context = eglCreateContext(gl.display, config, EGL_NO_CONTEXT, ctx_attrs);
    if (gl.context == EGL_NO_CONTEXT) die("eglCreateContext failed");

    if (!eglMakeCurrent(gl.display, EGL_NO_SURFACE, EGL_NO_SURFACE, gl.context))
        die("eglMakeCurrent(surfaceless) failed");

    gl.fns.eglCreateImageKHR =
        reinterpret_cast<PFNEGLCREATEIMAGEKHRPROC>(
            must_egl_proc("eglCreateImageKHR"));
    gl.fns.eglDestroyImageKHR =
        reinterpret_cast<PFNEGLDESTROYIMAGEKHRPROC>(
            must_egl_proc("eglDestroyImageKHR"));
    gl.fns.eglCreateSyncKHR =
        reinterpret_cast<PFNEGLCREATESYNCKHRPROC>(
            must_egl_proc("eglCreateSyncKHR"));
    gl.fns.eglDestroySyncKHR =
        reinterpret_cast<PFNEGLDESTROYSYNCKHRPROC>(
            must_egl_proc("eglDestroySyncKHR"));
    gl.fns.eglDupNativeFenceFDANDROID =
        reinterpret_cast<PFNEGLDUPNATIVEFENCEFDANDROIDPROC>(
            must_egl_proc("eglDupNativeFenceFDANDROID"));
    // GL_OES_EGL_image: required to wrap an EGLImage as a GL_TEXTURE_2D.
    const GLubyte* gl_exts = glGetString(GL_EXTENSIONS);
    if (!gl_exts
        || !std::strstr(reinterpret_cast<const char*>(gl_exts),
                        "GL_OES_EGL_image")) {
        die("GL_OES_EGL_image missing");
    }
    gl.fns.glEGLImageTargetTexture2DOES =
        reinterpret_cast<PFNGLEGLIMAGETARGETTEXTURE2DOESPROC>(
            must_egl_proc("glEGLImageTargetTexture2DOES"));

    // Allocate each slot as a LINEAR DMA-BUF via GBM, import as EGLImage
    // with an explicit modifier, then wrap as GL_TEXTURE_2D + FBO. This
    // avoids EGL_MESA_image_dma_buf_export's "modifier=INVALID" pitfall
    // where the producer can't tell the consumer the true tiling.
    const uint64_t linear_mods[] = { DRM_FORMAT_MOD_LINEAR };
    for (uint32_t i = 0; i < SLOT_COUNT; ++i) {
        Slot& s = gl.slots[i];

        s.bo = gbm_bo_create_with_modifiers2(
            gl.gbm, opt.width, opt.height, GBM_FORMAT_ABGR8888,
            linear_mods, 1, GBM_BO_USE_RENDERING);
        if (!s.bo) die("gbm_bo_create_with_modifiers2(LINEAR) failed");

        s.dmabuf_fd    = gbm_bo_get_fd(s.bo);
        if (s.dmabuf_fd < 0) die("gbm_bo_get_fd failed");
        s.stride       = gbm_bo_get_stride(s.bo);
        s.offset       = gbm_bo_get_offset(s.bo, 0);
        s.drm_modifier = gbm_bo_get_modifier(s.bo);
        // Conservative byte count, used only by the daemon for accounting.
        s.size         = s.stride * opt.height;

        const EGLint mod_lo = static_cast<EGLint>(
            s.drm_modifier & 0xffffffffULL);
        const EGLint mod_hi = static_cast<EGLint>(
            (s.drm_modifier >> 32) & 0xffffffffULL);
        EGLint attrs[] = {
            EGL_WIDTH,                          static_cast<EGLint>(opt.width),
            EGL_HEIGHT,                         static_cast<EGLint>(opt.height),
            EGL_LINUX_DRM_FOURCC_EXT,           static_cast<EGLint>(DRM_FORMAT_ABGR8888),
            EGL_DMA_BUF_PLANE0_FD_EXT,          s.dmabuf_fd,
            EGL_DMA_BUF_PLANE0_OFFSET_EXT,      static_cast<EGLint>(s.offset),
            EGL_DMA_BUF_PLANE0_PITCH_EXT,       static_cast<EGLint>(s.stride),
            EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, mod_lo,
            EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, mod_hi,
            EGL_NONE,
        };
        s.egl_image = gl.fns.eglCreateImageKHR(
            gl.display, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT,
            nullptr, attrs);
        if (s.egl_image == EGL_NO_IMAGE_KHR)
            die("eglCreateImageKHR(EGL_LINUX_DMA_BUF_EXT) failed");

        glGenTextures(1, &s.texture);
        glBindTexture(GL_TEXTURE_2D, s.texture);
        gl.fns.glEGLImageTargetTexture2DOES(GL_TEXTURE_2D, s.egl_image);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);

        glGenFramebuffers(1, &s.fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, s.fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0,
                               GL_TEXTURE_2D, s.texture, 0);
        if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE)
            die("slot FBO incomplete");
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
    }
}

int export_sync_fd(GlCtx& gl) {
    EGLSyncKHR sync = gl.fns.eglCreateSyncKHR(
        gl.display, EGL_SYNC_NATIVE_FENCE_ANDROID, nullptr);
    if (sync == EGL_NO_SYNC_KHR) return -1;
    // Ensure the sync is inserted into the command stream *before* we
    // dup the fd, otherwise the native fence may be empty.
    glFlush();
    int fd = gl.fns.eglDupNativeFenceFDANDROID(gl.display, sync);
    gl.fns.eglDestroySyncKHR(gl.display, sync);
    if (fd == EGL_NO_NATIVE_FENCE_FD_ANDROID) return -1;
    return fd;
}

void destroy_gl(GlCtx& gl) {
    if (gl.display != EGL_NO_DISPLAY) {
        for (auto& s : gl.slots) {
            if (s.egl_image != EGL_NO_IMAGE_KHR)
                gl.fns.eglDestroyImageKHR(gl.display, s.egl_image);
            if (s.fbo)     glDeleteFramebuffers(1, &s.fbo);
            if (s.texture) glDeleteTextures(1, &s.texture);
            if (s.dmabuf_fd >= 0) close(s.dmabuf_fd);
            if (s.bo) gbm_bo_destroy(s.bo);
        }
        eglMakeCurrent(gl.display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
        if (gl.context != EGL_NO_CONTEXT)
            eglDestroyContext(gl.display, gl.context);
        eglTerminate(gl.display);
    }
    if (gl.gbm) gbm_device_destroy(gl.gbm);
    if (gl.drm_fd >= 0) close(gl.drm_fd);
}


// ---------------------------------------------------------------------------
// mpv
// ---------------------------------------------------------------------------

struct MpvState {
    mpv_handle*         mpv { nullptr };
    mpv_render_context* ctx { nullptr };
};

struct WakeState {
    std::mutex              mu;
    std::condition_variable cv;
    bool                    pending { false };
};

void on_mpv_render_update(void* ctx) {
    auto* w = static_cast<WakeState*>(ctx);
    {
        std::lock_guard<std::mutex> lk(w->mu);
        w->pending = true;
    }
    w->cv.notify_one();
}

void* mpv_get_proc_address(void* /*ctx*/, const char* name) {
    return reinterpret_cast<void*>(eglGetProcAddress(name));
}

void mpv_init(MpvState& m, const Options& opt, WakeState& wake) {
    m.mpv = mpv_create();
    if (!m.mpv) die("mpv_create failed");

    mpv_set_option_string(m.mpv, "vo",                     "libmpv");
    mpv_set_option_string(m.mpv, "audio",                  "no");
    mpv_set_option_string(m.mpv, "terminal",               "no");
    mpv_set_option_string(m.mpv, "msg-level",              "all=warn");
    mpv_set_option_string(m.mpv, "loop-file",
                          opt.loop_file ? "inf" : "no");
    // With the GL render API, hwdec can stay on GPU (vaapi-egl etc.),
    // so "auto-safe" is the right default; SW render path used to stall.
    mpv_set_option_string(m.mpv, "hwdec",
                          opt.hwdec ? "auto-safe" : "no");
    mpv_set_option_string(m.mpv, "keep-open",              "always");
    mpv_set_option_string(m.mpv, "input-default-bindings", "no");
    mpv_set_option_string(m.mpv, "input-vo-keyboard",      "no");

    if (int rc = mpv_initialize(m.mpv); rc < 0)
        die(std::string("mpv_initialize: ") + mpv_error_string(rc));

    mpv_opengl_init_params gl_params {};
    gl_params.get_proc_address     = mpv_get_proc_address;
    gl_params.get_proc_address_ctx = nullptr;

    mpv_render_param create_params[] = {
        { MPV_RENDER_PARAM_API_TYPE,
          const_cast<char*>(MPV_RENDER_API_TYPE_OPENGL) },
        { MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &gl_params },
        { MPV_RENDER_PARAM_INVALID, nullptr },
    };
    if (int rc = mpv_render_context_create(&m.ctx, m.mpv, create_params); rc < 0)
        die(std::string("mpv_render_context_create: ")
            + mpv_error_string(rc));

    mpv_render_context_set_update_callback(m.ctx, on_mpv_render_update, &wake);

    if (!opt.video_path.empty()) {
        const char* cmd[] = { "loadfile", opt.video_path.c_str(), nullptr };
        if (int rc = mpv_command(m.mpv, cmd); rc < 0) {
            std::fprintf(stderr,
                         "waywallen-mpv-renderer: loadfile %s failed: %s\n",
                         opt.video_path.c_str(), mpv_error_string(rc));
        }
    }
}

bool mpv_render_into_slot(MpvState& m, GlCtx& gl, uint32_t slot,
                          const Options& opt) {
    mpv_opengl_fbo fbo_info {};
    fbo_info.fbo             = static_cast<int>(gl.slots[slot].fbo);
    fbo_info.w               = static_cast<int>(opt.width);
    fbo_info.h               = static_cast<int>(opt.height);
    fbo_info.internal_format = 0;

    // DMA-BUF rows are stored top-to-bottom (DRM convention). A GL FBO
    // backed by a DMA-BUF-imported texture therefore maps window
    // coordinate y=0 to DMA-BUF row 0 already — no extra flip needed
    // on mpv's side. (The old export path used the opposite convention.)
    int flip_y = 0;
    mpv_render_param params[] = {
        { MPV_RENDER_PARAM_OPENGL_FBO, &fbo_info },
        { MPV_RENDER_PARAM_FLIP_Y,     &flip_y },
        { MPV_RENDER_PARAM_INVALID,    nullptr },
    };
    return mpv_render_context_render(m.ctx, params) >= 0;
}

void mpv_drain_events(MpvState& m, std::atomic<bool>& shutdown) {
    while (true) {
        mpv_event* ev = mpv_wait_event(m.mpv, 0.0);
        if (!ev || ev->event_id == MPV_EVENT_NONE) break;
        if (ev->event_id == MPV_EVENT_SHUTDOWN)
            shutdown.store(true, std::memory_order_release);
    }
}


// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

struct HostState {
    int                   sock { -1 };
    std::mutex            send_mu; // serializes sendmsg on `sock`
    std::atomic<bool>     shutdown { false };
    std::atomic<uint64_t> seq { 0 };
};

void wake_up(WakeState& w) {
    {
        std::lock_guard<std::mutex> lk(w.mu);
        w.pending = true;
    }
    w.cv.notify_one();
}

void send_bind(HostState& s, const Options& opt, GlCtx& gl) {
    uint64_t sizes[SLOT_COUNT];
    int      fds[SLOT_COUNT];
    for (uint32_t i = 0; i < SLOT_COUNT; ++i) {
        sizes[i] = gl.slots[i].size;
        fds[i]   = gl.slots[i].dmabuf_fd;
    }

    ww_evt_bind_buffers_t bb {};
    bb.count        = SLOT_COUNT;
    bb.fourcc       = DRM_FORMAT_ABGR8888;
    bb.width        = opt.width;
    bb.height       = opt.height;
    bb.stride       = gl.slots[0].stride;
    bb.modifier     = gl.slots[0].drm_modifier;
    bb.plane_offset = gl.slots[0].offset;
    bb.sizes.count  = SLOT_COUNT;
    bb.sizes.data   = sizes;

    std::lock_guard<std::mutex> lock(s.send_mu);
    int rc = ww_bridge_send_bind_buffers(s.sock, &bb, fds);
    if (rc != 0) die("send bind_buffers failed: " + std::to_string(rc));
}

void send_frame(HostState& s, GlCtx& gl, uint32_t slot) {
    int sync_fd = export_sync_fd(gl);
    if (sync_fd < 0) {
        std::fprintf(stderr,
                     "waywallen-mpv-renderer: eglDupNativeFenceFDANDROID failed\n");
        s.shutdown.store(true, std::memory_order_release);
        return;
    }

    ww_evt_frame_ready_t fr {};
    fr.image_index = slot;
    fr.seq         = s.seq.fetch_add(1, std::memory_order_relaxed);
    fr.ts_ns       = now_ns();

    int rc;
    {
        std::lock_guard<std::mutex> lock(s.send_mu);
        rc = ww_bridge_send_frame_ready(s.sock, &fr, sync_fd);
    }
    // SCM_RIGHTS dup'd the fd on success; close our copy either way.
    close(sync_fd);
    if (rc != 0) {
        std::fprintf(stderr,
                     "waywallen-mpv-renderer: send frame_ready failed: %d\n",
                     rc);
        s.shutdown.store(true, std::memory_order_release);
    }
}


// ---------------------------------------------------------------------------
// Control reader
// ---------------------------------------------------------------------------

void apply_control(HostState& s, MpvState& m, const ww_bridge_control_t& c) {
    switch (c.op) {
    case WW_REQ_HELLO:
        break;
    case WW_REQ_LOAD_SCENE:
        if (c.u.load_scene.pkg && c.u.load_scene.pkg[0]) {
            const char* cmd[] = { "loadfile", c.u.load_scene.pkg, nullptr };
            mpv_command(m.mpv, cmd);
        }
        break;
    case WW_REQ_PLAY: {
        int v = 0;
        mpv_set_property(m.mpv, "pause", MPV_FORMAT_FLAG, &v);
        break;
    }
    case WW_REQ_PAUSE: {
        int v = 1;
        mpv_set_property(m.mpv, "pause", MPV_FORMAT_FLAG, &v);
        break;
    }
    case WW_REQ_MOUSE:
        // Videos don't respond to mouse input today.
        break;
    case WW_REQ_SET_FPS:
        // libmpv paces itself to the media's native frame rate.
        break;
    case WW_REQ_SHUTDOWN:
        s.shutdown.store(true, std::memory_order_release);
        break;
    default:
        std::fprintf(stderr,
                     "waywallen-mpv-renderer: unknown control op %d\n",
                     static_cast<int>(c.op));
        break;
    }
}

void reader_loop(HostState& s, MpvState& m, WakeState& wake) {
    while (!s.shutdown.load(std::memory_order_acquire)) {
        ww_bridge_control_t msg {};
        int                 rc = ww_bridge_recv_control(s.sock, &msg);
        if (rc != 0) {
            if (!s.shutdown.load(std::memory_order_acquire)) {
                std::fprintf(stderr,
                             "waywallen-mpv-renderer: recv_control failed: %d\n",
                             rc);
            }
            s.shutdown.store(true, std::memory_order_release);
            wake_up(wake);
            return;
        }
        apply_control(s, m, msg);
        ww_bridge_control_free(&msg);
    }
}

} // namespace


// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

int main(int argc, char** argv) {
    Options opt = parse_args(argc, argv);
    if (opt.ipc_path.empty()) die("--ipc <socket_path> is required");

    ::prctl(PR_SET_PDEATHSIG, SIGTERM);

    GlCtx gl;
    init_egl(gl, opt);

    WakeState wake;
    MpvState  mpv;
    mpv_init(mpv, opt, wake);

    HostState host;
    host.sock = ww_bridge_connect(opt.ipc_path.c_str());
    if (host.sock < 0)
        die("ww_bridge_connect: " + std::string(std::strerror(-host.sock)));

    if (int rc = ww_bridge_send_ready(host.sock); rc != 0)
        die("send ready failed: " + std::to_string(rc));

    send_bind(host, opt, gl);

    std::thread reader([&]() { reader_loop(host, mpv, wake); });

    uint32_t slot = 0;
    while (!host.shutdown.load(std::memory_order_acquire)) {
        // Block until mpv signals a new update (or we're shutting down).
        // This replaces the old 5ms polling sleep and removes the external
        // fps gate entirely — pacing is libmpv's responsibility.
        {
            std::unique_lock<std::mutex> lk(wake.mu);
            wake.cv.wait(lk, [&] {
                return wake.pending
                       || host.shutdown.load(std::memory_order_acquire);
            });
            wake.pending = false;
        }
        if (host.shutdown.load(std::memory_order_acquire)) break;

        mpv_drain_events(mpv, host.shutdown);
        if (host.shutdown.load(std::memory_order_acquire)) break;

        const uint64_t update = mpv_render_context_update(mpv.ctx);
        if (!(update & MPV_RENDER_UPDATE_FRAME)) continue;

        if (!mpv_render_into_slot(mpv, gl, slot, opt)) continue;

        send_frame(host, gl, slot);

        slot = (slot + 1) % SLOT_COUNT;
    }

    // --- Shutdown ---------------------------------------------------------
    // Flush any outstanding GL work before we tear mpv down.
    glFinish();

    if (mpv.ctx) mpv_render_context_free(mpv.ctx);
    if (mpv.mpv) mpv_terminate_destroy(mpv.mpv);

    if (reader.joinable()) {
        ::shutdown(host.sock, SHUT_RD);
        reader.join();
    }
    ww_bridge_close(host.sock);

    destroy_gl(gl);
    return 0;
}
