module;
#include "waywallen/app.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
import :app;
import :display;
import :gpu;
import :renderer;
import :presentation;
import :query;
import :notify;
import :plugin_translation;
import :ui_language;

using namespace waywallen;
using namespace Qt::Literals::StringLiterals;

namespace proto = waywallen::control::v1;

namespace
{

constexpr const char* kUiBusName    = "org.waywallen.waywallen.UI";
constexpr const char* kUiObjectPath = "/org/waywallen/waywallen/UI";
constexpr const char* kUiInterface  = "org.waywallen.waywallen.UI1";

} // namespace

auto app_instance(waywallen::App* in = nullptr) -> waywallen::App* {
    static waywallen::App* instance { in };
    rstd_assert(instance != nullptr, "app object not inited");
    rstd_assert(in == nullptr || instance == in, "there should be only one app object");
    return instance;
}

class AppPrivate {
public:
    struct RuntimeLifetime {
        ~RuntimeLifetime() { QAsyncResult::dropEx(); }
    };

    AppPrivate(App* self, quint16 port)
        : m_p(self),
          m_main_win(nullptr),
          m_qml_network_cache(1024ll * 1024ll * 1024ll),
          m_backend(Box<Backend>::make(port)),
          m_display_mgr(Box<DisplayManager>::make()),
          m_presentation_mgr(Box<PresentationManager>::make()),
          m_renderer_mgr(Box<RendererManager>::make()),
          m_library_mgr(Box<LibraryManager>::make()),
          m_gpu_mgr(Box<GpuManager>::make()),
          m_qml_engine(Box<QQmlApplicationEngine>::make()),
          m_ui_language(Box<UiLanguageController>::make(
              *qobject_cast<QGuiApplication*>(QGuiApplication::instance()), *m_qml_engine)),
          m_port(port) {}
    ~AppPrivate() { save_settings(); }

    void save_settings() {}

    App*                   m_p;
    QPointer<QQuickWindow> m_main_win;
    QmlNetworkDiskCache    m_qml_network_cache;
    RuntimeLifetime        m_runtime;
    // Reverse dependency order keeps the QML engine first and Backend last during destruction.
    Box<Backend>               m_backend;
    Box<DisplayManager>        m_display_mgr;
    Box<PresentationManager>   m_presentation_mgr;
    Box<RendererManager>       m_renderer_mgr;
    Box<LibraryManager>        m_library_mgr;
    Box<GpuManager>            m_gpu_mgr;
    Box<QQmlApplicationEngine> m_qml_engine;
    Box<UiLanguageController>  m_ui_language;
    qint64                     m_network_cache_size { 0 };
    quint16                    m_port;
    bool                       m_ui_raise_registered { false };
};

