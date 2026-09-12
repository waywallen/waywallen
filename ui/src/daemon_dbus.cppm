module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/daemon_dbus.moc"
#endif

export module waywallen:daemon_dbus;
import qextra;

export namespace waywallen
{

class DaemonDBusClient : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

public:
    /// Single source of truth for the daemon's reachability + compatibility.
    /// All UI bindings read this; per-flag bool variables are deliberately
    /// avoided so the state machine has one anchor.
    enum Status
    {
        Disconnected,    ///< Service not on bus, or DBus call failed.
        VersionMissing,  ///< Daemon online but lacks Version property (old build).
        VersionMismatch, ///< Daemon speaks a control-plane revision this build does
                         ///< not; on a daemon without `Capabilities`, its `Version`
                         ///< differs from this build's.
        Connected,       ///< WsPort + Version both ok and compatible.
    };
    Q_ENUM(Status)

    Q_PROPERTY(Status status READ status NOTIFY statusChanged FINAL)
    Q_PROPERTY(quint16 wsPort READ wsPort NOTIFY wsPortChanged FINAL)
    Q_PROPERTY(QString daemonVersion READ daemonVersion NOTIFY statusChanged FINAL)
    /// `Daemon1.Capabilities`: control-plane revision first, then optional
    /// feature names. Empty when the daemon predates the property.
    Q_PROPERTY(QStringList daemonCapabilities READ daemonCapabilities NOTIFY statusChanged FINAL)
    /// Convenience derived from `status == Connected`.
    Q_PROPERTY(bool daemonAvailable READ daemonAvailable NOTIFY statusChanged FINAL)
    Q_PROPERTY(bool quitOnDaemonShutdown READ quitOnDaemonShutdown WRITE setQuitOnDaemonShutdown
                   NOTIFY quitOnDaemonShutdownChanged FINAL)
    Q_PROPERTY(bool daemonShutdownExpected READ daemonShutdownExpected NOTIFY
                   daemonShutdownExpectedChanged FINAL)

    explicit DaemonDBusClient(QObject* parent = nullptr);
    ~DaemonDBusClient() override;

    static DaemonDBusClient* create(QQmlEngine*, QJSEngine*);
    static DaemonDBusClient* instance();

    Status             status() const { return m_status; }
    quint16            wsPort() const { return m_ws_port; }
    const QString&     daemonVersion() const { return m_daemon_version; }
    const QStringList& daemonCapabilities() const { return m_daemon_capabilities; }
    bool               daemonAvailable() const { return m_status == Connected; }
    bool               quitOnDaemonShutdown() const { return m_quit_on_daemon_shutdown; }
    bool               daemonShutdownExpected() const { return m_daemon_shutdown_expected; }
    void               setQuitOnDaemonShutdown(bool enabled);

    /// Synchronous round-trip: read WsPort, then probe Version. Updates
    /// `status` to one of {Disconnected, VersionMissing, VersionMismatch,
    /// Connected}. Returns the freshly-read port (0 on failure).
    Q_INVOKABLE quint16 refreshWsPort();

    /// True when the daemon advertises `name` in `Capabilities`. An optional
    /// feature is used only when this says so; unknown means unsupported.
    Q_INVOKABLE bool hasDaemonCapability(const QString& name) const {
        return m_daemon_capabilities.contains(name);
    }

    Q_INVOKABLE bool refreshDisplays();

    Q_INVOKABLE bool quitDaemon();

    /// Spawn the daemon as a detached child. Returns true on success.
    Q_INVOKABLE bool launchDaemon();

    /// Enumerate processes whose /proc/<pid>/comm equals "waywallen". Each
    /// entry: { "pid": uint, "cmdline": string }. Used by the "daemon not
    /// run" dialog to surface zombies before relaunching.
    Q_INVOKABLE QVariantList listWaywallenProcesses();

    /// Send SIGTERM to `pid`. Returns true if kill(2) succeeded.
    Q_INVOKABLE bool killProcess(quint32 pid);

    Q_SIGNAL void statusChanged();
    Q_SIGNAL void wsPortChanged(quint16 port);
    Q_SIGNAL void quitOnDaemonShutdownChanged();
    Q_SIGNAL void daemonShutdownExpectedChanged();

private:
    Q_SLOT void on_service_registered(const QString& service);
    Q_SLOT void on_service_unregistered(const QString& service);
    Q_SLOT void on_ready();
    Q_SLOT void on_shutting_down();
    Q_SLOT void on_properties_changed(const QString& iface, const QVariantMap& changed,
                                      const QStringList& invalidated);

    void setup_subscriptions();
    void set_status(Status s);
    void set_ws_port(quint16 port);
    void set_daemon_shutdown_expected(bool expected);

    /// `org.freedesktop.DBus.Properties.Get(kInterface, prop)`.
    QDBusMessage call_get(const QString& prop);

    /// Reads `Capabilities`; empty list when the daemon does not have it.
    QStringList read_capabilities();

    QDBusConnection      m_bus;
    QDBusServiceWatcher* m_watcher { nullptr };
    quint16              m_ws_port { 0 };
    QString              m_daemon_version;
    QStringList          m_daemon_capabilities;
    Status               m_status { Disconnected };
    bool                 m_quit_on_daemon_shutdown { true };
    bool                 m_daemon_shutdown_expected { false };
};

} // namespace waywallen
