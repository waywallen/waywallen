module;

#include <errno.h>
#include <sys/socket.h>

module waywallen.plugin.run_loop;

namespace waywallen::plugin
{
using namespace rstd::prelude;
using namespace rstd::literals;

ControlEvent::ControlEvent(ControlEvent&& other) noexcept: value(other.value) { other.value = {}; }
auto ControlEvent::operator=(ControlEvent&& other) noexcept -> ControlEvent& {
    if (this != &other) {
        ww_bridge_control_free(&value);
        value       = other.value;
        other.value = {};
    }
    return *this;
}
ControlEvent::~ControlEvent() { ww_bridge_control_free(&value); }

struct RunLoopFields {
    ::alloc::collections::VecDeque<ControlEvent> controls;
    int                                          error {};
    bool                                         closing {};
};

struct RunLoopState {
    rstd::sync::Mutex<RunLoopFields> fields;
    rstd::sync::Condvar              changed;
};

RunLoop::RunLoop(rstd::sync::Arc<RunLoopState> state, rstd::thread::JoinHandle<void> reader,
                 int socket)
    : state_(rstd::move(state)), reader_(Some(rstd::move(reader))), socket_(socket) {}

RunLoop::RunLoop(RunLoop&& other) noexcept
    : state_(rstd::move(other.state_)),
      reader_(other.reader_.take()),
      socket_(rstd::exchange(other.socket_, -1)) {}

auto RunLoop::operator=(RunLoop&& other) noexcept -> RunLoop& {
    if (this != &other) {
        close();
        state_  = rstd::move(other.state_);
        reader_ = other.reader_.take();
        socket_ = rstd::exchange(other.socket_, -1);
    }
    return *this;
}

RunLoop::~RunLoop() { close(); }

auto RunLoop::start(int socket) -> rstd::io::Result<RunLoop> {
    if (socket < 0)
        return Err(
            rstd::io::Error::from_kind(rstd::io::ErrorKind { rstd::io::ErrorKind::InvalidInput }));
    auto state  = rstd::sync::Arc<RunLoopState>::make();
    auto worker = rstd::thread::builder::Builder::make();
    worker.name(String::make("renderer-control"_str));
    auto started = worker.spawn([state = state.clone(), socket] {
        for (;;) {
            {
                auto fields = state->fields.lock().unwrap();
                state->changed.wait_while(fields, [](const auto& fields) {
                    return ! fields.closing && fields.controls.len() >= usize(64);
                });
                if (fields->closing) return;
            }
            ControlEvent event;
            int          rc = ww_bridge_recv_control(socket, &event.value);
            {
                auto fields = state->fields.lock().unwrap();
                if (fields->closing) return;
                if (rc)
                    fields->error = rc;
                else
                    fields->controls.push_back(rstd::move(event));
            }
            state->changed.notify_all();
            if (rc) return;
        }
    });
    if (started.is_err()) return Err(started.unwrap_err());
    return Ok(RunLoop(rstd::move(state), started.unwrap(), socket));
}

auto RunLoop::try_recv() -> Next {
    if (! state_) return Err(-ENOTCONN);
    auto fields = state_->fields.lock().unwrap();
    if (auto next = fields->controls.pop_front(); next) {
        state_->changed.notify_all();
        return Ok(rstd::move(next));
    }
    if (fields->error) return Err(fields->error);
    if (fields->closing) return Err(-ENOTCONN);
    return Ok(None<ControlEvent>());
}

auto RunLoop::wait(Option<rstd::time::Instant> deadline) -> Next {
    if (! state_) return Err(-ENOTCONN);
    auto fields = state_->fields.lock().unwrap();
    for (;;) {
        if (auto next = fields->controls.pop_front(); next) {
            state_->changed.notify_all();
            return Ok(rstd::move(next));
        }
        if (fields->error) return Err(fields->error);
        if (fields->closing) return Err(-ENOTCONN);
        if (deadline) {
            auto now = rstd::time::Instant::now();
            if (now >= *deadline) return Ok(None<ControlEvent>());
            (void)state_->changed.wait_timeout(fields, *deadline - now);
        } else {
            state_->changed.wait(fields);
        }
    }
}

auto RunLoop::pending() const -> bool {
    if (! state_) return true;
    auto fields = state_->fields.lock().unwrap();
    return fields->closing || fields->error || ! fields->controls.is_empty();
}

void RunLoop::close() {
    if (! reader_) return;
    {
        auto fields     = state_->fields.lock().unwrap();
        fields->closing = true;
        fields->controls.clear();
    }
    state_->changed.notify_all();
    (void)::shutdown(socket_, SHUT_RD);
    (void)rstd::move(reader_.take().unwrap()).join();
}
} // namespace waywallen::plugin
