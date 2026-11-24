module;
#include "waywallen/action.moc.h"
#include <QtDBus/QDBusPendingCallWatcher>
#include <QtDBus/QDBusPendingReply>

module waywallen;
import qextra;
import qextra.qt;
import :action;
import :app;

namespace waywallen
{

auto Action::instance() -> Action* {
    static Action* the = new Action(App::instance());
    return the;
}

Action* Action::create(QQmlEngine*, QJSEngine*) {
    auto a = instance();
    QJSEngine::setObjectOwnership(a, QJSEngine::CppOwnership);
    return a;
}

Action::Action(QObject* parent): QObject(parent) {}
Action::~Action() = default;

auto Action::wallpaperSelectStorage() const -> QObject* { return m_wallpaper_select_storage; }

void Action::copyToClipboard(const QString& text) { QGuiApplication::clipboard()->setText(text); }

namespace
{
constexpr const char* kPlasmaWallpaperPlugin = "org.waywallen.kde";
} // namespace

bool Action::hasPlasmaWallpaperPlugin() const {
    const auto path = QStringLiteral("plasma/wallpapers/%1/metadata.json")
                          .arg(QLatin1String(kPlasmaWallpaperPlugin));
    return ! QStandardPaths::locate(QStandardPaths::GenericDataLocation, path).isEmpty();
}

void Action::usePlasmaWallpaperPlugin() {
    auto msg = QDBusMessage::createMethodCall(QStringLiteral("org.kde.plasmashell"),
                                              QStringLiteral("/PlasmaShell"),
                                              QStringLiteral("org.kde.PlasmaShell"),
                                              QStringLiteral("evaluateScript"));
    msg << QStringLiteral("desktops().forEach(d => { d.wallpaperPlugin = \"%1\"; })")
               .arg(QLatin1String(kPlasmaWallpaperPlugin));
    auto* watcher =
        new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(msg, 10000), this);
    connect(watcher, &QDBusPendingCallWatcher::finished, this, [this](QDBusPendingCallWatcher* w) {
        w->deleteLater();
        const QDBusPendingReply<QString> reply = *w;
        Q_EMIT plasmaWallpaperPluginSet(reply.isError() ? reply.error().message() : QString());
    });
}

void Action::enterWallpaperSelect(QObject* storage) {
    if (m_wallpaper_select_storage != storage) {
        m_wallpaper_select_storage = storage;
        Q_EMIT wallpaperSelectStorageChanged();
    }
    Q_EMIT wallpaperSelectEntered(storage);
}

} // namespace waywallen

#include "waywallen/action.moc.cpp"
