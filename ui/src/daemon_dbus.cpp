module;
#include "waywallen/daemon_dbus.moc.h"

#include <QtCore/QDebug>
#include <QtCore/QProcess>
#include <QtCore/QVariant>
#include <QtDBus/QDBusConnection>
#include <QtDBus/QDBusConnectionInterface>
#include <QtDBus/QDBusInterface>
#include <QtDBus/QDBusMessage>
#include <QtDBus/QDBusReply>
#include <QtDBus/QDBusServiceWatcher>

module waywallen;
import :daemon_dbus;

namespace waywallen
{

namespace {

constexpr const char* kBusName     = "org.waywallen.waywallen.Daemon";
constexpr const char* kObjectPath  = "/org/waywallen/waywallen/Daemon";
constexpr const char* kInterface   = "org.waywallen.waywallen.Daemon1";
constexpr const char* kPropsIface  = "org.freedesktop.DBus.Properties";

DaemonDBusClient* g_instance { nullptr };

} // namespace

DaemonDBusClient* DaemonDBusClient::create(QQmlEngine*, QJSEngine*) {
    auto* inst = instance();
    QJSEngine::setObjectOwnership(inst, QJSEngine::CppOwnership);
    return inst;
}

DaemonDBusClient* DaemonDBusClient::instance() {
    if (! g_instance) {
        g_instance = new DaemonDBusClient();
    }
    return g_instance;
}

DaemonDBusClient::DaemonDBusClient(QObject* parent)
    : QObject(parent), m_bus(QDBusConnection::sessionBus()) {
    if (! g_instance) {
        g_instance = this;
    }

    if (! m_bus.isConnected()) {
        qWarning("DaemonDBusClient: session bus not connected: %s",
                 qPrintable(m_bus.lastError().message()));
        return;
    }

    setup_subscriptions();

    // Initial availability + port probe.
    auto iface = m_bus.interface();
    bool registered =
        iface && iface->isServiceRegistered(QString::fromLatin1(kBusName)).value();
    if (registered) {
        update_availability(true);
        refreshWsPort();
    } else {
        update_availability(false);
    }
}

DaemonDBusClient::~DaemonDBusClient() {
    if (g_instance == this) {
        g_instance = nullptr;
    }
}

void DaemonDBusClient::setup_subscriptions() {
    m_watcher = new QDBusServiceWatcher(QString::fromLatin1(kBusName),
                                        m_bus,
                                        QDBusServiceWatcher::WatchForRegistration
                                            | QDBusServiceWatcher::WatchForUnregistration,
                                        this);
    connect(m_watcher, &QDBusServiceWatcher::serviceRegistered, this,
            &DaemonDBusClient::on_service_registered);
    connect(m_watcher, &QDBusServiceWatcher::serviceUnregistered, this,
            &DaemonDBusClient::on_service_unregistered);

    // Ready signal
    bool ok = m_bus.connect(QString::fromLatin1(kBusName),
                            QString::fromLatin1(kObjectPath),
                            QString::fromLatin1(kInterface),
                            QStringLiteral("Ready"),
                            this,
                            SLOT(on_ready()));
    if (! ok) {
        qWarning("DaemonDBusClient: failed to subscribe to Ready signal");
    }

    // ShuttingDown signal
    ok = m_bus.connect(QString::fromLatin1(kBusName),
                       QString::fromLatin1(kObjectPath),
                       QString::fromLatin1(kInterface),
                       QStringLiteral("ShuttingDown"),
                       this,
                       SLOT(on_shutting_down()));
    if (! ok) {
        qWarning("DaemonDBusClient: failed to subscribe to ShuttingDown signal");
    }

    // PropertiesChanged signal (filter by interface in slot).
    ok = m_bus.connect(QString::fromLatin1(kBusName),
                       QString::fromLatin1(kObjectPath),
                       QString::fromLatin1(kPropsIface),
                       QStringLiteral("PropertiesChanged"),
                       this,
                       SLOT(on_properties_changed(QString, QVariantMap, QStringList)));
    if (! ok) {
        qWarning("DaemonDBusClient: failed to subscribe to PropertiesChanged");
    }
}

quint16 DaemonDBusClient::refreshWsPort() {
    if (! m_bus.isConnected()) {
        return 0;
    }

    QDBusMessage msg = QDBusMessage::createMethodCall(QString::fromLatin1(kBusName),
                                                     QString::fromLatin1(kObjectPath),
                                                     QString::fromLatin1(kPropsIface),
                                                     QStringLiteral("Get"));
    msg << QString::fromLatin1(kInterface) << QStringLiteral("WsPort");

    // Short timeout; if daemon isn't there we want to fail fast.
    QDBusMessage reply = m_bus.call(msg, QDBus::Block, 2000);
    if (reply.type() != QDBusMessage::ReplyMessage) {
        qDebug("DaemonDBusClient: WsPort read failed: %s",
               qPrintable(reply.errorMessage()));
        update_availability(false);
        set_ws_port(0);
        return 0;
    }

    const auto args = reply.arguments();
    if (args.isEmpty()) {
        return m_ws_port;
    }
    QVariant inner = args.front();
    if (inner.canConvert<QDBusVariant>()) {
        inner = inner.value<QDBusVariant>().variant();
    }
    bool ok = false;
    quint16 port = static_cast<quint16>(inner.toUInt(&ok));
    if (ok) {
        update_availability(true);
        set_ws_port(port);
    }
    return m_ws_port;
}

bool DaemonDBusClient::launchDaemon() {
    qDebug("DaemonDBusClient: launching daemon (QProcess::startDetached)");
    bool ok = QProcess::startDetached(QStringLiteral("waywallen"), {});
    if (! ok) {
        qWarning("DaemonDBusClient: failed to start waywallen");
    }
    return ok;
}

void DaemonDBusClient::on_service_registered(const QString& service) {
    if (service != QString::fromLatin1(kBusName)) return;
    qDebug("DaemonDBusClient: daemon registered on bus");
    update_availability(true);
    refreshWsPort();
}

void DaemonDBusClient::on_service_unregistered(const QString& service) {
    if (service != QString::fromLatin1(kBusName)) return;
    qDebug("DaemonDBusClient: daemon unregistered from bus");
    update_availability(false);
    set_ws_port(0);
}

void DaemonDBusClient::on_ready() {
    qDebug("DaemonDBusClient: Ready signal received");
    update_availability(true);
    refreshWsPort();
}

void DaemonDBusClient::on_shutting_down() {
    qDebug("DaemonDBusClient: ShuttingDown signal received");
    update_availability(false);
    // Do not clear port aggressively here; NameOwnerChanged will follow.
}

void DaemonDBusClient::on_properties_changed(const QString&     iface,
                                             const QVariantMap& changed,
                                             const QStringList& /*invalidated*/) {
    if (iface != QString::fromLatin1(kInterface)) return;
    auto it = changed.find(QStringLiteral("WsPort"));
    if (it == changed.end()) return;
    QVariant v = it.value();
    if (v.canConvert<QDBusVariant>()) {
        v = v.value<QDBusVariant>().variant();
    }
    bool ok = false;
    quint16 port = static_cast<quint16>(v.toUInt(&ok));
    if (ok) {
        set_ws_port(port);
    }
}

void DaemonDBusClient::update_availability(bool available) {
    if (m_available == available) return;
    m_available = available;
    Q_EMIT daemonAvailabilityChanged(m_available);
}

void DaemonDBusClient::set_ws_port(quint16 port) {
    if (m_ws_port == port) return;
    m_ws_port = port;
    Q_EMIT wsPortChanged(m_ws_port);
}

} // namespace waywallen

#include "waywallen/daemon_dbus.moc"
