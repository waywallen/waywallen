module waywallen.switcher;
import :client;
import dbusto;
import liwebsocketo;

using namespace rstd::prelude;
using namespace rstd::literals;
using rstd::async::coro;
using rstd::bytes::Bytes;
using rstd::sync::Arc;
namespace async = rstd::async;

namespace waywallen::switch_client
{
constexpr auto  bus_name      = "org.waywallen.waywallen.Daemon"_str;
constexpr auto  bus_path      = "/org/waywallen/waywallen/Daemon"_str;
constexpr auto  bus_interface = "org.waywallen.waywallen.Daemon1"_str;
constexpr usize message_limit { 16 * 1024 * 1024 };

auto failure(ref<str> message, ErrorKind kind = ErrorKind::Connection) -> Failure {
    return { kind, String::make(message) };
}
auto bus_failure(const dbusto::Error& error) -> Failure {
    auto kind = error.is_TimedOut() ? ErrorKind::TimedOut : ErrorKind::Connection;
    if (error.is_Remote()) {
        auto name = error.as_Remote().name.as_str();
        if (name == "org.freedesktop.DBus.Error.NoReply"_str ||
            name == "org.freedesktop.DBus.Error.Timeout"_str)
            kind = ErrorKind::TimedOut;
        else if (name == "org.freedesktop.DBus.Error.Failed"_str)
            kind = ErrorKind::Rejected;
    }
    return { kind, rstd::format("DBus: {}", error) };
}
auto bus_call(ref<str> member, dbusto::Body body = {}) -> dbusto::Message {
    auto message = dbusto::method_call(bus_name, bus_path, bus_interface, member, rstd::move(body));
    message.flags = u8(2); // Do not activate a daemon as a side effect of discovery.
    return message;
}
auto open_bus(Duration timeout) -> coro<Result<dbusto::Connected>> {
    if (timeout.is_zero())
        co_return Err(failure("timeout must be positive"_str, ErrorKind::InvalidInput));
    auto address = rstd::env::var("DBUS_SESSION_BUS_ADDRESS"_str);
    if (address.is_err() || address->is_empty())
        co_return Err(
            failure("DBUS_SESSION_BUS_ADDRESS is not set; is a session bus running?"_str));
    dbusto::ConnectionOptions options;
    options.setup_timeout = timeout;
    options.call_timeout  = timeout;
    auto opened           = co_await dbusto::connect(address->as_str(), options);
    if (opened.is_err()) co_return Err(bus_failure(opened.unwrap_err()));
    co_return Ok(rstd::move(opened).unwrap());
}
auto discover(dbusto::Connection& connection) -> coro<Result<u16>> {
    auto call      = bus_call("GetAll"_str, dbusto::pack(String::make(bus_interface)));
    call.interface = String::make("org.freedesktop.DBus.Properties"_str);
    auto reply     = co_await connection.call(rstd::move(call));
    if (reply.is_err()) co_return Err(bus_failure(reply.unwrap_err()));
    if (dbusto::body_signature(reply->body).as_str() != "a{sv}"_str)
        co_return Err(failure("invalid daemon DBus properties"_str));
    Option<u16> port;
    bool        supported = false;
    for (const auto& entry : reply->body[usize()].as_Array().values) {
        const auto& fields = entry.as_Entry().values;
        const auto& key    = fields[usize()].as_Text().value;
        const auto& value  = *fields[usize(1)].as_Variant().value;
        if (key.as_str() == "WsPort"_str && value.is_UInt16())
            port = Some(u16(value.as_UInt16().value));
        if (key.as_str() == "Capabilities"_str && value.is_Array()) {
            for (const auto& capability : value.as_Array().values)
                if (capability.is_Text() && capability.as_Text().value.as_str() == "control.v1"_str)
                    supported = true;
        }
    }
    if (! supported) co_return Err(failure("daemon does not advertise control.v1"_str));
    if (port.is_none() || *port == u16())
        co_return Err(failure("daemon has no ready WebSocket port"_str));
    co_return Ok(*port);
}
auto daemon_port(Duration timeout) -> coro<Result<u16>> {
    auto opened = co_await open_bus(timeout);
    if (opened.is_err()) co_return Err(rstd::move(opened).unwrap_err());
    auto connected = rstd::move(opened).unwrap();
    auto driver    = async::spawn(rstd::move(connected.driver).run());
    auto result    = co_await discover(connected.connection);
    (void)co_await connected.connection.close();
    (void)co_await rstd::move(driver);
    co_return rstd::move(result);
}
auto step(bool previous, Duration timeout) -> coro<Result<empty>> {
    auto opened = co_await open_bus(timeout);
    if (opened.is_err()) co_return Err(rstd::move(opened).unwrap_err());
    auto          connected  = rstd::move(opened).unwrap();
    auto          driver     = async::spawn(rstd::move(connected.driver).run());
    auto          compatible = co_await discover(connected.connection);
    Result<empty> result     = Ok(empty {});
    if (compatible.is_err())
        result = Err(rstd::move(compatible).unwrap_err());
    else {
        auto reply =
            co_await connected.connection.call(bus_call(previous ? "Previous"_str : "Next"_str));
        if (reply.is_err()) result = Err(bus_failure(reply.unwrap_err()));
    }
    (void)co_await connected.connection.close();
    (void)co_await rstd::move(driver);
    co_return rstd::move(result);
}

struct Transport {
    Arc<rstd::net::TcpStream> socket;
    Bytes                     buffered;
    auto                      read() -> coro<rstd::io::Result<Bytes>> {
        if (! buffered.is_empty()) {
            auto result = rstd::move(buffered);
            buffered    = Bytes {};
            co_return Ok(rstd::move(result));
        }
        co_return co_await socket->recv();
    }
    auto write(Bytes bytes) -> coro<rstd::io::Result<usize>> {
        co_return co_await socket->send(rstd::move(bytes));
    }
    void cancel() { (void)socket->shutdown_both(); }
};
using WebSocket = liwebsocketo::WebSocketConnection<Transport>;

auto upgrade(Arc<rstd::net::TcpStream> socket, u16 port) -> coro<Result<Bytes>> {
    array<u8, 16> nonce;
    if (rstd::random::fill_secure(nonce.as_mut_slice()).is_err())
        co_return Err(failure("cannot generate WebSocket nonce"_str));
    auto handshake = liwebsocketo::ClientHandshake::make(
        "/"_str, rstd::format("127.0.0.1:{}", port).as_str(), nonce);
    if (handshake.is_err()) co_return Err(failure("cannot create WebSocket handshake"_str));
    auto sent = co_await socket->send_all(handshake->request().bytes().clone());
    if (sent.is_err())
        co_return Err(failure(rstd::format("WebSocket handshake: {}", sent.unwrap_err()).as_str()));
    lihttpto::Http1HeadParser parser(true, usize(65536));
    for (;;) {
        auto bytes = co_await socket->recv();
        if (bytes.is_err() || bytes->is_empty())
            co_return Err(failure("connection closed during WebSocket handshake"_str));
        auto parsed = parser.push(bytes->as_slice());
        if (parsed.is_err()) co_return Err(failure("invalid HTTP upgrade response"_str));
        if (parsed->is_NeedMore()) continue;
        auto complete = rstd::move(parsed->as_Complete());
        auto head     = rstd::move(complete.head).into_response();
        if (head.is_err() || handshake->validate(*head).is_err())
            co_return Err(failure("daemon rejected WebSocket upgrade"_str));
        co_return Ok(bytes->slice(complete.input_consumed, bytes->len()));
    }
}

struct State {
    Arc<rstd::net::TcpStream> socket;
    WebSocket                 websocket;
    Duration                  timeout;
    u64                       next_id { 1 };
    Option<pb::M_StatusSync>  status;
    bool                      closed {};
    State(Arc<rstd::net::TcpStream> socket, WebSocket websocket, Duration timeout)
        : socket(rstd::move(socket)), websocket(rstd::move(websocket)), timeout(timeout) {}
};
Client::Client(Arc<State> state): state_(rstd::move(state)) {}
Client::Client(Client&&) noexcept                    = default;
auto Client::operator=(Client&&) noexcept -> Client& = default;
Client::~Client()                                    = default;
auto Client::connect(Options options) -> coro<Result<Client>> {
    if (options.timeout.is_zero() || options.port == Some(u16()))
        co_return Err(failure("invalid connection options"_str, ErrorKind::InvalidInput));
    auto port = options.port;
    if (port.is_none()) {
        auto discovered = co_await daemon_port(options.timeout);
        if (discovered.is_err()) co_return Err(rstd::move(discovered).unwrap_err());
        port = Some(*discovered);
    }
    auto address =
        rstd::net::SocketAddr::parse(rstd::format("127.0.0.1:{}", *port).as_str()).unwrap();
    auto prepared = rstd::net::TcpStream::prepare_connect(address);
    if (prepared.is_err()) co_return Err(failure("cannot prepare daemon connection"_str));
    auto pending   = rstd::move(prepared).unwrap();
    auto connected = co_await async::timeout(async::into_future(pending.finish()), options.timeout);
    if (connected.is_err() || connected->is_err()) {
        (void)co_await rstd::move(pending).close();
        co_return Err(failure(connected.is_err() ? "daemon connection timed out"_str
                                                 : "cannot connect to daemon"_str,
                              connected.is_err() ? ErrorKind::TimedOut : ErrorKind::Connection));
    }
    auto socket   = Arc<rstd::net::TcpStream>::make(rstd::move(pending).into_stream().unwrap());
    auto upgraded = co_await async::timeout(async::into_future(upgrade(socket.clone(), *port)),
                                            options.timeout);
    if (upgraded.is_err() || upgraded->is_err()) {
        (void)socket->shutdown_both();
        (void)co_await rstd::move(socket).try_unwrap().unwrap().close();
        if (upgraded.is_err())
            co_return Err(failure("WebSocket handshake timed out"_str, ErrorKind::TimedOut));
        co_return Err(rstd::move(*upgraded).unwrap_err());
    }
    liwebsocketo::WebSocketOptions limits;
    limits.limits.frame_bytes   = message_limit;
    limits.limits.message_bytes = message_limit;
    limits.receive_bytes        = message_limit;
    auto websocket              = WebSocket::start(
        { socket.clone(), rstd::move(*upgraded).unwrap() }, liwebsocketo::Role::Client, limits);
    if (websocket.is_err()) {
        (void)socket->shutdown_both();
        (void)co_await rstd::move(socket).try_unwrap().unwrap().close();
        co_return Err(failure("cannot start WebSocket driver"_str));
    }
    co_return Ok(Client(
        Arc<State>::make(rstd::move(socket), rstd::move(websocket).unwrap(), options.timeout)));
}

auto exchange(Arc<State> state, pb::M_Request request) -> coro<Result<pb::M_Response>> {
    request.f_request_id  = state->next_id++;
    auto expected_id      = request.f_request_id;
    auto expected_payload = request.o_payload->tag();
    auto encoded          = liproto::encode_to_vec(request, message_limit);
    if (encoded.is_err()) co_return Err(failure("cannot encode control request"_str));
    auto sent = co_await state->websocket.send(
        { liwebsocketo::Kind::Binary, Bytes::copy_from_slice(encoded->as_slice()) });
    if (sent.is_err() || sent->is_err()) co_return Err(failure("WebSocket send failed"_str));
    for (;;) {
        auto received = co_await state->websocket.receive();
        if (received.is_err() || received->is_err())
            co_return Err(failure("daemon disconnected before replying"_str));
        auto message = rstd::move(*received).unwrap();
        if (message.kind != liwebsocketo::Kind::Binary)
            co_return Err(failure("expected a binary control frame"_str));
        auto decoded = liproto::decode<pb::M_ServerFrame>(message.data.as_slice());
        if (decoded.is_err() || decoded->o_kind.is_none())
            co_return Err(failure("invalid control frame"_str));
        auto& frame = *decoded->o_kind;
        if (frame.is_event()) {
            auto& event = *frame.as_event();
            if (event.o_payload.is_some() && event.o_payload->is_status_sync())
                state->status = Some(rstd::move(*event.o_payload->as_status_sync()));
            continue;
        }
        auto response = rstd::move(*frame.as_response());
        if (response.f_request_id != expected_id) continue;
        if (response.f_error_code != i32() || response.f_status != i32(1))
            co_return Err(Failure { ErrorKind::Rejected,
                                    rstd::format("daemon error {} (status {}): {}",
                                                 response.f_error_code,
                                                 response.f_status,
                                                 response.f_message) });
        if (response.o_payload.is_none() || static_cast<unsigned>(response.o_payload->tag()) !=
                                                static_cast<unsigned>(expected_payload))
            co_return Err(failure("unexpected control response payload"_str));
        co_return Ok(rstd::move(response));
    }
}
auto Client::request(pb::M_Request request) -> coro<Result<pb::M_Response>> {
    if (state_->closed || request.o_payload.is_none())
        co_return Err(failure("client is closed or request is empty"_str));
    auto result = co_await async::timeout(
        async::into_future(exchange(state_.clone(), rstd::move(request))), state_->timeout);
    if (result.is_err()) {
        state_->websocket.cancel();
        co_return Err(
            failure("request timed out; a write may already have taken effect (not retried)"_str,
                    ErrorKind::TimedOut));
    }
    co_return rstd::move(*result);
}
auto Client::status() const -> const Option<pb::M_StatusSync>& { return state_->status; }
auto Client::close() -> coro<void> {
    if (state_->closed) co_return;
    state_->closed = true;
    co_await rstd::move(state_->websocket).into_shutdown();
    (void)co_await rstd::move(state_->socket).try_unwrap().unwrap().close();
}
} // namespace waywallen::switch_client