namespace waywallen
{

App* App::create(QQmlEngine*, QJSEngine*) {
    auto app = app_instance();
    // not delete by qml
    QJSEngine::setObjectOwnership(app, QJSEngine::CppOwnership);
    return app;
}

App* App::instance() { return app_instance(); }

App::App(quint16 port, rstd::empty): QObject(nullptr), d_ptr(new AppPrivate(this, port)) {
    app_instance(this);
    QGuiApplication::instance()->installEventFilter(this);
}

App::~App() { QGuiApplication::instance()->removeEventFilter(this); }

void App::init() {
    Q_D(App);
    auto engine = this->engine();
    d->m_qml_network_cache.install(*engine);
    refreshNetworkCacheSize();

    QAsyncResult::initEx(this, rstd::usize(4), [](QStringView error) {
        Q_EMIT App::instance()->errorOccurred(error.toString());
    });

    connect(
        engine, &QQmlApplicationEngine::quit, QGuiApplication::instance(), &QGuiApplication::quit);

    // Resolve ws port. Priority: explicit --ws-port override > DBus-discovered.
    auto* dbus = DaemonDBusClient::instance();
    if (d->m_port == 0 && dbus->status() == DaemonDBusClient::Connected) {
        quint16 p = dbus->wsPort();
        if (p != 0) {
            d->m_backend->setPort(p);
        }
    }

    auto sync_tray_language = [d, dbus]() {
        if (dbus->status() != DaemonDBusClient::Connected ||
            ! dbus->hasDaemonCapability(QStringLiteral("tray-language")))
            return;
        dbus->setTrayLanguage(d->m_ui_language->resolvedLanguage());
    };
    connect(dbus, &DaemonDBusClient::statusChanged, this, sync_tray_language);
    connect(this, &App::resolvedUiLanguageChanged, this, sync_tray_language);
    sync_tray_language();

    // React to daemon availability / port changes. The backend is only
    // (re)configured when the daemon reports `Connected` — VersionMissing
    // and VersionMismatch hold the backend disconnected even if a port
    // value is in hand, since the wire contract isn't trusted.
    auto sync_backend = [d, dbus]() {
        if (d->m_port != 0) {
            // Explicit override from CLI; ignore DBus-driven changes.
            return;
        }
        if (dbus->status() != DaemonDBusClient::Connected) {
            d->m_backend->disconnect();
            return;
        }
        const quint16 port = dbus->wsPort();
        if (port == 0) {
            d->m_backend->disconnect();
            return;
        }
        d->m_backend->setPort(port);
        d->m_backend->connectTo();
    };
    connect(dbus, &DaemonDBusClient::wsPortChanged, this, sync_backend);
    connect(dbus, &DaemonDBusClient::statusChanged, this, sync_backend);

    d->m_display_mgr->attachTo(d->m_backend.get());
    d->m_presentation_mgr->attachTo(d->m_backend.get(), d->m_display_mgr.get());
    d->m_renderer_mgr->attachTo(d->m_backend.get());
    d->m_library_mgr->attachTo(d->m_backend.get());

    // Perform full sync on every connection (initial and reconnect).
    connect(d->m_backend.get(), &Backend::connected, this, [d]() {
        qDebug("ws connected; triggering full status sync");
        // We use queries for the async fetch + manager sync side effects.
        // Queries are parented to the manager so they don't leak.
        auto* dq = new DisplayListQuery(d->m_display_mgr.get());
        dq->reload();
        auto* cq = new CanvasListQuery(d->m_display_mgr.get());
        cq->reload();
        auto* rq = new RendererListQuery(d->m_renderer_mgr.get());
        rq->reload();
        auto* lq = new LibraryListQuery(d->m_library_mgr.get());
        lq->reload();
        auto* gq = new GpuListQuery(d->m_gpu_mgr.get());
        gq->reload();
    });

    // Eagerly construct the daemon-event mirror. Without this Notify
    // would only spring into existence when the first QML consumer
    // accesses it — and would miss the daemon's startup scan event.
    (void)Notify::instance();

    auto* plugin_translations = PluginTranslationStore::instance();
    plugin_translations->setLocale(d->m_ui_language->resolvedLanguage());
    plugin_translations->initialize();

    // Connect to the daemon's WebSocket (no-op if port is still 0).
    d->m_backend->connectTo();

    engine->addImportPath(u"qrc:/"_s);
    // Load the main window from the QML module.
    engine->loadFromModule("waywallen.ui", "Window");

    for (auto el : engine->rootObjects()) {
        if (auto win = qobject_cast<QQuickWindow*>(el)) {
            d->m_main_win = win;
        }
    }

    rstd_assert(d->m_main_win, "main window must exist");
}

void App::registerUiRaiseService() {
    Q_D(App);
    if (d->m_ui_raise_registered) return;
    auto bus = QDBusConnection::sessionBus();
    if (! bus.isConnected()) return;
    auto* raise_bus = new UiRaiseBus(this);
    if (! bus.registerObject(QString::fromLatin1(kUiObjectPath),
                             QString::fromLatin1(kUiInterface),
                             raise_bus,
                             QDBusConnection::ExportAllSlots)) {
        qWarning("failed to register UI Raise on DBus: %s", qPrintable(bus.lastError().message()));
        delete raise_bus;
        return;
    }
    d->m_ui_raise_registered = true;
}

void App::setSingleUiEnabled(bool enabled) {
    auto bus = QDBusConnection::sessionBus();
    if (! bus.isConnected()) return;
    const auto name = QString::fromLatin1(kUiBusName);
    if (! enabled) {
        bus.unregisterService(name);
        return;
    }
    if (! bus.registerService(name)) {
        qWarning("UI single-instance: failed to claim %s: %s",
                 kUiBusName,
                 qPrintable(bus.lastError().message()));
        return;
    }
    registerUiRaiseService();
}

void App::raiseMainWindow(const QString& xdgActivationToken) {
    Q_D(App);
    if (! d->m_main_win) return;

    const bool have_token = ! xdgActivationToken.isEmpty();
    if (have_token) {
        qputenv("XDG_ACTIVATION_TOKEN", xdgActivationToken.toUtf8());
    }

    Qt::WindowStates state = d->m_main_win->windowStates();
    if (state.testFlag(Qt::WindowMinimized)) {
        d->m_main_win->setWindowStates(state & ~Qt::WindowMinimized);
    }

    d->m_main_win->setVisible(true);
    d->m_main_win->raise();

    d->m_main_win->requestActivate();
}

bool claimOrRaiseUiInstance() {
    auto bus = QDBusConnection::sessionBus();
    if (! bus.isConnected()) {
        qWarning("UI single-instance: session bus unavailable; continuing");
        return true;
    }
    if (bus.registerService(QString::fromLatin1(kUiBusName))) {
        return true;
    }
    const QByteArray token = qgetenv("XDG_ACTIVATION_TOKEN");
    if (! token.isEmpty()) {
        qunsetenv("XDG_ACTIVATION_TOKEN");
    }
    QDBusMessage msg = QDBusMessage::createMethodCall(QString::fromLatin1(kUiBusName),
                                                      QString::fromLatin1(kUiObjectPath),
                                                      QString::fromLatin1(kUiInterface),
                                                      QStringLiteral("Raise"));
    msg << QString::fromUtf8(token);
    QDBusMessage reply = bus.call(msg, QDBus::Block, 2000);
    if (reply.type() != QDBusMessage::ReplyMessage) {
        qWarning("UI single-instance: Raise failed: %s", qPrintable(reply.errorMessage()));
    }
    return false;
}

auto App::engine() const -> QQmlApplicationEngine* {
    Q_D(const App);
    return d->m_qml_engine.as_mut_ptr();
}

auto App::backend() const -> Backend* {
    Q_D(const App);
    return d->m_backend.as_mut_ptr();
}

auto App::displayManager() const -> DisplayManager* {
    Q_D(const App);
    return d->m_display_mgr.as_mut_ptr();
}

auto App::rendererManager() const -> RendererManager* {
    Q_D(const App);
    return d->m_renderer_mgr.as_mut_ptr();
}

auto App::libraryManager() const -> LibraryManager* {
    Q_D(const App);
    return d->m_library_mgr.as_mut_ptr();
}

auto App::gpuManager() const -> GpuManager* {
    Q_D(const App);
    return d->m_gpu_mgr.as_mut_ptr();
}

auto App::presentationManager() const -> PresentationManager* {
    Q_D(const App);
    return d->m_presentation_mgr.as_mut_ptr();
}

auto App::networkCacheSize() const -> qint64 {
    Q_D(const App);
    return d->m_network_cache_size;
}

auto App::networkCacheMaximumSize() const -> qint64 {
    Q_D(const App);
    return d->m_qml_network_cache.maximumCacheSize();
}

auto App::uiLanguage() const -> const QString& {
    Q_D(const App);
    return d->m_ui_language->preference();
}

auto App::resolvedUiLanguage() const -> const QString& {
    Q_D(const App);
    return d->m_ui_language->resolvedLanguage();
}

auto App::availableUiLanguages() const -> QVariantList {
    Q_D(const App);
    return d->m_ui_language->availableLanguages();
}

void App::refreshNetworkCacheSize() {
    Q_D(App);
    const auto size = d->m_qml_network_cache.cacheSize();
    if (d->m_network_cache_size == size) return;
    d->m_network_cache_size = size;
    Q_EMIT networkCacheSizeChanged();
}

void App::setNetworkCacheMaximumSize(qint64 size) {
    Q_D(App);
    if (size <= 0 || d->m_qml_network_cache.maximumCacheSize() == size) return;
    d->m_qml_network_cache.setMaximumCacheSize(size);
    Q_EMIT networkCacheMaximumSizeChanged();
    refreshNetworkCacheSize();
}

void App::clearNetworkCache() {
    Q_D(App);
    d->m_qml_network_cache.clear();
    refreshNetworkCacheSize();
}

bool App::setUiLanguage(const QString& language) {
    Q_D(App);
    const auto previous_preference = d->m_ui_language->preference();
    const auto previous_resolved   = d->m_ui_language->resolvedLanguage();
    if (! d->m_ui_language->setLanguage(language)) return false;

    PluginTranslationStore::instance()->setLocale(d->m_ui_language->resolvedLanguage());

    if (previous_preference != d->m_ui_language->preference()) Q_EMIT uiLanguageChanged();
    if (previous_resolved != d->m_ui_language->resolvedLanguage())
        Q_EMIT resolvedUiLanguageChanged();
    return true;
}

bool App::eventFilter(QObject* watched, QEvent* event) {
    if (event->type() == QEvent::LocaleChange) {
        Q_D(App);
        const auto previous_resolved = d->m_ui_language->resolvedLanguage();
        if (d->m_ui_language->preference() == QStringLiteral("system") &&
            d->m_ui_language->refreshSystemLanguage()) {
            PluginTranslationStore::instance()->setLocale(d->m_ui_language->resolvedLanguage());
            Q_EMIT uiLanguageChanged();
            if (previous_resolved != d->m_ui_language->resolvedLanguage())
                Q_EMIT resolvedUiLanguageChanged();
        }
    }
    return QObject::eventFilter(watched, event);
}

void App::load_settings() {}

void App::save_settings() {}

} // namespace waywallen

#include "waywallen/app.moc.cpp"
