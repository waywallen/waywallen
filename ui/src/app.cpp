module;
#include "waywallen/app.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :app;

using namespace waywallen;
using namespace Qt::Literals::StringLiterals;

auto app_instance(waywallen::App* in = nullptr) -> waywallen::App* {
    static waywallen::App* instance { in };
    assert(instance != nullptr, "app object not inited");
    assert(in == nullptr || instance == in, "there should be only one app object");
    return instance;
}

class AppPrivate {
public:
    AppPrivate(App* self, quint16 port)
        : m_p(self),
          m_main_win(nullptr),
          m_qml_engine(Box<QQmlApplicationEngine>::make()),
          m_backend(Box<Backend>::make(port)),
          m_display_mgr(Box<DisplayManager>::make()),
          m_renderer_mgr(Box<RendererManager>::make()),
          m_gui_context(Box<QtExecutionContext>::make(
              QThread::currentThread(),
              (QEvent::Type)QEvent::registerEventType())),
          m_pool(4),
          m_port(port) {}
    ~AppPrivate() {
        m_qml_engine.reset();
        save_settings();
    }

    void save_settings() {}

    App*                       m_p;
    QPointer<QQuickWindow>     m_main_win;
    Box<QQmlApplicationEngine> m_qml_engine;
    Box<Backend>               m_backend;
    Box<DisplayManager>        m_display_mgr;
    Box<RendererManager>       m_renderer_mgr;
    Box<QtExecutionContext>    m_gui_context;
    asio::thread_pool          m_pool;
    quint16                    m_port;
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

App::App(quint16 port, rstd::empty)
    : QObject(nullptr), d_ptr(new AppPrivate(this, port)) {
    app_instance(this);
}

App::~App() {
    QAsyncResult::dropEx();
}

void App::init() {
    Q_D(App);
    auto engine = this->engine();

    // Initialize async executors.
    {
        auto qex = QtExecutor(d->m_gui_context.get());
        QAsyncResult::initEx(qex, d->m_pool.get_executor(), [](QStringView error) {
            qWarning("async error: %s", qPrintable(error.toString()));
        });
    }

    connect(engine, &QQmlApplicationEngine::quit, QGuiApplication::instance(), &QGuiApplication::quit);

    // Resolve ws port. Priority: explicit --ws-port override > DBus-discovered.
    auto* dbus = DaemonDBusClient::instance();
    if (d->m_port == 0) {
        quint16 p = dbus->wsPort();
        if (p != 0) {
            d->m_backend->setPort(p);
        }
    }

    // React to daemon availability / port changes.
    connect(dbus, &DaemonDBusClient::wsPortChanged, this, [this, d](quint16 port) {
        if (d->m_port != 0) {
            // Explicit override from CLI; ignore DBus-driven port changes.
            return;
        }
        if (port == 0) {
            d->m_backend->disconnect();
            return;
        }
        d->m_backend->setPort(port);
        d->m_backend->connectTo();
    });
    connect(dbus, &DaemonDBusClient::daemonAvailabilityChanged, this, [d](bool available) {
        if (! available) {
            d->m_backend->disconnect();
        }
    });

    // Hook DisplayManager up to Backend events *before* connectTo so
    // the snapshot the daemon pushes right after the handshake lands.
    d->m_display_mgr->attachTo(d->m_backend.get());
    d->m_renderer_mgr->attachTo(d->m_backend.get());

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

    assert(d->m_main_win, "main window must exist");
}

auto App::engine() const -> QQmlApplicationEngine* {
    Q_D(const App);
    return d->m_qml_engine.as_mut_ptr();
}

auto App::backend() const -> Backend* {
    Q_D(const App);
    return d->m_backend.as_mut_ptr();
}

void App::load_settings() {}

void App::save_settings() {}

} // namespace waywallen

#include "waywallen/app.moc.cpp"
