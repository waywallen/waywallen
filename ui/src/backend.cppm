module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/backend.moc"
#endif

export module waywallen:backend;
export import :proto;
import rstd;
import rstd.cppstd;
import qextra;

using rstd::boxed::Box;
using rstd::result::Result;
using rstd::sync::atomic::Atomic;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

export namespace waywallen
{

namespace detail
{
class BackendHelper;
} // namespace detail

class Backend : public QObject {
    Q_OBJECT

    friend class detail::BackendHelper;
    friend class App;

public:
    Backend(quint16 port);
    ~Backend();

    void connectTo();

    auto send(proto::Request&& req) -> task<Result<proto::Response, QString>>;

    Q_SIGNAL void connected();
    Q_SIGNAL void disconnected();
    Q_SIGNAL void error(QString);

    Q_SLOT void on_retry();

private:
    Q_SLOT void on_error(QString);
    Q_SLOT void on_connected();

    auto serial() -> quint64;

    Box<QThread>                    m_thread;
    Box<QtExecutionContext>         m_context;
    Box<ncrequest::WebSocketClient> m_client;
    Box<QProtobufSerializer>        m_serializer;

    cppstd::map<quint64, cppstd::move_only_function<void(asio::error_code, proto::Response)>>
        m_handlers;

    Atomic<quint64>      m_serial;
    quint16              m_port;
    QTimer*              m_reconnect_timer;
    int                  m_reconnect_delay;
    static constexpr int kMaxReconnectDelay = 30000;
};
} // namespace waywallen
