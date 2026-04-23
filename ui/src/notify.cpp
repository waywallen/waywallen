module;
#include "waywallen/notify.moc.h"

module waywallen;
import :notify;
import :app;

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

Notify::Notify(QObject* parent): QObject(parent) {}
Notify::~Notify() = default;

void Notify::info(const QString& m) { Q_EMIT notified(Severity::Info, m); }
void Notify::success(const QString& m) { Q_EMIT notified(Severity::Success, m); }
void Notify::warning(const QString& m) { Q_EMIT notified(Severity::Warning, m); }
void Notify::error(const QString& m) { Q_EMIT notified(Severity::Error, m); }
void Notify::post(Severity sev, const QString& m) { Q_EMIT notified(sev, m); }

} // namespace waywallen

#include "waywallen/notify.moc.cpp"
