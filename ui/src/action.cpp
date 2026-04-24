module;
#include "waywallen/action.moc.h"

module waywallen;
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

} // namespace waywallen

#include "waywallen/action.moc.cpp"
