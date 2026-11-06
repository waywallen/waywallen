#include <waywallen-bridge/bridge.h>
#include <rstd/test/gtest.hpp>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>

import rstd;
import rstd.test;
import waywallen.plugin.run_loop;

namespace
{
using namespace rstd::prelude;
using namespace rstd::literals;
using rstd::os::fd::OwnedFd;
using rstd::time::Duration;
using rstd::time::Instant;
using waywallen::plugin::RunLoop;

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

TEST(RunLoop, TimeoutThenReceive) {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    ASSERT_TRUE(! loop.pending());
    auto timed = loop.wait(deadline(5));
    ASSERT_TRUE(timed.is_ok() && timed.unwrap().is_none());
    ASSERT_TRUE(pause(pair.peer.as_raw_fd(), 42) == 0);
    auto received = loop.wait(deadline());
    ASSERT_TRUE(received.is_ok());
    auto event = received.unwrap();
    ASSERT_TRUE(event.is_some());
    ASSERT_TRUE(event->value.op == WW_EVT_IN_PAUSE);
    ASSERT_TRUE(event->value.u.pause.transition.fade_ms == 42);
    auto empty = loop.try_recv();
    ASSERT_TRUE(empty.is_ok() && empty.unwrap().is_none());
}

TEST(RunLoop, PayloadMove) {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    ASSERT_TRUE(settings(pair.peer.as_raw_fd(), "-2") == 0);
    ASSERT_TRUE(settings(pair.peer.as_raw_fd(), "1080") == 0);
    auto first_result  = loop.wait(deadline());
    auto second_result = loop.wait(deadline());
    ASSERT_TRUE(first_result.is_ok() && second_result.is_ok());
    auto first  = first_result.unwrap();
    auto second = second_result.unwrap();
    ASSERT_TRUE(first.is_some() && second.is_some());
    ASSERT_TRUE(first->value.op == WW_EVT_IN_SETTING_CHANGED);
    ASSERT_TRUE(second->value.op == WW_EVT_IN_SETTING_CHANGED);
    ASSERT_TRUE(text(second->value.u.setting_changed.settings.data[0].value) == "1080"_str);
    *second = rstd::move(*first);
    first   = None();
    ASSERT_TRUE(second->value.u.setting_changed.settings.count == 1);
    ASSERT_TRUE(text(second->value.u.setting_changed.settings.data[0].key) == "resolution"_str);
    ASSERT_TRUE(text(second->value.u.setting_changed.settings.data[0].value) == "-2"_str);
    auto moved = rstd::move(second).unwrap();
    ASSERT_TRUE(text(moved.value.u.setting_changed.settings.data[0].value) == "-2"_str);
    loop.close();
    ASSERT_TRUE(text(moved.value.u.setting_changed.settings.data[0].value) == "-2"_str);
}

TEST(RunLoop, PartialFrame) {
    auto                              pair  = sockets();
    auto                              loop  = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    char                              key[] = "fps", value[] = "60";
    ww_kv_t                           item { key, value };
    const ww_evt_in_setting_changed_t message { { 1, &item } };
    Encoded                           encoded;
    ASSERT_TRUE(ww_evt_in_setting_changed_encode(&message, &encoded.value) == 0);
    auto          total = encoded.value.len + 4;
    unsigned char header[] { static_cast<unsigned char>(WW_EVT_IN_SETTING_CHANGED & 255),
                             static_cast<unsigned char>(WW_EVT_IN_SETTING_CHANGED >> 8),
                             static_cast<unsigned char>(total & 255),
                             static_cast<unsigned char>(total >> 8) };
    ASSERT_TRUE(send_bytes(pair.peer.as_raw_fd(), header, 1));
    auto first = loop.wait(deadline(5));
    ASSERT_TRUE(first.is_ok() && first.unwrap().is_none());
    ASSERT_TRUE(send_bytes(pair.peer.as_raw_fd(), header + 1, 3));
    auto second = loop.wait(deadline(5));
    ASSERT_TRUE(second.is_ok() && second.unwrap().is_none());
    ASSERT_TRUE(encoded.value.len > 1);
    ASSERT_TRUE(send_bytes(pair.peer.as_raw_fd(), encoded.value.data, encoded.value.len - 1));
    auto third = loop.wait(deadline(5));
    ASSERT_TRUE(third.is_ok() && third.unwrap().is_none());
    ASSERT_TRUE(send_bytes(pair.peer.as_raw_fd(), encoded.value.data + encoded.value.len - 1, 1));
    auto complete = loop.wait(deadline());
    ASSERT_TRUE(complete.is_ok());
    auto event = complete.unwrap();
    ASSERT_TRUE(event.is_some() && event->value.op == WW_EVT_IN_SETTING_CHANGED);
    ASSERT_TRUE(event->value.u.setting_changed.settings.count == 1);
    ASSERT_TRUE(text(event->value.u.setting_changed.settings.data[0].value) == "60"_str);
}

TEST(RunLoop, BoundedFifo) {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    for (unsigned i = 0; i < 256; ++i) ASSERT_TRUE(pause(pair.peer.as_raw_fd(), i) == 0);
    rstd::thread::sleep(Duration::from_millis(u64(10)));
    ASSERT_TRUE(loop.pending());
    for (unsigned i = 0; i < 256; ++i) {
        auto result = loop.wait(deadline());
        ASSERT_TRUE(result.is_ok());
        auto event = result.unwrap();
        ASSERT_TRUE(event.is_some() && event->value.op == WW_EVT_IN_PAUSE);
        ASSERT_TRUE(event->value.u.pause.transition.fade_ms == i);
    }
    auto empty = loop.try_recv();
    ASSERT_TRUE(empty.is_ok() && empty.unwrap().is_none());
}

TEST(RunLoop, CloseFullQueue) {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    for (unsigned i = 0; i < 256; ++i) ASSERT_TRUE(settings(pair.peer.as_raw_fd(), "-2") == 0);
    rstd::thread::sleep(Duration::from_millis(u64(10)));
    ASSERT_TRUE(loop.pending());
    auto started = Instant::now();
    loop.close();
    ASSERT_TRUE(started.elapsed() < Duration::from_secs(u64(2)));
    loop.close();
    ASSERT_TRUE(::fcntl(pair.renderer.as_raw_fd(), F_GETFD) >= 0);
}

TEST(RunLoop, CloseBlockedReader) {
    auto pair    = sockets();
    auto loop    = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    auto started = Instant::now();
    loop.close();
    ASSERT_TRUE(started.elapsed() < Duration::from_secs(u64(2)));
    loop.close();
    ASSERT_TRUE(::fcntl(pair.renderer.as_raw_fd(), F_GETFD) >= 0);
    ASSERT_TRUE(loop.try_recv().is_err());
}

TEST(RunLoop, ClosePartialReader) {
    auto                pair   = sockets();
    auto                loop   = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    const unsigned char header = WW_EVT_IN_PAUSE;
    ASSERT_TRUE(send_bytes(pair.peer.as_raw_fd(), &header, 1));
    auto timed = loop.wait(deadline(5));
    ASSERT_TRUE(timed.is_ok() && timed.unwrap().is_none());
    auto started = Instant::now();
    loop.close();
    ASSERT_TRUE(started.elapsed() < Duration::from_secs(u64(2)));
}

TEST(RunLoop, DrainBeforeEof) {
    auto pair = sockets();
    auto loop = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    ASSERT_TRUE(pause(pair.peer.as_raw_fd(), 1) == 0);
    ASSERT_TRUE(pause(pair.peer.as_raw_fd(), 2) == 0);
    ASSERT_TRUE(::shutdown(pair.peer.as_raw_fd(), SHUT_WR) == 0);
    for (unsigned i = 1; i <= 2; ++i) {
        auto result = loop.wait(deadline());
        ASSERT_TRUE(result.is_ok());
        auto event = result.unwrap();
        ASSERT_TRUE(event.is_some() && event->value.op == WW_EVT_IN_PAUSE);
        ASSERT_TRUE(event->value.u.pause.transition.fade_ms == i);
    }
    ASSERT_TRUE(loop.wait(deadline()).is_err());
    ASSERT_TRUE(loop.pending());
}

TEST(RunLoop, MoveLoop) {
    auto pair  = sockets();
    auto first = RunLoop::start(pair.renderer.as_raw_fd()).unwrap();
    auto moved = rstd::move(first);
    first.close();
    ASSERT_TRUE(pause(pair.peer.as_raw_fd(), 7) == 0);
    auto result = moved.wait(deadline());
    ASSERT_TRUE(result.is_ok());
    auto event = result.unwrap();
    ASSERT_TRUE(event.is_some() && event->value.op == WW_EVT_IN_PAUSE);
    ASSERT_TRUE(event->value.u.pause.transition.fade_ms == 7);
}
} // namespace
