module;
#include "waywallen/backend.moc.h"

#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :app;
import qextra;
import ncrequest.event;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;
using namespace rstd::prelude;

namespace proto = waywallen::control::v1;

#ifdef _WIN32
template<typename T>
using stream_type = asio::basic_stream_socket<asio::generic::stream_protocol, T>;
#else
template<typename T>
using stream_type = asio::posix::basic_stream_descriptor<T>;
#endif

namespace waywallen
{

namespace detail
{
class BackendHelper {
public:
    template<typename CompletionToken>
    static auto async_send(Backend& backend, proto::Request&& req, CompletionToken&& token) {
        using ret = void(asio::error_code, proto::Response);
        return asio::async_initiate<CompletionToken, ret>(
            [&](auto&& handler) {
                asio::dispatch(
                    backend.m_context->get_executor(),
                    [&backend, req = std::move(req), handler = std::move(handler)] mutable {
                        auto id = req.requestId();
                        backend.m_handlers.insert_or_assign(
                            id, std::move_only_function<ret> { std::move(handler) });
                        auto bytes = req.serialize(backend.m_serializer.get());
                        backend.m_client->send({ bytes.constData(), (std::size_t)bytes.size() });
                    });
            },
            token);
    }
};
} // namespace detail

Backend::Backend(quint16 port)
    : m_thread(Box<QThread>::make()),
      m_context(
          Box<QtExecutionContext>::make(m_thread.get(), (QEvent::Type)QEvent::registerEventType())),
      m_client(Box<ncrequest::WebSocketClient>::make(
          ncrequest::event::create<stream_type>(m_context->get_executor()))),
      m_serializer(Box<QProtobufSerializer>::make()),
      m_serial(1),
      m_port(port),
      m_reconnect_timer(nullptr),
      m_reconnect_delay(1000) {
    m_client->set_on_error_callback([this](auto err) {
        qWarning("ws error: %s", err.data());
        Q_EMIT this->error(QString::fromUtf8((const char*)err.data(), err.size()));
    });
    m_client->set_on_connected_callback([this]() {
        Q_EMIT this->connected();
    });
    m_client->set_on_message_callback([this, cache = std::make_shared<std::vector<std::byte>>()](
                                          std::span<const std::byte> bytes, bool last) {
        if (! last) {
            std::ranges::copy(bytes, std::back_inserter(*cache));
            return;
        }

        proto::ServerFrame frame;
        if (cache->empty()) {
            frame.deserialize(m_serializer.get(), bytes);
        } else {
            std::ranges::copy(bytes, std::back_inserter(*cache));
            frame.deserialize(m_serializer.get(), *cache);
            cache->clear();
        }

        if (frame.hasResponse()) {
            auto rsp = frame.response();
            if (auto it = m_handlers.find(rsp.requestId()); it != m_handlers.end()) {
                it->second(asio::error_code {}, std::move(rsp));
                m_handlers.erase(it);
            } else {
                qDebug("ws: unmatched response id=%llu status=%d",
                       (unsigned long long)rsp.requestId(),
                       (int)rsp.status());
            }
        } else if (frame.hasEvent()) {
            Q_EMIT this->eventReceived(frame.event());
        } else {
            qWarning("ws: ServerFrame with no kind set");
        }
    });

    // start ws thread
    {
        m_thread->start();
    }

    // connect signals
    {
        connect(this, &Backend::connected, this, &Backend::on_connected);
        connect(this, &Backend::error, this, &Backend::on_error);
    }

    {
        asio::post(m_context->get_executor(), [] {
            // name the ws thread for debugging
        });
    }
}

Backend::~Backend() {
    m_thread->quit();
    m_thread->wait();
    m_client.reset();
}

void Backend::connectTo() {
    if (m_port == 0) {
        qDebug("backend: port is 0, skipping connect (waiting for daemon)");
        return;
    }
    m_reconnect_delay = 1000;
    qDebug("connecting to ws://127.0.0.1:%d", (int)m_port);
    m_client->connect(std::format("ws://127.0.0.1:{}", m_port));
}

void Backend::setPort(quint16 port) {
    if (m_port == port) return;
    m_port = port;
    if (m_reconnect_timer) {
        m_reconnect_timer->stop();
    }
}

void Backend::disconnect() {
    if (m_reconnect_timer) {
        m_reconnect_timer->stop();
    }
    // Dropping port signals "no daemon"; the ws client will surface an error
    // on its own when the TCP connection drops.
}

void Backend::on_connected() {
    qDebug("ws connected to port %d", (int)m_port);
    m_reconnect_delay = 1000;
    if (m_reconnect_timer) {
        m_reconnect_timer->stop();
    }

    // Send health check as connection test.
    auto req = proto::Request {};
    req.setRequestId(serial());
    req.setHealth(proto::HealthRequest {});
    auto bytes = req.serialize(m_serializer.get());
    m_client->send({ bytes.constData(), (std::size_t)bytes.size() });
}

void Backend::on_error(QString msg) {
    qWarning("backend error: %s", qPrintable(msg));
    Q_EMIT this->disconnected();

    // Schedule reconnect with exponential backoff.
    if (! m_reconnect_timer) {
        m_reconnect_timer = new QTimer(this);
        m_reconnect_timer->setSingleShot(true);
        connect(m_reconnect_timer, &QTimer::timeout, this, &Backend::on_retry);
    }
    qDebug("reconnecting in %d ms", m_reconnect_delay);
    m_reconnect_timer->start(m_reconnect_delay);
    m_reconnect_delay = std::min(m_reconnect_delay * 2, kMaxReconnectDelay);
}

void Backend::on_retry() { connectTo(); }

auto Backend::send(proto::Request&& req) -> task<Result<proto::Response, QString>> {
    req.setRequestId(serial());
    auto [ec, rsp] =
        co_await detail::BackendHelper::async_send(*this, std::move(req), asio::as_tuple(use_task));
    if (ec) {
        co_return Err(QString::fromStdString(ec.message()));
    }
    if (rsp.status() != proto::Status::OK) {
        QString err =
            rsp.message().isEmpty() ? QString("status %1").arg((int)rsp.status()) : rsp.message();
        co_return Err(err);
    }
    co_return Ok(std::move(rsp));
}

auto Backend::serial() -> quint64 {
    quint64 cur = m_serial.load();
    for (;;) {
        const quint64 to = cur + 1;
        if (m_serial.compare_exchange_strong(cur, to)) {
            break;
        }
    }
    return cur;
}

} // namespace waywallen

#include "waywallen/backend.moc"
