#include <waywallen-bridge/bridge.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>

import rstd;
import waywallen.plugin.run_loop;

namespace
{
using namespace rstd::prelude;
using namespace rstd::literals;
using rstd::os::fd::OwnedFd;
using rstd::time::Duration;
using rstd::time::Instant;
using waywallen::plugin::RunLoop;

#define CHECK(condition)                                                                 \
    do {                                                                                 \
        if (! (condition)) {                                                             \
            rstd::io::eprintln("run loop check failed at line {}: {}",                   \
                               __LINE__,                                                 \
                               rstd::ffi::CStr::from_ptr(#condition).to_str().unwrap()); \
            return 1;                                                                    \
        }                                                                                \
    } while (false)

struct Sockets {
    OwnedFd peer, renderer;
};

auto sockets() -> Sockets {
    int descriptors[2];
    if (::socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, descriptors) != 0)
        rstd::panic { "socketpair failed" };
    return { OwnedFd::from_raw_fd(descriptors[0]), OwnedFd::from_raw_fd(descriptors[1]) };
}

struct Encoded {
    ww_buf_t value {};
    Encoded() { ww_buf_init(&value); }
    ~Encoded() { ww_buf_free(&value); }
    Encoded(const Encoded&) = delete;
};

auto deadline(unsigned milliseconds = 1000) -> Option<Instant> {
    return Some(Instant::now() + Duration::from_millis(u64(milliseconds)));
}

auto text(const char* value) -> ref<str> {
    return rstd::ffi::CStr::from_ptr(value).to_str().unwrap();
}

int pause(int fd, unsigned fade) {
    const ww_evt_in_pause_t message { { fade } };
    Encoded                 encoded;
    int                     rc = ww_evt_in_pause_encode(&message, &encoded.value);
    return rc ? rc
              : ww_bridge_send_frame(
                    fd, WW_EVT_IN_PAUSE, encoded.value.data, encoded.value.len, nullptr, 0);
}

int settings(int fd, const char* value) {
    char                              key[] = "resolution";
    ww_kv_t                           item { key, const_cast<char*>(value) };
    const ww_evt_in_setting_changed_t message { { 1, &item } };
    Encoded                           encoded;
    int rc = ww_evt_in_setting_changed_encode(&message, &encoded.value);
    return rc ? rc
              : ww_bridge_send_frame(fd,
                                     WW_EVT_IN_SETTING_CHANGED,
                                     encoded.value.data,
                                     encoded.value.len,
                                     nullptr,
                                     0);
}

bool send_bytes(int fd, const void* data, size_t size) {
    auto bytes = static_cast<const unsigned char*>(data);
    while (size) {
        auto sent = ::send(fd, bytes, size, MSG_NOSIGNAL);
        if (sent <= 0) return false;
        bytes += sent;
        size -= size_t(sent);
    }
    return true;
}

int timeout_then_receive() {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    CHECK(! loop.pending());
    auto timed = loop.wait(deadline(5));
    CHECK(timed.is_ok() && timed.unwrap().is_none());
    CHECK(pause(pair.peer.as_raw_fd(), 42) == 0);
    auto received = loop.wait(deadline());
    CHECK(received.is_ok());
    auto event = received.unwrap();
    CHECK(event.is_some());
    CHECK(event->value.op == WW_EVT_IN_PAUSE);
    CHECK(event->value.u.pause.transition.fade_ms == 42);
    auto empty = loop.try_recv();
    CHECK(empty.is_ok() && empty.unwrap().is_none());
    return 0;
}

int payload_move() {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    CHECK(settings(pair.peer.as_raw_fd(), "-2") == 0);
    CHECK(settings(pair.peer.as_raw_fd(), "1080") == 0);
    auto first_result  = loop.wait(deadline());
    auto second_result = loop.wait(deadline());
    CHECK(first_result.is_ok() && second_result.is_ok());
    auto first  = first_result.unwrap();
    auto second = second_result.unwrap();
    CHECK(first.is_some() && second.is_some());
    CHECK(first->value.op == WW_EVT_IN_SETTING_CHANGED);
    CHECK(second->value.op == WW_EVT_IN_SETTING_CHANGED);
    CHECK(text(second->value.u.setting_changed.settings.data[0].value) == "1080"_str);
    *second = rstd::move(*first);
    first   = None();
    CHECK(second->value.u.setting_changed.settings.count == 1);
    CHECK(text(second->value.u.setting_changed.settings.data[0].key) == "resolution"_str);
    CHECK(text(second->value.u.setting_changed.settings.data[0].value) == "-2"_str);
    auto moved = rstd::move(second).unwrap();
    CHECK(text(moved.value.u.setting_changed.settings.data[0].value) == "-2"_str);
    loop.close();
    CHECK(text(moved.value.u.setting_changed.settings.data[0].value) == "-2"_str);
    return 0;
}

int partial_frame() {
    auto                              pair  = sockets();
    auto                              loop  = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    char                              key[] = "fps", value[] = "60";
    ww_kv_t                           item { key, value };
    const ww_evt_in_setting_changed_t message { { 1, &item } };
    Encoded                           encoded;
    CHECK(ww_evt_in_setting_changed_encode(&message, &encoded.value) == 0);
    auto          total = encoded.value.len + 4;
    unsigned char header[] { static_cast<unsigned char>(WW_EVT_IN_SETTING_CHANGED & 255),
                             static_cast<unsigned char>(WW_EVT_IN_SETTING_CHANGED >> 8),
                             static_cast<unsigned char>(total & 255),
                             static_cast<unsigned char>(total >> 8) };
    CHECK(send_bytes(pair.peer.as_raw_fd(), header, 1));
    auto first = loop.wait(deadline(5));
    CHECK(first.is_ok() && first.unwrap().is_none());
    CHECK(send_bytes(pair.peer.as_raw_fd(), header + 1, 3));
    auto second = loop.wait(deadline(5));
    CHECK(second.is_ok() && second.unwrap().is_none());
    CHECK(encoded.value.len > 1);
    CHECK(send_bytes(pair.peer.as_raw_fd(), encoded.value.data, encoded.value.len - 1));
    auto third = loop.wait(deadline(5));
    CHECK(third.is_ok() && third.unwrap().is_none());
    CHECK(send_bytes(pair.peer.as_raw_fd(), encoded.value.data + encoded.value.len - 1, 1));
    auto complete = loop.wait(deadline());
    CHECK(complete.is_ok());
    auto event = complete.unwrap();
    CHECK(event.is_some() && event->value.op == WW_EVT_IN_SETTING_CHANGED);
    CHECK(event->value.u.setting_changed.settings.count == 1);
    CHECK(text(event->value.u.setting_changed.settings.data[0].value) == "60"_str);
    return 0;
}

int bounded_fifo() {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    for (unsigned i = 0; i < 256; ++i) CHECK(pause(pair.peer.as_raw_fd(), i) == 0);
    rstd::thread::sleep(Duration::from_millis(u64(10)));
    CHECK(loop.pending());
    for (unsigned i = 0; i < 256; ++i) {
        auto result = loop.wait(deadline());
        CHECK(result.is_ok());
        auto event = result.unwrap();
        CHECK(event.is_some() && event->value.op == WW_EVT_IN_PAUSE);
        CHECK(event->value.u.pause.transition.fade_ms == i);
    }
    auto empty = loop.try_recv();
    CHECK(empty.is_ok() && empty.unwrap().is_none());
    return 0;
}

int close_full_queue() {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    for (unsigned i = 0; i < 256; ++i) CHECK(settings(pair.peer.as_raw_fd(), "-2") == 0);
    rstd::thread::sleep(Duration::from_millis(u64(10)));
    CHECK(loop.pending());
    auto started = Instant::now();
    loop.close();
    CHECK(started.elapsed() < Duration::from_secs(u64(2)));
    loop.close();
    CHECK(::fcntl(pair.renderer.as_raw_fd(), F_GETFD) >= 0);
    return 0;
}

int close_blocked_reader() {
    auto pair    = sockets();
    auto loop    = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    auto started = Instant::now();
    loop.close();
    CHECK(started.elapsed() < Duration::from_secs(u64(2)));
    loop.close();
    CHECK(::fcntl(pair.renderer.as_raw_fd(), F_GETFD) >= 0);
    CHECK(loop.try_recv().is_err());
    return 0;
}

int close_partial_reader() {
    auto                pair   = sockets();
    auto                loop   = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    const unsigned char header = WW_EVT_IN_PAUSE;
    CHECK(send_bytes(pair.peer.as_raw_fd(), &header, 1));
    auto timed = loop.wait(deadline(5));
    CHECK(timed.is_ok() && timed.unwrap().is_none());
    auto started = Instant::now();
    loop.close();
    CHECK(started.elapsed() < Duration::from_secs(u64(2)));
    return 0;
}

int drain_before_eof() {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    CHECK(pause(pair.peer.as_raw_fd(), 1) == 0);
    CHECK(pause(pair.peer.as_raw_fd(), 2) == 0);
    CHECK(::shutdown(pair.peer.as_raw_fd(), SHUT_WR) == 0);
    for (unsigned i = 1; i <= 2; ++i) {
        auto result = loop.wait(deadline());
        CHECK(result.is_ok());
        auto event = result.unwrap();
        CHECK(event.is_some() && event->value.op == WW_EVT_IN_PAUSE);
        CHECK(event->value.u.pause.transition.fade_ms == i);
    }
    CHECK(loop.wait(deadline()).is_err());
    CHECK(loop.pending());
    return 0;
}

int move_loop() {
    auto pair  = sockets();
    auto first = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    auto moved = rstd::move(first);
    first.close();
    CHECK(pause(pair.peer.as_raw_fd(), 7) == 0);
    auto result = moved.wait(deadline());
    CHECK(result.is_ok());
    auto event = result.unwrap();
    CHECK(event.is_some() && event->value.op == WW_EVT_IN_PAUSE);
    CHECK(event->value.u.pause.transition.fade_ms == 7);
    return 0;
}
} // namespace

int main() {
    ::alarm(15);
    int (*tests[])() { timeout_then_receive, payload_move,     partial_frame,
                       bounded_fifo,         close_full_queue, close_blocked_reader,
                       close_partial_reader, drain_before_eof, move_loop };
    for (auto test : tests)
        if (int result = test()) return result;
    rstd::io::println("run loop checks=9");
    return 0;
}
