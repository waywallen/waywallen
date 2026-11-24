export module waywallen.switcher:client;
export import rstd;
export import proto.p_control;

using namespace rstd::prelude;

export namespace waywallen::switch_client
{
namespace pb = p_waywallen::p_control::p_v1;
using rstd::async::coro;
using rstd::time::Duration;

enum class ErrorKind
{
    InvalidInput,
    Connection,
    Rejected,
    TimedOut
};
struct Failure {
    ErrorKind kind;
    String    message;
};
template<class T>
using Result = rstd::Result<T, Failure>;

struct Options {
    Option<u16> port;
    Duration    timeout { Duration::from_secs(u64(10)) };
};

struct State;
// Use from one task and await close() after the last request, before destroying the runtime.
class Client {
    rstd::sync::Arc<State> state_;
    explicit Client(rstd::sync::Arc<State> state);

public:
    Client(Client&&) noexcept;
    auto operator=(Client&&) noexcept -> Client&;
    ~Client();
    static auto connect(Options options) -> coro<Result<Client>>;
    auto        request(pb::M_Request request) -> coro<Result<pb::M_Response>>;
    auto        status() const -> const Option<pb::M_StatusSync>&;
    auto        close() -> coro<void>;
};

auto step(bool previous, Duration timeout) -> coro<Result<empty>>;
} // namespace waywallen::switch_client
