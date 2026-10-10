module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/update_checker.moc"
#endif

export module waywallen:update_checker;
import qextra.qt;

namespace waywallen
{

/// Looks up the latest GitHub release of waywallen and tells whether it is
/// newer than this build. The time of the last successful check and the
/// version it found are kept in QSettings, so a restart neither repeats the
/// request within the check interval nor loses a known update. Network
/// failures are silent and retried later.
///
/// A build without the `update-check` feature keeps the type, but it is not
/// `supported`: it stays disabled, sends no request and reports no update.
export class UpdateChecker : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

    Q_PROPERTY(bool supported READ supported CONSTANT FINAL)
    Q_PROPERTY(bool enabled READ enabled WRITE setEnabled NOTIFY enabledChanged FINAL)
    Q_PROPERTY(bool checking READ checking NOTIFY checkingChanged FINAL)
    Q_PROPERTY(bool updateAvailable READ updateAvailable NOTIFY latestVersionChanged FINAL)
    Q_PROPERTY(QString latestVersion READ latestVersion NOTIFY latestVersionChanged FINAL)
    Q_PROPERTY(QUrl releaseUrl READ releaseUrl NOTIFY latestVersionChanged FINAL)
    Q_PROPERTY(QDateTime lastCheckTime READ lastCheckTime NOTIFY lastCheckTimeChanged FINAL)

public:
    explicit UpdateChecker(QObject* parent = nullptr);

    static UpdateChecker* create(QQmlEngine*, QJSEngine*);
    static UpdateChecker* instance();

    bool             supported() const;
    bool             enabled() const { return m_enabled; }
    bool             checking() const { return m_reply != nullptr; }
    bool             updateAvailable() const;
    const QString&   latestVersion() const { return m_latest_version; }
    const QUrl&      releaseUrl() const { return m_release_url; }
    const QDateTime& lastCheckTime() const { return m_last_check_time; }
    void             setEnabled(bool enabled);

    /// Check now, regardless of the interval and of `enabled`.
    Q_INVOKABLE void check();

    Q_SIGNAL void enabledChanged();
    Q_SIGNAL void checkingChanged();
    Q_SIGNAL void latestVersionChanged();
    Q_SIGNAL void lastCheckTimeChanged();
    Q_SIGNAL void checkFinished(bool ok);

private:
    void schedule();
    void handleReply(QNetworkReply* reply);

    QNetworkAccessManager* m_network;
    QTimer                 m_timer;
    QNetworkReply*         m_reply { nullptr };
    QString                m_latest_version;
    QUrl                   m_release_url;
    QDateTime              m_last_check_time;
    bool                   m_enabled { true };
};

} // namespace waywallen
