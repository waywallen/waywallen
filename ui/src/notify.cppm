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

public:
    Notify(QObject* parent);
    ~Notify() override;
    // QML should always reach us through `create` so we stay a
    // singleton parented to App.
    Notify() = delete;

    static auto    instance() -> Notify*;
    static Notify* create(QQmlEngine*, QJSEngine*);

Q_SIGNALS:
    /// Daemon began a wallpaper rescan (`GlobalEvent::ScanStarted`).
    void wallpaperScanStarted();
    /// Daemon finished a wallpaper rescan
    /// (`GlobalEvent::ScanCompleted` / `ScanFailed`). `count` is the
    /// total entry count after sync (0 on failure); `error` is empty
    /// on success, otherwise a one-line reason.
    void wallpaperScanCompleted(quint32 count, const QString& error);
};

} // namespace waywallen
