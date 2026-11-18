module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/notify.moc"
#endif

export module waywallen:notify;
import qextra.qt;
export import :proto;

namespace waywallen
{

export struct TaskProgressSnapshot {
    double  progress { 0.0 };
    bool    progressing { false };
    bool    ended { false };
    bool    error { false };
    QString message;
    quint64 sequence { 0 };
};

/// UI-side mirror of the daemon's `GlobalEvent` broadcasts. The
/// daemon serializes process-wide events (sync lifecycle etc.) onto
/// `ServerFrame.event` over the WS; `Notify` subscribes to
/// `Backend::eventReceived` once at construction and re-emits each
/// daemon-global variant as a strongly-typed Qt signal so QML / C++
/// consumers don't have to inspect raw protobuf payloads.
///
/// `Notify` does **not** drive toast UX. Per-event toasts (if any)
/// belong with the consuming page using `Action::toast`. This object
/// is intentionally narrow: it relays daemon events, nothing more.
export class Notify : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

public:
    /// Mirror of `pb::DaemonPhase`. UI gates the startup dialog and
    /// initial query fan-out on this; default is `Starting` so the
    /// dialog wins until the first authoritative `StatusSync` arrives.
    enum class DaemonPhase
    {
        Starting = 0,
        Ready    = 1,
    };
    Q_ENUM(DaemonPhase)

    /// Mirrors `StatusSync.scan_in_progress`. Bind QML directly to
    /// this property — no need to count transient sync events, which
    /// can be lost on lag or late connect.
    Q_PROPERTY(bool scanInProgress READ scanInProgress NOTIFY statusChanged FINAL)
    /// Mirrors `StatusSync.active_task_count`. Number of TaskManager
    /// tasks currently in `Running`.
    Q_PROPERTY(quint32 activeTaskCount READ activeTaskCount NOTIFY statusChanged FINAL)
    /// Mirrors `StatusSync.phase`. Default `Starting` — UI shows a
    /// startup dialog until this flips to `Ready`. Reset to
    /// `Starting` when the WS disconnects so a daemon restart triggers
    /// the dialog again.
    Q_PROPERTY(DaemonPhase daemonPhase READ daemonPhase NOTIFY statusChanged FINAL)
    Q_PROPERTY(waywallen::control::v1::DisplayBackendStatus displayBackend READ displayBackend
                   NOTIFY statusChanged FINAL)
    Q_PROPERTY(bool globalPaused READ globalPaused NOTIFY statusChanged FINAL)
    Q_PROPERTY(bool globalMuted READ globalMuted NOTIFY statusChanged FINAL)
    Q_PROPERTY(bool globalStopped READ globalStopped NOTIFY statusChanged FINAL)

    Notify(QObject* parent);
    ~Notify() override;
    // QML should always reach us through `create` so we stay a
    // singleton parented to App.
    Notify() = delete;

    static auto    instance() -> Notify*;
    static Notify* create(QQmlEngine*, QJSEngine*);

    auto scanInProgress() const -> bool { return m_scan_in_progress; }
    auto activeTaskCount() const -> quint32 { return m_active_task_count; }
    auto daemonPhase() const -> DaemonPhase { return m_daemon_phase; }
    auto displayBackend() const -> const control::v1::DisplayBackendStatus& {
        return m_display_backend;
    }
    auto globalPaused() const -> bool { return m_global_paused; }
    auto globalMuted() const -> bool { return m_global_muted; }
    auto globalStopped() const -> bool { return m_global_stopped; }
    auto taskProgressSnapshot(const QString& queryId, TaskProgressSnapshot& out) const -> bool;
    auto taskProgressSequence() const -> quint64 { return m_task_progress_sequence; }

Q_SIGNALS:
    /// Daemon finished a wallpaper sync (success or failure). `count`
    /// is the total entry count after sync (0 on failure); `error` is
    /// empty on success, otherwise a one-line reason. Sync start is
    /// observable via the `scanInProgress` property.
    void wallpaperSyncFinished(quint32 count, const QString& error);
    void wallpaperHiddenChanged(const QStringList& wallpaperIds, bool hidden);
    /// Daemon added one or more libraries — manually via `LibraryAdd`
    /// or via `LibraryAutoDetect`. `paths` is the absolute roots that
    /// were just inserted. The matching `LibraryChanged` per-library
    /// state events still drive the library list update; this is the
    /// transient toast trigger.
    void librariesAdded(const QStringList& paths);
    /// Emitted whenever the daemon pushes a `StatusSync` snapshot
    /// (initial connect + every change), or on local resets such as
    /// WS disconnect. The `scanInProgress` / `activeTaskCount` /
    /// `daemonPhase` properties already reflect the new values.
    void statusChanged();
    /// Edge-triggered: fires when `daemonPhase` transitions
    /// `Starting → Ready`. Pages that need to fan out initial queries
    /// should listen here AND, in `Component.onCompleted`, level-check
    /// `daemonPhase === Ready` for pages constructed after the daemon
    /// is already ready.
    void daemonReady();
    /// Daemon broadcast a `SettingsChanged` event after a successful
    /// settings mutation (or schema-driven startup reconciliation). UI
    /// settings forms should re-fetch via `SettingsGetQuery` to pick
    /// up writes from peer clients. The payload itself is intentionally
    /// not relayed here — receivers re-query so they go through the
    /// same parsing path as the initial load.
    void settingsChanged();
    /// Daemon's display endpoint rejected an external display client at
    /// handshake (version mismatch / bad protocol name). Window.qml
    /// turns this into a toast.
    void displayConnectionFailed(const QString& clientName, quint32 clientProtocolVersion,
                                 quint32 errorCode, const QString& reason);
    void remoteDownloadProgress(const QString& sourceId, const QString& id, int state,
                                const QString& error);
    void qrLoginProgress(const QString& sessionId, const QString& pluginId, const QString& actionId,
                         int state, const QString& qrImage, const QVariant& displayValue,
                         const QVariant& error, const QVariant& title, const QVariant& instruction);
    void browserLoginProgress(const QString& sessionId, const QString& pluginId,
                              const QString& actionId, int state, const QVariant& displayValue,
                              const QVariant& error, const QVariant& title,
                              const QVariant& instruction);
    void pluginStateChanged();
    void playlistChanged();
    void pluginChanged();
    void pluginUpdateChanged();
    void pluginRestartFailed(const QString& pluginId, const QString& error);
    void systemWallpaperFailed(const QString& wallpaperId, const QString& error);
    void taskProgress(const QString& queryId, double progress, bool progressing, bool ended,
                      bool error, const QString& message, quint64 sequence);

private:
    bool                                 m_scan_in_progress { false };
    quint32                              m_active_task_count { 0 };
    DaemonPhase                          m_daemon_phase { DaemonPhase::Starting };
    control::v1::DisplayBackendStatus    m_display_backend;
    bool                                 m_global_paused { false };
    bool                                 m_global_muted { false };
    bool                                 m_global_stopped { false };
    QHash<QString, TaskProgressSnapshot> m_task_progress;
    quint64                              m_task_progress_sequence { 0 };
};

} // namespace waywallen
