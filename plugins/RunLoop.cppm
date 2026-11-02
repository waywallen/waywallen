export module waywallen.plugin.run_loop;

import rstd;
import waywallen.bridge;

export namespace waywallen::plugin
{
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
    rstd::sync::Arc<RunLoopState>                state_;
    rstd::Option<rstd::thread::JoinHandle<void>> reader_;
    int                                          socket_;

    RunLoop(rstd::sync::Arc<RunLoopState> state, rstd::thread::JoinHandle<void> reader, int socket);

public:
    using Next = rstd::Result<rstd::Option<ControlEvent>, int>;

    RunLoop(const RunLoop&)                    = delete;
    auto operator=(const RunLoop&) -> RunLoop& = delete;
    RunLoop(RunLoop&& other) noexcept;
    auto operator=(RunLoop&& other) noexcept -> RunLoop&;
    ~RunLoop();

    // The socket remains borrowed until close() has joined the reader.
    static auto start(int socket) -> rstd::io::Result<RunLoop>;
    auto        try_recv() -> Next;
    // None is a deadline expiry; transport closure is an error after queued events.
    auto wait(rstd::Option<rstd::time::Instant> deadline = rstd::None()) -> Next;
    auto pending() const -> bool;
    void close();
};
} // namespace waywallen::plugin
