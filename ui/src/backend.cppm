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
import waywallen.backend.connection;

using rstd::boxed::Box;
using rstd::result::Result;
using rstd::sync::atomic::Atomic;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

export namespace waywallen
{

namespace detail
{
class BackendTransport;
} // namespace detail

class Backend : public QObject {
    Q_OBJECT

    friend class App;
    friend class detail::BackendTransport;

public:
    Backend(quint16 port);
    ~Backend();

    void    connectTo();
    void    setPort(quint16 port);
    quint16 port() const { return m_port; }
    void    disconnect();

    auto send(proto::Request&& req) -> task<Result<proto::Response, QString>>;

    Q_SIGNAL void connected();
    Q_SIGNAL void disconnected();
    Q_SIGNAL void error(QString);
    /// Server-initiated event (no request_id). Carried in
    /// `ServerFrame.event` and dispatched on the ws thread; receivers
    /// should marshal to the main thread via queued connections.
    Q_SIGNAL void eventReceived(proto::Event evt);

    Q_SLOT void on_retry();

private:
    Q_SIGNAL void transportConnected();
    Q_SIGNAL void transportDisconnected();

    Q_SLOT void on_error(QString);
    Q_SLOT void on_connected();
    Q_SLOT void on_disconnected();

    auto serial() -> quint64;

    Box<QThread>                             m_thread;
    detail::BackendTransport*                m_transport;
    BackendPending<proto::Response, QString> m_pending;

    Atomic<quint64>      m_serial;
    quint16              m_port;
    QTimer*              m_reconnect_timer;
    int                  m_reconnect_delay;
    bool                 m_disconnect_requested;
    bool                 m_connected;
    static constexpr int kMaxReconnectDelay = 30000;
};
} // namespace waywallen
