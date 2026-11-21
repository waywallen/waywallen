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
/// On an AppImage run it also downloads the matching release asset next to the
/// running image and offers to restart into it. Native and Flatpak builds keep
/// the link-only behaviour.
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
    Q_PROPERTY(
        bool selfUpdateAvailable READ selfUpdateAvailable NOTIFY selfUpdateAvailableChanged FINAL)
    Q_PROPERTY(bool downloading READ downloading NOTIFY downloadStateChanged FINAL)
    Q_PROPERTY(qreal downloadProgress READ downloadProgress NOTIFY downloadProgressChanged FINAL)
    Q_PROPERTY(bool updateReady READ updateReady NOTIFY downloadStateChanged FINAL)
    Q_PROPERTY(QString updateError READ updateError NOTIFY downloadStateChanged FINAL)
    Q_PROPERTY(bool restarting READ restarting NOTIFY downloadStateChanged FINAL)

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
    bool             selfUpdateAvailable() const;
    bool             downloading() const { return m_download != nullptr; }
    qreal            downloadProgress() const { return m_download_progress; }
    bool             updateReady() const;
    const QString&   updateError() const { return m_update_error; }
    bool             restarting() const { return m_restarting; }
    void             setEnabled(bool enabled);

    /// Check now, regardless of the interval and of `enabled`.
    Q_INVOKABLE void check();

    /// Download the selected release asset next to the running AppImage.
    Q_INVOKABLE void download();

    /// Stop the daemon, start the downloaded AppImage and exit this UI.
    Q_INVOKABLE void restartToUpdate();

    Q_SIGNAL void enabledChanged();
    Q_SIGNAL void checkingChanged();
    Q_SIGNAL void latestVersionChanged();
    Q_SIGNAL void lastCheckTimeChanged();
    Q_SIGNAL void checkFinished(bool ok);
    Q_SIGNAL void selfUpdateAvailableChanged();
    Q_SIGNAL void downloadStateChanged();
    Q_SIGNAL void downloadProgressChanged();

private:
    void schedule();
    void handleReply(QNetworkReply* reply);
    void finish_download();
    void set_download_error(const QString& error);
    void on_restart_tick();
    void signal_daemon_terminate();
    void launch_new_image();
    void fail_restart(const QString& error);

    QString resolved_image() const;
    bool    can_self_update() const;
    QString part_path() const;

    QNetworkAccessManager* m_network;
    QTimer                 m_timer;
    QNetworkReply*         m_reply { nullptr };
    QNetworkReply*         m_download { nullptr };
    QFile*                 m_download_file { nullptr };
    QTimer                 m_restart_timer;
    QElapsedTimer          m_restart_clock;
    QString                m_latest_version;
    QUrl                   m_release_url;
    QUrl                   m_asset_url;
    qint64                 m_asset_size { 0 };
    qint64                 m_download_total { 0 };
    QString                m_download_version;
    QString                m_ready_version;
    QString                m_update_error;
    QDateTime              m_last_check_time;
    qreal                  m_download_progress { 0.0 };
    bool                   m_enabled { true };
    bool                   m_restarting { false };
    bool                   m_restart_sigterm_sent { false };
};

} // namespace waywallen
