module;
#include "waywallen/notify.moc.h"

module waywallen;
import :notify;
import :app;

namespace proto = waywallen::control::v1;

namespace waywallen
{

auto Notify::instance() -> Notify* {
    // Lazy-constructed once; parented to App so it rides the normal
    // QObject ownership tree and gets cleaned up on app teardown.
    static Notify* the = new Notify(App::instance());
    return the;
}

Notify* Notify::create(QQmlEngine*, QJSEngine*) {
    auto n = instance();
    QJSEngine::setObjectOwnership(n, QJSEngine::CppOwnership);
    return n;
}

Notify::Notify(QObject* parent): QObject(parent) {
    // Subscribe to the daemon's server-event channel exactly once.
    // Backend lives for the App's lifetime; the connection is parented
    // to `this` so the QueuedConnection unwinds cleanly on shutdown.
    if (auto* backend = App::instance()->backend()) {
        connect(backend, &Backend::eventReceived, this,
                [this](const proto::Event& evt) {
                    if (evt.hasWallpaperScanStarted()) {
                        Q_EMIT wallpaperScanStarted();
                    } else if (evt.hasWallpaperScanCompleted()) {
                        const auto& done = evt.wallpaperScanCompleted();
                        Q_EMIT wallpaperScanCompleted(done.count(), done.error());
                    } else if (evt.hasLibrariesAdded()) {
                        const auto& src = evt.librariesAdded().paths();
                        QStringList paths;
                        paths.reserve(src.size());
                        for (const auto& p : src) {
                            paths.push_back(p);
                        }
                        Q_EMIT librariesAdded(paths);
                    } else if (evt.hasStatusSync()) {
                        const auto& s = evt.statusSync();
                        const bool    new_scan = s.scanInProgress();
                        const quint32 new_tasks = s.activeTaskCount();
                        if (new_scan != m_scan_in_progress || new_tasks != m_active_task_count) {
                            m_scan_in_progress  = new_scan;
                            m_active_task_count = new_tasks;
                            Q_EMIT statusChanged();
                        }
                    }
                },
                Qt::QueuedConnection);
    }
}
Notify::~Notify() = default;

} // namespace waywallen

#include "waywallen/notify.moc.cpp"
