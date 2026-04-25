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
                    }
                },
                Qt::QueuedConnection);
    }
}
Notify::~Notify() = default;

} // namespace waywallen

#include "waywallen/notify.moc.cpp"
