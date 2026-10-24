module;
#include <algorithm>
#include <memory>
#include <future>
#include <rstd/macro.hpp>
#include "waywallen/backend.moc.h"

module waywallen;
import :app;
import waywallen.backend.connection;
import qextra;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;
using namespace rstd::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

namespace detail
{

class BackendTransport : public QObject {
public:
    explicit BackendTransport(Backend* backend): m_backend(backend) {}

    void initialize(rstd::async::RuntimeHandle runtime) {
        if (m_connection) return;
        m_serializer = std::make_unique<QProtobufSerializer>();

        BackendConnectionCallbacks callbacks;
        callbacks.error = [this](QString message) {
            fail(message);
        };
        callbacks.connected = [this] {
            m_connected = true;
            Q_EMIT m_backend->transportConnected();
        };
        callbacks.disconnected = [this] {
            m_connected = false;
            fail_pending(QStringLiteral("backend disconnected"));
            Q_EMIT m_backend->transportDisconnected();
        };
        callbacks.message = [this](QByteArray bytes) {
            proto::ServerFrame frame;
            if (! frame.deserialize(m_serializer.get(), bytes)) {
                fail(QStringLiteral("Invalid backend protobuf message"));
                return;
            }
            if (frame.hasResponse()) {
                auto response = frame.response();
                auto id       = response.requestId();
                (void)m_backend->m_pending.complete(id, rstd::move(response));
            } else if (frame.hasEvent()) {
                Q_EMIT m_backend->eventReceived(frame.event());
            } else {
                fail(QStringLiteral("Backend ServerFrame has no kind"));
            }
        };
        m_connection =
            std::make_unique<BackendConnection>(rstd::move(runtime), std::move(callbacks));
    }

    void connect_to(String url) {
        if (! m_connection || m_shutting_down) return;
        m_connected = false;
        fail_pending(QStringLiteral("backend connection replaced"));
        m_connection->open(qextra::to_qstring(url));
    }

    void disconnect_from_server() {
        if (m_connection) m_connection->disconnect();
        m_connected = false;
        fail_pending(QStringLiteral("backend disconnected"));
        Q_EMIT m_backend->transportDisconnected();
    }

    void send(proto::Request request) {
        auto id = request.requestId();
        if (! m_backend->m_pending.contains(id)) return;
        if (! m_connection || ! m_connected || m_shutting_down) {
            m_backend->m_pending.fail(id, QStringLiteral("backend disconnected"));
            return;
        }

        auto sent = m_connection->send(request.serialize(m_serializer.get()));
        if (sent.is_err()) {
            m_backend->m_pending.fail(id, sent.unwrap_err());
        }
    }

    void send_untracked(proto::Request request) {
        if (! m_connection || ! m_connected || m_shutting_down) return;
        auto sent = m_connection->send(request.serialize(m_serializer.get()));
        if (sent.is_err()) fail(sent.unwrap_err());
    }

    auto shutdown() -> std::shared_future<void> {
        m_shutting_down = true;
        m_connected     = false;
        fail_pending(QStringLiteral("backend shutting down"));
        if (m_connection) return m_connection->shutdown();
        std::promise<void> done;
        done.set_value();
        return done.get_future().share();
    }

    void finish_shutdown() {
        m_connection.reset();
        m_serializer.reset();
    }

private:
    void fail(const QString& error) {
        m_connected = false;
        fail_pending(error);
        if (m_connection) m_connection->disconnect();
        if (! m_shutting_down) Q_EMIT m_backend->error(error);
    }

    void fail_pending(const QString& error) { m_backend->m_pending.fail_all(error); }

    Backend*                             m_backend;
    std::unique_ptr<BackendConnection>   m_connection;
    std::unique_ptr<QProtobufSerializer> m_serializer;
    bool                                 m_connected {}, m_shutting_down {};
};

class RequestGuard {
public:
    RequestGuard(BackendPending<proto::Response, QString> pending, quint64 request_id)
        : m_pending(rstd::move(pending)), m_request_id(request_id) {}

    RequestGuard(const RequestGuard&)            = delete;
    RequestGuard& operator=(const RequestGuard&) = delete;
    RequestGuard(RequestGuard&&)                 = delete;
    RequestGuard& operator=(RequestGuard&&)      = delete;

    ~RequestGuard() { m_pending.cancel(m_request_id); }

private:
    BackendPending<proto::Response, QString> m_pending;
    quint64                                  m_request_id;
};

} // namespace detail

Backend::Backend(quint16 port)
    : m_thread(Box<QThread>::make()),
      m_transport(new detail::BackendTransport(this)),
      m_serial(1),
      m_port(port),
      m_reconnect_timer(nullptr),
      m_reconnect_delay(1000),
      m_disconnect_requested(false),
      m_connected(false) {
    m_transport->moveToThread(m_thread.get());
    connect(m_thread.get(), &QThread::finished, m_transport, &QObject::deleteLater);
    m_thread->start();
    connect(this, &Backend::transportConnected, this, &Backend::on_connected);
    connect(this, &Backend::error, this, &Backend::on_error);
    connect(this, &Backend::transportDisconnected, this, &Backend::on_disconnected);
}

Backend::~Backend() {
    std::shared_future<void> closed;
    (void)QMetaObject::invokeMethod(
        m_transport,
        [transport = m_transport, &closed] {
            closed = transport->shutdown();
        },
        Qt::BlockingQueuedConnection);
    try {
        closed.get();
    } catch (const std::exception& error) {
        qCritical("backend shutdown: %s", error.what());
    }
    (void)QMetaObject::invokeMethod(
        m_transport,
        [transport = m_transport] {
            transport->finish_shutdown();
        },
        Qt::BlockingQueuedConnection);
    m_thread->quit();
    m_thread->wait();
}

