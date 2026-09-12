module;
#include "waywallen/daemon_dbus.moc.h"

#include <csignal>
#include <sys/types.h>

module waywallen;
import qextra;
import :daemon_dbus;

namespace waywallen
{

namespace
{

constexpr const char* kBusName                 = "org.waywallen.waywallen.Daemon";
constexpr const char* kObjectPath              = "/org/waywallen/waywallen/Daemon";
constexpr const char* kInterface               = "org.waywallen.waywallen.Daemon1";
constexpr const char* kPropsIface              = "org.freedesktop.DBus.Properties";
constexpr const char* kQuitOnDaemonShutdownKey = "quitOnDaemonShutdown";
/// Control-plane revision this build speaks; see `Daemon1.Capabilities`.
constexpr const char* kControlRevision = "control.v1";

DaemonDBusClient* g_instance { nullptr };

bool is_unknown_property_error(const QString& name) {
    // Different DBus stacks report the missing-property condition with
    // slightly different error names; accept both.
    return name == QLatin1String("org.freedesktop.DBus.Error.UnknownProperty") ||
           name == QLatin1String("org.freedesktop.DBus.Error.InvalidArgs");
}

QVariant unwrap_variant(QVariant v) {
    if (v.canConvert<QDBusVariant>()) {
        v = v.value<QDBusVariant>().variant();
    }
    return v;
}

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

    m_quit_on_daemon_shutdown =
        QSettings().value(QString::fromLatin1(kQuitOnDaemonShutdownKey), true).toBool();

    if (! m_bus.isConnected()) {
        qWarning("DaemonDBusClient: session bus not connected: %s",
                 qPrintable(m_bus.lastError().message()));
        return;
    }

    setup_subscriptions();

    // Initial probe — refreshWsPort drives the full state-machine update.
    auto iface      = m_bus.interface();
    bool registered = iface && iface->isServiceRegistered(QString::fromLatin1(kBusName)).value();
    if (registered) {
        refreshWsPort();
    } else {
        set_status(Disconnected);
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
                                        QDBusServiceWatcher::WatchForRegistration |
                                            QDBusServiceWatcher::WatchForUnregistration,
                                        this);
    connect(m_watcher,
            &QDBusServiceWatcher::serviceRegistered,
            this,
            &DaemonDBusClient::on_service_registered);
    connect(m_watcher,
            &QDBusServiceWatcher::serviceUnregistered,
            this,
            &DaemonDBusClient::on_service_unregistered);

    bool ok = m_bus.connect(QString::fromLatin1(kBusName),
                            QString::fromLatin1(kObjectPath),
                            QString::fromLatin1(kInterface),
                            QStringLiteral("Ready"),
                            this,
                            SLOT(on_ready()));
    if (! ok) {
        qWarning("DaemonDBusClient: failed to subscribe to Ready signal");
    }

