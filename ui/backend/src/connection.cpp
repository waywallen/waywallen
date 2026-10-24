module;
#include <QNetworkAccessManager>
#include <QNetworkProxy>
#include <QThread>
#include <QtGlobal>
#include <algorithm>
#include <chrono>
#include <stdexcept>

module waywallen.backend.connection;
import ncrequest;
import rstd.cppstd;

using namespace rstd::prelude;
using rstd::bytes::Bytes;
using rstd::sync::Arc;
namespace lf = lifetchto;
namespace qt = ncrequest::client::qt_network;

namespace waywallen
{
namespace
{
template<class Error>
auto describe(const Error& error) -> QString {
    auto text = rstd::format("{}", error);
    return QString::fromUtf8(reinterpret_cast<const char*>(text.data()),
                             static_cast<qsizetype>(text.len().to_primitive()));
}

auto describe(const ncrequest::WebSocketError& error) -> QString {
    auto text   = rstd::format("{}", error);
    auto result = QString::fromUtf8(reinterpret_cast<const char*>(text.data()),
                                    static_cast<qsizetype>(text.len().to_primitive()));
    if (error.cause().is_some()) result += QStringLiteral(": ") + describe(*error.cause());
    return result;
}

auto describe(const ncrequest::WebSocketConnectFailure& error) -> QString {
    if (error.transport.is_some()) return describe(*error.transport);
    if (error.admission.is_some()) return describe(*error.admission);
    if (error.connection.is_some()) return describe(*error.connection);
    if (error.handshake.is_some()) return describe(*error.handshake);
    if (error.upgrade.is_some() && error.upgrade->transport.is_some())
        return describe(*error.upgrade->transport);
    return QStringLiteral("WebSocket handshake failed");
}

struct ConnectionState {
    BackendConnectionCallbacks              callbacks;
    BackendConnectionLimits                 limits;
    lf::Event                               changed;
    Option<lihttpto::Url>                   target;
    Option<Arc<ncrequest::Session>>         session;
    Option<Arc<ncrequest::WebSocketClient>> client;
    Vec<rstd::async::JoinHandle<void>>      sends;
    quint64                                 generation {};
    bool connected {}, closing {}, failed {}, peer_disconnected {};

    void stop() {
        connected = false;
        if (client) (*client)->disconnect();
        if (session) (*session)->request_close();
        changed.notify();
    }

    bool current(quint64 value) const { return generation == value && ! closing; }

