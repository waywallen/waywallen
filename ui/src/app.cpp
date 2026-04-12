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
    AppPrivate(App* self)
        : m_p(self), m_main_win(nullptr), m_qml_engine(Box<QQmlApplicationEngine>::make()) {}
    ~AppPrivate() {
        m_qml_engine.reset();

        save_settings();
    }

    void save_settings() {}

    App*                       m_p;
    QPointer<QQuickWindow>     m_main_win;
    Box<QQmlApplicationEngine> m_qml_engine;
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

App::App(qint16 port, rstd::empty): QObject(nullptr) {}

App::~App() {}

void App::init() {}

void App::load_settings() {
}

void App::save_settings() {
}

} // namespace waywallen

#include "waywallen/app.moc.cpp"