module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/daemon_dbus.moc"
#endif

#include <QtCore/QVariantMap>
#include <QtDBus/QDBusConnection>
#include <QtDBus/QDBusServiceWatcher>
#include <QtDBus/QDBusVariant>

export module waywallen:daemon_dbus;
export import qextra;

export namespace waywallen
{

class DaemonDBusClient : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

    Q_PROPERTY(quint16 wsPort READ wsPort NOTIFY wsPortChanged FINAL)
    Q_PROPERTY(bool daemonAvailable READ daemonAvailable NOTIFY daemonAvailabilityChanged FINAL)

public:
    explicit DaemonDBusClient(QObject* parent = nullptr);
    ~DaemonDBusClient() override;

    static DaemonDBusClient* create(QQmlEngine*, QJSEngine*);
    static DaemonDBusClient* instance();

    quint16 wsPort() const { return m_ws_port; }
    bool    daemonAvailable() const { return m_available; }

    /// Synchronously read ws_port property; updates cache and emits signal on change.
    /// Returns current value (0 if unavailable).
    Q_INVOKABLE quint16 refreshWsPort();

    /// Spawn the daemon as a detached child. Returns true on success.
    Q_INVOKABLE bool launchDaemon();

    Q_SIGNAL void wsPortChanged(quint16 port);
    Q_SIGNAL void daemonAvailabilityChanged(bool available);

private:
    Q_SLOT void on_service_registered(const QString& service);
    Q_SLOT void on_service_unregistered(const QString& service);
    Q_SLOT void on_ready();
    Q_SLOT void on_shutting_down();
    Q_SLOT void on_properties_changed(const QString&     iface,
                                      const QVariantMap& changed,
                                      const QStringList& invalidated);

    void setup_subscriptions();
    void update_availability(bool available);
    void set_ws_port(quint16 port);

    QDBusConnection      m_bus;
    QDBusServiceWatcher* m_watcher { nullptr };
    quint16              m_ws_port { 0 };
    bool                 m_available { false };
};

} // namespace waywallen
