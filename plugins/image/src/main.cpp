// waywallen-image-renderer — FFmpeg-decoded still image renderer subprocess
// for the waywallen daemon. Spawned for wallpapers of type "image".
//

#include <waywallen-bridge/bridge.h>

#include "av_image.hpp"
#include "vk_producer.hpp"

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

struct Options {
    std::string ipc_path;
    std::string image_path;
    uint32_t    width { 1920 };
    uint32_t    height { 1080 };
    bool        decode_only { false };
    bool        vulkan_probe { false };
    bool        produce_once { false };
};

uint64_t now_ns() {
    const auto t = std::chrono::steady_clock::now().time_since_epoch();
    return static_cast<uint64_t>(
        std::chrono::duration_cast<std::chrono::nanoseconds>(t).count());
}

[[noreturn]] void die(const std::string& msg) {
    std::fprintf(stderr, "waywallen-image-renderer: %s\n", msg.c_str());
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
        } else if (a == "--image" || a == "--path") {
            o.image_path = next();
        } else if (a == "--decode-only") {
            // Test hook: run the ffmpeg decode path and exit without
            // opening the bridge socket. Non-zero exit on decode failure.
            o.decode_only = true;
        } else if (a == "--vulkan-probe") {
            // Test hook: build one VkProducer slot, print its layout,
            // exit. Non-zero on failure. No IPC, no decode.
            o.vulkan_probe = true;
        } else if (a == "--produce-once") {
            // Test hook: decode --image, upload into one VkProducer slot,
            // export a sync_fd, close fds, exit. No IPC.
            o.produce_once = true;
        } else {
            // Swallow unknown --key value pairs forwarded by the daemon from
            // source-plugin metadata (e.g. --fps, --workshop_id for animated
            // formats we don't implement yet).
            if (!a.empty() && a.rfind("--", 0) == 0 && i + 1 < argc
                && std::string(argv[i + 1]).rfind("--", 0) != 0) {
                ++i;
            }
        }
    }
    return o;
}


// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

struct HostState {
    int                   sock { -1 };
    std::mutex            send_mu;
    std::atomic<bool>     shutdown { false };
    std::mutex            wake_mu;
    std::condition_variable wake_cv;
};

void signal_shutdown(HostState& s) {
    s.shutdown.store(true, std::memory_order_release);
    s.wake_cv.notify_all();
}

void apply_control(HostState& s, const ww_bridge_control_t& c) {
    switch (c.op) {
    case WW_REQ_HELLO:
        break;
    case WW_REQ_LOAD_SCENE:
        // TODO(M4): re-decode and re-upload when the daemon hot-swaps the
        // image. Today we log and keep the initial image.
        std::fprintf(stderr,
                     "waywallen-image-renderer: load_scene pkg=%s "
                     "(hot-swap not yet implemented)\n",
                     c.u.load_scene.pkg ? c.u.load_scene.pkg : "(null)");
        break;
    case WW_REQ_PLAY:
    case WW_REQ_PAUSE:
        // Static images: play/pause are no-ops. Animated formats land in M5.
        break;
    case WW_REQ_MOUSE:
    case WW_REQ_SET_FPS:
        // Images don't respond to input and pace themselves (zero fps).
        break;
    case WW_REQ_SHUTDOWN:
        signal_shutdown(s);
        break;
    default:
        std::fprintf(stderr,
                     "waywallen-image-renderer: unknown control op %d\n",
                     static_cast<int>(c.op));
        break;
    }
}

void reader_loop(HostState& s) {
    while (!s.shutdown.load(std::memory_order_acquire)) {
        ww_bridge_control_t msg {};
        int                 rc = ww_bridge_recv_control(s.sock, &msg);
        if (rc != 0) {
            if (!s.shutdown.load(std::memory_order_acquire)) {
                std::fprintf(stderr,
                             "waywallen-image-renderer: recv_control failed: %d\n",
                             rc);
            }
            signal_shutdown(s);
            return;
        }
        apply_control(s, msg);
        ww_bridge_control_free(&msg);
    }
}

} // namespace


// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

