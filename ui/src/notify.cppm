module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/notify.moc"
#endif

export module waywallen:notify;
export import qextra;

namespace waywallen
{

/// UI-side mirror of the daemon's `GlobalEvent` broadcasts. The
/// daemon serializes process-wide events (scan lifecycle etc.) onto
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

    /// Mirrors `StatusSync.scan_in_progress`. Bind QML directly to
    /// this property — no need to count `wallpaperScanStarted`/
    /// `wallpaperScanCompleted` transitions, which can be lost on lag
    /// or late connect.
    Q_PROPERTY(bool scanInProgress READ scanInProgress NOTIFY statusChanged FINAL)
    /// Mirrors `StatusSync.active_task_count`. Number of TaskManager
    /// tasks currently in `Running`.
    Q_PROPERTY(quint32 activeTaskCount READ activeTaskCount NOTIFY statusChanged FINAL)

public:
    Notify(QObject* parent);
    ~Notify() override;
    // QML should always reach us through `create` so we stay a
    // singleton parented to App.
    Notify() = delete;

    static auto    instance() -> Notify*;
    static Notify* create(QQmlEngine*, QJSEngine*);

    auto scanInProgress() const -> bool { return m_scan_in_progress; }
    auto activeTaskCount() const -> quint32 { return m_active_task_count; }

Q_SIGNALS:
    /// Daemon began a wallpaper rescan (`GlobalEvent::ScanStarted`).
    void wallpaperScanStarted();
    /// Daemon finished a wallpaper rescan
    /// (`GlobalEvent::ScanCompleted` / `ScanFailed`). `count` is the
    /// total entry count after sync (0 on failure); `error` is empty
    /// on success, otherwise a one-line reason.
    void wallpaperScanCompleted(quint32 count, const QString& error);
    /// Daemon added one or more libraries — manually via `LibraryAdd`
    /// or via `LibraryAutoDetect`. `paths` is the absolute roots that
    /// were just inserted. The matching `LibraryChanged` per-library
    /// state events still drive the library list update; this is the
    /// transient toast trigger.
    void librariesAdded(const QStringList& paths);
    /// Emitted whenever the daemon pushes a `StatusSync` snapshot
    /// (initial connect + every change). The `scanInProgress` and
    /// `activeTaskCount` properties already reflect the new values
    /// when this fires.
    void statusChanged();

private:
    bool    m_scan_in_progress { false };
    quint32 m_active_task_count { 0 };
};

} // namespace waywallen