    void fail(quint64 value, QString error) {
        if (! current(value) || std::exchange(failed, true)) return;
        stop();
        if (callbacks.error) callbacks.error(std::move(error));
    }
};

auto watch_send(Arc<ConnectionState> state, ncrequest::PendingSend pending, quint64 generation,
                rstd::async::AnyExecutor executor) -> coro<void> {
    auto written = co_await pending.written();
    if (! co_await executor) co_return;
    if (written.is_err()) state->fail(generation, describe(written.unwrap_err()));
}

auto run_connection(Arc<ConnectionState> state, rstd::async::RuntimeHandle runtime,
                    rstd::async::AnyExecutor executor) -> coro<void> {
    if (! co_await executor) co_return;
    lf::EngineOptions limits;
    limits.limits.bytes     = usize(state->limits.engine_bytes);
    limits.limits.resources = usize(state->limits.send_messages) + usize(32);
    auto made_engine        = lf::Engine::make(runtime.clone(), limits);
    if (made_engine.is_err()) {
        state->fail(state->generation, describe(made_engine.unwrap_err()));
        co_return;
    }
    auto engine  = rstd::move(made_engine).unwrap();
    auto manager = rstd::boxed::Box<QNetworkAccessManager>::make();
    manager->setProxy(QNetworkProxy(QNetworkProxy::NoProxy));
    qt::Options options;
    options.manager   = manager.get();
    options.proxy     = ncrequest::ProxyOptions::disabled();
    auto made_backend = qt::Backend::make(engine.resources(), rstd::move(options));
    if (made_backend.is_err()) {
        state->fail(state->generation, describe(made_backend.unwrap_err()));
        (void)co_await engine.close();
        co_await executor;
        co_return;
    }
    auto backend = rstd::move(made_backend).unwrap();
    for (;;) {
        if (! co_await executor) co_return;
        auto requested = state->changed.listen();
        if (state->closing) break;
        if (! state->target) {
            co_await rstd::move(requested);
            continue;
        }
        auto target = std::move(*state->target);
        state->target.take();
        auto generation          = state->generation;
        state->failed            = false;
        state->peer_disconnected = false;
        auto made_session =
            ncrequest::Session::from_backend(engine.handle(), backend.shared_binding());
        if (made_session.is_err()) {
            state->fail(generation, describe(made_session.unwrap_err()));
            continue;
        }
        auto session   = rstd::move(made_session).unwrap();
        state->session = Some(session.clone());
        ncrequest::WebSocketOptions ws;
        ws.limits.frame_bytes   = usize(state->limits.frame_bytes);
        ws.limits.message_bytes = usize(state->limits.message_bytes);
        ws.receive_messages     = usize(state->limits.receive_messages);
        ws.receive_bytes        = usize(state->limits.receive_bytes);
        ws.send_messages        = usize(state->limits.send_messages);
        ws.send_bytes           = usize(state->limits.send_bytes);
        // IPC stays open for the process lifetime; handshake and close retain their deadlines.
        ws.upgrade.duplex.total_timeout = Some(lf::http::TotalTimeout::disabled());
        ncrequest::WebSocketCallbacks callbacks;
        callbacks.executor = Some(executor.clone());
        callbacks.message  = [state = state.clone(), generation](liwebsocketo::Message message) {
            if (! state->current(generation) || state->failed) return;
            if (message.kind == liwebsocketo::Kind::Pong) return;
            if (message.kind != liwebsocketo::Kind::Binary) {
                state->fail(generation, QStringLiteral("Expected a binary backend message"));
                return;
            }
            auto bytes = message.data.as_slice();
            if (state->callbacks.message)
                state->callbacks.message(
                    QByteArray(reinterpret_cast<const char*>(bytes.as_raw_ptr()),
                               static_cast<qsizetype>(bytes.len().to_primitive())));
        };
        callbacks.error = [state = state.clone(), generation](ncrequest::WebSocketError error) {
            state->fail(generation, describe(error));
        };
        callbacks.disconnected = [state = state.clone(), generation] {
            if (! state->current(generation)) return;
            state->connected         = false;
            state->peer_disconnected = true;
            state->changed.notify();
        };
        auto opened = co_await ncrequest::WebSocketClient::connect(
            session.clone(), { rstd::move(target), {} }, rstd::move(callbacks), {}, {}, ws);
        if (! co_await executor) co_return;
        bool was_connected = false;
        if (opened.is_err()) {
            state->fail(generation, describe(opened.unwrap_err()));
        } else {
            auto client   = Arc<ncrequest::WebSocketClient>::make(rstd::move(opened).unwrap());
            state->client = Some(client.clone());
            if (state->current(generation) && ! state->failed && ! state->peer_disconnected &&
                client->is_connected()) {
                was_connected = state->connected = true;
                if (state->callbacks.connected) state->callbacks.connected();
                for (;;) {
                    auto changed = state->changed.listen();
                    if (! state->current(generation) || ! state->connected) break;
                    co_await rstd::move(changed);
                    if (! co_await executor) co_return;
                }
            }
            state->connected = false;
            auto sends       = rstd::move(state->sends);
            state->sends     = Vec<rstd::async::JoinHandle<void>>::make();
            if (state->closing && ! state->failed && client->is_connected())
                (void)co_await client->close();
            auto closed = co_await client->shutdown();
            for (auto& send : sends) (void)co_await rstd::move(send);
            if (! co_await executor) co_return;
            state->client.take();
            if (closed.is_err()) state->fail(generation, describe(closed.unwrap_err()));
        }
        auto closed = co_await session->close();
        if (! co_await executor) co_return;
        state->session.take();
        if (closed.is_err()) state->fail(generation, describe(closed.errors[usize()]));
        if (state->current(generation) && was_connected && state->callbacks.disconnected)
            state->callbacks.disconnected();
    }
    auto closed_backend = co_await backend.close();
    auto closed_engine  = co_await engine.close();
    if (! co_await executor) co_return;
    if (closed_backend.is_err() && state->callbacks.error)
        state->callbacks.error(describe(closed_backend.unwrap_err()));
    if (closed_engine.is_err() && state->callbacks.error)
        state->callbacks.error(describe(closed_engine.errors[usize()]));
}

auto finish_connection(rstd::async::JoinHandle<void>       worker,
                       std::shared_ptr<std::promise<void>> done) -> coro<void> {
    auto result = co_await rstd::move(worker);
    if (result.is_err())
        done->set_exception(
            std::make_exception_ptr(std::runtime_error("Backend connection task failed")));
    else
        done->set_value();
}
} // namespace

class BackendConnection::Private {
public:
    Arc<ConnectionState>          state;
    rstd::async::RuntimeHandle    runtime;
    qt::CallbackExecutor          callback_executor;
    std::shared_future<void>      done;
    rstd::async::JoinHandle<void> worker;