    ok = m_bus.connect(QString::fromLatin1(kBusName),
                       QString::fromLatin1(kObjectPath),
                       QString::fromLatin1(kInterface),
                       QStringLiteral("ShuttingDown"),
                       this,
                       SLOT(on_shutting_down()));
    if (! ok) {
        qWarning("DaemonDBusClient: failed to subscribe to ShuttingDown signal");
    }

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

QDBusMessage DaemonDBusClient::call_get(const QString& prop) {
    QDBusMessage msg = QDBusMessage::createMethodCall(QString::fromLatin1(kBusName),
                                                      QString::fromLatin1(kObjectPath),
                                                      QString::fromLatin1(kPropsIface),
                                                      QStringLiteral("Get"));
    msg << QString::fromLatin1(kInterface) << prop;
    return m_bus.call(msg, QDBus::Block, 2000);
}

quint16 DaemonDBusClient::refreshWsPort() {
    if (! m_bus.isConnected()) {
        set_ws_port(0);
        set_status(Disconnected);
        return 0;
    }

    // Step 1: WsPort. Failure here means daemon is gone / unreachable.
    QDBusMessage port_reply = call_get(QStringLiteral("WsPort"));
    if (port_reply.type() != QDBusMessage::ReplyMessage) {
        qDebug("DaemonDBusClient: WsPort read failed: %s", qPrintable(port_reply.errorMessage()));
        set_ws_port(0);
        set_status(Disconnected);
        return 0;
    }
    {
        const auto args = port_reply.arguments();
        if (! args.isEmpty()) {
            bool    ok   = false;
            quint16 port = static_cast<quint16>(unwrap_variant(args.front()).toUInt(&ok));
            if (ok) set_ws_port(port);
        }
    }

    // Step 2: Version. Best-effort — UnknownProperty/InvalidArgs maps to
    // VersionMissing (old daemon predating the version handshake).
    QDBusMessage ver_reply = call_get(QStringLiteral("Version"));
    if (ver_reply.type() != QDBusMessage::ReplyMessage) {
        if (is_unknown_property_error(ver_reply.errorName())) {
            m_daemon_version.clear();
            set_status(VersionMissing);
        } else {
            qDebug("DaemonDBusClient: Version read failed: %s (%s)",
                   qPrintable(ver_reply.errorName()),
                   qPrintable(ver_reply.errorMessage()));
            set_status(Disconnected);
        }
        return m_ws_port;
    }
    {
        const auto args = ver_reply.arguments();
        if (args.isEmpty()) {
            m_daemon_version.clear();
            m_daemon_capabilities.clear();
            set_status(VersionMissing);
            return m_ws_port;
        }
        m_daemon_version = unwrap_variant(args.front()).toString();
    }

    // Step 3: Capabilities. The daemon names the control-plane revision it
    // speaks, then its optional features; gate on the revision rather than on
    // the release string. A daemon that predates the property answers
    // UnknownProperty and leaves the list empty — keep comparing versions
    // there, so an old daemon is treated exactly as before.
    m_daemon_capabilities = read_capabilities();
    const bool compatible = m_daemon_capabilities.isEmpty()
                                ? m_daemon_version == QCoreApplication::applicationVersion()
                                : m_daemon_capabilities.contains(QLatin1String(kControlRevision));
    set_status(compatible ? Connected : VersionMismatch);
    return m_ws_port;
}

QStringList DaemonDBusClient::read_capabilities() {
    QDBusMessage reply = call_get(QStringLiteral("Capabilities"));
    if (reply.type() != QDBusMessage::ReplyMessage) {
        if (! is_unknown_property_error(reply.errorName())) {
            qDebug("DaemonDBusClient: Capabilities read failed: %s (%s)",
                   qPrintable(reply.errorName()),
                   qPrintable(reply.errorMessage()));
        }
        return {};
    }
    const auto args = reply.arguments();
    if (args.isEmpty()) return {};
    return unwrap_variant(args.front()).toStringList();
}

bool DaemonDBusClient::refreshDisplays() {
    if (! m_bus.isConnected() || m_status != Connected) return false;

    QDBusMessage msg   = QDBusMessage::createMethodCall(QString::fromLatin1(kBusName),
                                                        QString::fromLatin1(kObjectPath),
                                                        QString::fromLatin1(kInterface),
                                                        QStringLiteral("RefreshDisplays"));
    QDBusMessage reply = m_bus.call(msg, QDBus::Block, 2000);
    if (reply.type() == QDBusMessage::ReplyMessage) return true;

    qWarning("DaemonDBusClient: RefreshDisplays failed: %s", qPrintable(reply.errorMessage()));
    return false;
}

bool DaemonDBusClient::quitDaemon() {
    if (! m_bus.isConnected() || m_status != Connected) return false;

    set_daemon_shutdown_expected(true);
    QDBusMessage msg   = QDBusMessage::createMethodCall(QString::fromLatin1(kBusName),
                                                        QString::fromLatin1(kObjectPath),
                                                        QString::fromLatin1(kInterface),
                                                        QStringLiteral("Quit"));
    QDBusMessage reply = m_bus.call(msg, QDBus::Block, 2000);
    if (reply.type() == QDBusMessage::ReplyMessage) return true;

    set_daemon_shutdown_expected(false);
    qWarning("DaemonDBusClient: Quit failed: %s", qPrintable(reply.errorMessage()));
    return false;
}

bool DaemonDBusClient::launchDaemon() {
    set_daemon_shutdown_expected(false);
    qDebug("DaemonDBusClient: launching daemon (QProcess::startDetached)");
    bool ok = QProcess::startDetached(QStringLiteral("waywallen"), {});
    if (! ok) {
        qWarning("DaemonDBusClient: failed to start waywallen");
    }
    return ok;
}

QVariantList DaemonDBusClient::listWaywallenProcesses() {
    QVariantList      out;
    QDir              proc(QStringLiteral("/proc"));
    const QStringList entries = proc.entryList(QDir::Dirs | QDir::NoDotAndDotDot);
    for (const QString& entry : entries) {
        bool          is_pid = false;
        const quint32 pid    = entry.toUInt(&is_pid);
        if (! is_pid) continue;

        QFile comm_file(QStringLiteral("/proc/%1/comm").arg(entry));
        if (! comm_file.open(QIODevice::ReadOnly)) continue;
        QByteArray comm = comm_file.readAll().trimmed();
        if (comm != QByteArrayLiteral("waywallen")) continue;

        QFile   cmdline_file(QStringLiteral("/proc/%1/cmdline").arg(entry));
        QString cmdline;
        if (cmdline_file.open(QIODevice::ReadOnly)) {
            QByteArray raw = cmdline_file.readAll();
            // /proc cmdline is NUL-separated argv with a trailing NUL.
            for (char& c : raw) {
                if (c == '\0') c = ' ';
            }
            cmdline = QString::fromLocal8Bit(raw).trimmed();
        }
        if (cmdline.isEmpty()) cmdline = QString::fromLatin1(comm);

        QVariantMap row;
        row.insert(QStringLiteral("pid"), pid);
        row.insert(QStringLiteral("cmdline"), cmdline);
        out.append(row);
    }
    return out;
}

bool DaemonDBusClient::killProcess(quint32 pid) {
    if (pid == 0) return false;
    if (::kill(static_cast<pid_t>(pid), SIGTERM) != 0) {
        qWarning("DaemonDBusClient: kill(%u, SIGTERM) failed: %s",
                 pid,
                 qPrintable(QString::fromLocal8Bit(strerror(errno))));
        return false;
    }
    return true;
}

void DaemonDBusClient::on_service_registered(const QString& service) {
    if (service != QString::fromLatin1(kBusName)) return;
    qDebug("DaemonDBusClient: daemon registered on bus");
    set_daemon_shutdown_expected(false);
    refreshWsPort();
}

void DaemonDBusClient::on_service_unregistered(const QString& service) {
    if (service != QString::fromLatin1(kBusName)) return;
    qDebug("DaemonDBusClient: daemon unregistered from bus");
    set_ws_port(0);
    m_daemon_version.clear();
    m_daemon_capabilities.clear();
    set_status(Disconnected);
}

void DaemonDBusClient::on_ready() {
    qDebug("DaemonDBusClient: Ready signal received");
    refreshWsPort();
}

void DaemonDBusClient::on_shutting_down() {
    qDebug("DaemonDBusClient: ShuttingDown signal received");
    set_daemon_shutdown_expected(true);
    if (m_quit_on_daemon_shutdown) {
        QCoreApplication::quit();
        return;
    }
    set_status(Disconnected);
}

void DaemonDBusClient::on_properties_changed(const QString& iface, const QVariantMap& changed,
                                             const QStringList& /*invalidated*/) {
    if (iface != QString::fromLatin1(kInterface)) return;
    auto it = changed.find(QStringLiteral("WsPort"));
    if (it == changed.end()) return;
    bool    ok   = false;
    quint16 port = static_cast<quint16>(unwrap_variant(it.value()).toUInt(&ok));
    if (ok) {
        set_ws_port(port);
    }
}

void DaemonDBusClient::set_status(Status s) {
    if (m_status == s) return;
    m_status = s;
    Q_EMIT statusChanged();
}

void DaemonDBusClient::setQuitOnDaemonShutdown(bool enabled) {
    if (m_quit_on_daemon_shutdown == enabled) return;
    m_quit_on_daemon_shutdown = enabled;
    QSettings().setValue(QString::fromLatin1(kQuitOnDaemonShutdownKey), enabled);
    Q_EMIT quitOnDaemonShutdownChanged();
}

void DaemonDBusClient::set_ws_port(quint16 port) {
    if (m_ws_port == port) return;
    m_ws_port = port;
    Q_EMIT wsPortChanged(m_ws_port);
}

void DaemonDBusClient::set_daemon_shutdown_expected(bool expected) {
    if (m_daemon_shutdown_expected == expected) return;
    m_daemon_shutdown_expected = expected;
    Q_EMIT daemonShutdownExpectedChanged();
}

} // namespace waywallen

#include "waywallen/daemon_dbus.moc"