int main(int argc, char** argv) {
    Options opt = parse_args(argc, argv);

    if (opt.vulkan_probe) {
        std::string verr;
        auto prod = ww_image::VkProducer::create(opt.width, opt.height, &verr);
        if (!prod) {
            std::fprintf(stderr, "waywallen-image-renderer: vk_producer: %s\n",
                         verr.c_str());
            return 1;
        }
        const auto& L = prod->layout();
        std::fprintf(stderr,
                     "waywallen-image-renderer: vk slot "
                     "fd=%d fourcc=0x%08x mod=0x%llx "
                     "%ux%u offset=%u stride=%u size=%u\n",
                     L.dmabuf_fd, L.drm_fourcc,
                     static_cast<unsigned long long>(L.drm_modifier),
                     L.width, L.height, L.plane_offset, L.stride, L.size);
        if (L.dmabuf_fd < 0)       { std::fprintf(stderr, "FAIL: bad fd\n");   return 1; }
        if (L.stride < L.width*4)  { std::fprintf(stderr, "FAIL: stride\n");   return 1; }
        if (L.size < L.stride*L.height) { std::fprintf(stderr, "FAIL: size\n"); return 1; }
        return 0;
    }

    if (opt.decode_only) {
        if (opt.image_path.empty()) die("--decode-only requires --image");
        ww_image::DecodeError derr;
        ww_image::RgbaBuf buf =
            ww_image::decode_to_rgba(opt.image_path, opt.width, opt.height, &derr);
        if (buf.data.empty()) {
            std::fprintf(stderr,
                         "waywallen-image-renderer: decode failed: %s\n",
                         derr.message.c_str());
            return 1;
        }
        uint64_t sum = 0;
        for (uint8_t b : buf.data) sum += b;
        std::fprintf(stderr,
                     "waywallen-image-renderer: decoded %ux%u stride=%u "
                     "bytes=%zu pixel_sum=%llu\n",
                     buf.width, buf.height, buf.stride,
                     buf.data.size(),
                     static_cast<unsigned long long>(sum));
        return 0;
    }

    if (opt.produce_once) {
        if (opt.image_path.empty()) die("--produce-once requires --image");
        ww_image::DecodeError derr;
        ww_image::RgbaBuf buf =
            ww_image::decode_to_rgba(opt.image_path, opt.width, opt.height, &derr);
        if (buf.data.empty()) {
            std::fprintf(stderr,
                         "waywallen-image-renderer: decode failed: %s\n",
                         derr.message.c_str());
            return 1;
        }
        std::string verr;
        auto prod = ww_image::VkProducer::create(opt.width, opt.height, &verr);
        if (!prod) {
            std::fprintf(stderr,
                         "waywallen-image-renderer: vk_producer: %s\n",
                         verr.c_str());
            return 1;
        }
        std::string uerr;
        int sync_fd = prod->upload_and_submit(
            buf.data.data(), buf.data.size(), &uerr);
        if (sync_fd < 0) {
            std::fprintf(stderr,
                         "waywallen-image-renderer: upload: %s\n",
                         uerr.c_str());
            return 1;
        }
        const auto& L = prod->layout();
        std::fprintf(stderr,
                     "waywallen-image-renderer: produced "
                     "dmabuf_fd=%d mod=0x%llx stride=%u size=%u sync_fd=%d\n",
                     L.dmabuf_fd,
                     static_cast<unsigned long long>(L.drm_modifier),
                     L.stride, L.size, sync_fd);
        ::close(sync_fd);
        return 0;
    }

    if (opt.ipc_path.empty()) die("--ipc <socket_path> is required");

    ::prctl(PR_SET_PDEATHSIG, SIGTERM);

    HostState host;
    host.sock = ww_bridge_connect(opt.ipc_path.c_str());
    if (host.sock < 0)
        die("ww_bridge_connect: " + std::string(std::strerror(-host.sock)));

    if (int rc = ww_bridge_send_ready(host.sock); rc != 0)
        die("send ready failed: " + std::to_string(rc));

    std::fprintf(stderr,
                 "waywallen-image-renderer: ready image=%s %ux%u\n",
                 opt.image_path.empty() ? "(none)" : opt.image_path.c_str(),
                 opt.width, opt.height);

    std::unique_ptr<ww_image::VkProducer> producer;
    if (!opt.image_path.empty()) {
        ww_image::DecodeError derr;
        ww_image::RgbaBuf buf =
            ww_image::decode_to_rgba(opt.image_path, opt.width, opt.height, &derr);
        if (buf.data.empty()) {
            die("decode " + opt.image_path + ": " + derr.message);
        }

        std::string verr;
        producer = ww_image::VkProducer::create(opt.width, opt.height, &verr);
        if (!producer) die("vk_producer: " + verr);

        std::string uerr;
        int sync_fd = producer->upload_and_submit(
            buf.data.data(), buf.data.size(), &uerr);
        if (sync_fd < 0) die("upload: " + uerr);

        const auto& L = producer->layout();

        // One slot, one DMA-BUF fd. `sizes` is a parallel array for the
        // bridge; count must match `bb.count`.
        uint64_t sizes[1] = { L.size };
        int      fds[1]   = { L.dmabuf_fd };

        ww_evt_bind_buffers_t bb {};
        bb.count        = 1;
        bb.fourcc       = L.drm_fourcc;
        bb.width        = L.width;
        bb.height       = L.height;
        bb.stride       = L.stride;
        bb.modifier     = L.drm_modifier;
        bb.plane_offset = L.plane_offset;
        bb.sizes.count  = 1;
        bb.sizes.data   = sizes;

        {
            std::lock_guard<std::mutex> lock(host.send_mu);
            if (int rc = ww_bridge_send_bind_buffers(host.sock, &bb, fds);
                rc != 0) {
                ::close(sync_fd);
                die("send bind_buffers failed: " + std::to_string(rc));
            }
        }

        ww_evt_frame_ready_t fr {};
        fr.image_index = 0;
        fr.seq         = 0;
        fr.ts_ns       = now_ns();

        int rc;
        {
            std::lock_guard<std::mutex> lock(host.send_mu);
            rc = ww_bridge_send_frame_ready(host.sock, &fr, sync_fd);
        }
        // SCM_RIGHTS dup'd the fd on success; close our copy either way.
        ::close(sync_fd);
        if (rc != 0) die("send frame_ready failed: " + std::to_string(rc));

        std::fprintf(stderr,
                     "waywallen-image-renderer: sent bind_buffers + "
                     "frame_ready (%ux%u, stride=%u, size=%u)\n",
                     L.width, L.height, L.stride, L.size);
    }

    std::thread reader([&]() { reader_loop(host); });

    // M0: we don't produce frames yet. Block until shutdown; the reader
    // thread wakes us via signal_shutdown().
    {
        std::unique_lock<std::mutex> lk(host.wake_mu);
        host.wake_cv.wait(lk, [&] {
            return host.shutdown.load(std::memory_order_acquire);
        });
    }

    if (reader.joinable()) {
        ::shutdown(host.sock, SHUT_RD);
        reader.join();
    }
    ww_bridge_close(host.sock);
    return 0;
}