    Private(BackendConnection* owner, rstd::async::RuntimeHandle handle,
            BackendConnectionCallbacks callbacks, BackendConnectionLimits limits)
        : state(Arc<ConnectionState>::make()),
          runtime(rstd::move(handle)),
          callback_executor(qt::CallbackExecutor::make(owner).unwrap()),
          worker([&] {
              state->callbacks = std::move(callbacks);
              state->limits    = limits;
              auto promise     = std::make_shared<std::promise<void>>();
              done             = promise->get_future().share();
              auto executor    = rstd::async::AnyExecutor::from_executor(callback_executor.clone());
              return runtime.spawn(
                  finish_connection(runtime.spawn(run_connection(
                                        state.clone(), runtime.clone(), std::move(executor))),
                                    std::move(promise)));
          }()) {}
};

BackendConnection::BackendConnection(rstd::async::RuntimeHandle runtime,
                                     BackendConnectionCallbacks callbacks,
                                     BackendConnectionLimits limits, QObject* parent)
    : QObject(parent),
      d(std::make_unique<Private>(this, rstd::move(runtime), std::move(callbacks), limits)) {}

BackendConnection::~BackendConnection() {
    Q_ASSERT(d->done.wait_for(std::chrono::seconds(0)) == std::future_status::ready);
    d->callback_executor.close();
}

void BackendConnection::open(QStringView url) {
    Q_ASSERT(thread() == QThread::currentThread());
    if (d->state->closing) return;
    auto encoded = url.toString().toStdString();
    auto parsed  = lihttpto::Url::parse_http(rstd::cppstd::as_str(encoded).unwrap());
    if (parsed.is_err()) {
        d->state->fail(d->state->generation, QStringLiteral("Invalid backend URL"));
        return;
    }
    ++d->state->generation;
    d->state->stop();
    d->state->target = Some(rstd::move(parsed).unwrap());
    d->state->changed.notify();
}

auto BackendConnection::send(QByteArray bytes) -> Result<empty, QString> {
    Q_ASSERT(thread() == QThread::currentThread());
    auto state = d->state.clone();
    if (state->closing || ! state->connected || ! state->client)
        return Err(QStringLiteral("Backend disconnected"));
    auto message  = Bytes::copy_from_slice(slice<u8>::from_raw_parts(
        reinterpret_cast<const rstd::byte*>(bytes.constData()), usize(bytes.size())));
    auto accepted = (*state->client)->send({ liwebsocketo::Kind::Binary, rstd::move(message) });
    if (accepted.is_err()) return Err(describe(accepted.unwrap_err().error));
    state->sends.retain([](const auto& task) {
        return ! task.is_finished();
    });
    auto executor = rstd::async::AnyExecutor::from_executor(d->callback_executor.clone());
    state->sends.push(d->runtime.spawn(watch_send(
        state.clone(), rstd::move(accepted).unwrap(), state->generation, std::move(executor))));
    return Ok(empty {});
}

void BackendConnection::disconnect() {
    Q_ASSERT(thread() == QThread::currentThread());
    ++d->state->generation;
    d->state->target.take();
    d->state->stop();
}

auto BackendConnection::shutdown() -> std::shared_future<void> {
    Q_ASSERT(thread() == QThread::currentThread());
    d->state->closing   = true;
    d->state->connected = false;
    d->state->target.take();
    if (! d->state->client && d->state->session) (*d->state->session)->request_close();
    d->state->changed.notify();
    return d->done;
}
} // namespace waywallen