void Backend::connectTo() {
    if (m_port == 0) return;

    m_disconnect_requested = false;
    m_reconnect_delay      = 1000;
    if (m_reconnect_timer) m_reconnect_timer->stop();
    if (m_connected) {
        m_connected = false;
        Q_EMIT disconnected();
    }
    on_retry();
}

void Backend::on_retry() {
    if (m_port == 0 || m_disconnect_requested) return;
    auto url = rstd::format("http://127.0.0.1:{}", m_port);
    (void)QMetaObject::invokeMethod(
        m_transport,
        [transport = m_transport,
         url       = rstd::move(url),
         runtime   = QAsyncResult::runtime_handle()]() mutable {
            transport->initialize(rstd::move(runtime));
            transport->connect_to(rstd::move(url));
        },
        Qt::QueuedConnection);
}

void Backend::setPort(quint16 port) {
    if (m_port == port) return;
    m_port = port;
    if (m_reconnect_timer) m_reconnect_timer->stop();
}

void Backend::disconnect() {
    if (m_reconnect_timer) m_reconnect_timer->stop();
    m_disconnect_requested = true;
    (void)QMetaObject::invokeMethod(
        m_transport,
        [transport = m_transport] {
            transport->disconnect_from_server();
        },
        Qt::QueuedConnection);
}

void Backend::on_connected() {
    if (m_disconnect_requested) return;
    m_connected       = true;
    m_reconnect_delay = 1000;
    if (m_reconnect_timer) m_reconnect_timer->stop();

    auto request = proto::Request {};
    request.setRequestId(serial());
    request.setHealth(proto::HealthRequest {});
    (void)QMetaObject::invokeMethod(
        m_transport,
        [transport = m_transport, request = rstd::move(request)]() mutable {
            transport->send_untracked(rstd::move(request));
        },
        Qt::QueuedConnection);
    Q_EMIT connected();
}

void Backend::on_error(QString message) {
    qWarning("backend error: %s", qPrintable(message));
    if (m_connected) {
        m_connected = false;
        Q_EMIT disconnected();
    }
    if (m_disconnect_requested) return;

    if (! m_reconnect_timer) {
        m_reconnect_timer = new QTimer(this);
        m_reconnect_timer->setSingleShot(true);
        connect(m_reconnect_timer, &QTimer::timeout, this, &Backend::on_retry);
    }
    m_reconnect_timer->start(m_reconnect_delay);
    m_reconnect_delay = std::min(m_reconnect_delay * 2, kMaxReconnectDelay);
}

void Backend::on_disconnected() {
    if (m_connected) {
        m_connected = false;
        Q_EMIT disconnected();
    }
    if (m_disconnect_requested) {
        return;
    }
    if (m_reconnect_timer && m_reconnect_timer->isActive()) return;

    if (! m_reconnect_timer) {
        m_reconnect_timer = new QTimer(this);
        m_reconnect_timer->setSingleShot(true);
        connect(m_reconnect_timer, &QTimer::timeout, this, &Backend::on_retry);
    }
    m_reconnect_timer->start(m_reconnect_delay);
    m_reconnect_delay = std::min(m_reconnect_delay * 2, kMaxReconnectDelay);
}

auto Backend::send(proto::Request&& request) -> task<Result<proto::Response, QString>> {
    auto request_id = serial();
    request.setRequestId(request_id);

    auto completion_result = rstd::async::Completion<proto::Response, QString>::make();
    if (completion_result.is_err()) {
        co_return Err(QStringLiteral("failed to allocate request completion state"));
    }

    auto completion_pair = rstd::move(completion_result).unwrap_unchecked();
    auto completion      = rstd::move(completion_pair.get<0>());
    auto response        = rstd::move(completion_pair.get<1>());

    if (! m_pending.add(request_id, rstd::move(response))) {
        co_return Err(QStringLiteral("duplicate backend request id"));
    }
    auto guard = detail::RequestGuard { m_pending.clone(), request_id };

    auto posted = QMetaObject::invokeMethod(
        m_transport,
        [transport = m_transport, request = rstd::move(request)]() mutable {
            transport->send(rstd::move(request));
        },
        Qt::QueuedConnection);
    if (! posted) {
        co_return Err(QStringLiteral("backend transport is unavailable"));
    }

    auto result = co_await rstd::move(completion);
    if (result.is_err()) {
        auto error = rstd::move(result).unwrap_err_unchecked();
        if (error.is_failed()) {
            co_return Err(rstd::move(error).unwrap_failed());
        }
        co_return Err(QStringLiteral("request canceled"));
    }

    auto value = rstd::move(result).unwrap_unchecked();
    if (value.status() != proto::Status::OK) {
        auto error = value.message().isEmpty()
                         ? QStringLiteral("status %1").arg(static_cast<int>(value.status()))
                         : value.message();
        co_return Err(rstd::move(error));
    }
    co_return Ok(rstd::move(value));
}

auto Backend::serial() -> quint64 {
    quint64 current = m_serial.load();
    for (;;) {
        auto next = current + 1;
        if (m_serial.compare_exchange_strong(current, next)) break;
    }
    return current;
}

} // namespace waywallen

#include "waywallen/backend.moc"
