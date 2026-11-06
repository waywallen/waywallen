export module waywallen.plugin.run_loop;

import rstd;
import waywallen.bridge;

using namespace rstd::prelude;
using rstd::sync::Arc;
using rstd::thread::JoinHandle;
using Instant = rstd::time::Instant;

export namespace waywallen::plugin
{
enum class ExitCause
{
    Requested,
    Shutdown,
    Transport,
    Failure,
};

struct LoopExit {
    ExitCause cause;
    int       code {};
};

struct LoopStep {
    enum class Kind
    {
        Again,
        Wait
    };
    Kind            kind;
    Option<Instant> deadline;

    static auto again() -> LoopStep { return { Kind::Again, None() }; }
    static auto wait(Option<Instant> deadline = None()) -> LoopStep {
        return { Kind::Wait, deadline };
    }
    static auto wait_until(Instant deadline) -> LoopStep { return wait(Some(deadline)); }
};

struct ControlEvent {
    ww_bridge_control_t value {};

    ControlEvent()                                       = default;
    ControlEvent(const ControlEvent&)                    = delete;
    auto operator=(const ControlEvent&) -> ControlEvent& = delete;
    ControlEvent(ControlEvent&& other) noexcept;
    auto operator=(ControlEvent&& other) noexcept -> ControlEvent&;
    ~ControlEvent();
};

struct RunLoopState;

class RunLoop {
    Arc<RunLoopState>        state_;
    Option<JoinHandle<void>> reader_;
    Option<LoopExit>         exit_;
    int                      socket_;

    RunLoop(Arc<RunLoopState> state, JoinHandle<void> reader, int socket);

public:
    using Next = Result<Option<ControlEvent>, int>;

private:
    template<typename Control>
    auto dispatch(Next next, Control& on_control) -> bool {
        if (! running()) return false;
        if (next.is_err()) {
            request_stop(ExitCause::Transport, next.unwrap_err());
            return false;
        }
        auto event = next.unwrap();
        if (! event) return false;
        if (event->value.op == WW_EVT_IN_SHUTDOWN)
            request_stop(ExitCause::Shutdown);
        else
            on_control(event->value);
        return true;
    }

public:
    RunLoop(const RunLoop&)                    = delete;
    auto operator=(const RunLoop&) -> RunLoop& = delete;
    RunLoop(RunLoop&& other) noexcept;
    auto operator=(RunLoop&& other) noexcept -> RunLoop&;
    ~RunLoop();

    // The socket remains borrowed until close() has joined the reader.
    static auto start(int socket) -> rstd::io::Result<RunLoop>;
    auto        try_recv() -> Next;
    // None is a deadline expiry; transport closure is an error after queued events.
    auto wait(Option<Instant> deadline = None()) -> Next;
    auto pending() const -> bool;

    // Dispatch, work callbacks and stop state are owned by the calling thread.
    void request_stop(ExitCause cause = ExitCause::Requested, int code = 0) {
        if (! exit_) exit_ = Some(LoopExit { cause, code });
    }
    auto running() const -> bool { return ! exit_; }

    template<typename Control>
    void drain(Control&& on_control) {
        for (unsigned count = 0; count < 64 && running(); ++count)
            if (! dispatch(try_recv(), on_control)) break;
    }

    template<typename Control>
    void wait_until(Instant deadline, Control&& on_control) {
        while (running() && Instant::now() < deadline)
            if (! dispatch(wait(Some(deadline)), on_control)) break;
    }

    template<typename Control, typename Work>
    auto run(Control&& on_control, Work&& work) -> LoopExit {
        while (running()) {
            drain(on_control);
            if (! running()) break;
            auto next = work();
            if (! running()) break;
            if (next.kind == LoopStep::Kind::Wait) (void)dispatch(wait(next.deadline), on_control);
        }
        return *exit_;
    }

    void close();
};
} // namespace waywallen::plugin
