module;
#include "waywallen/msg/store.moc.h"

module waywallen;
import :msg.store;

namespace waywallen
{

namespace
{
auto store_instance() -> AppStore* {
    static AppStore* instance { new AppStore(App::instance()) };
    return instance;
}
} // namespace

AppStore::AppStore(QObject* parent): QObject(parent), wallpapers() {
}

AppStore::~AppStore() {
}

auto AppStore::instance() -> AppStore* { return store_instance(); }

AppStore* AppStore::create(QQmlEngine*, QJSEngine*) {
    auto self = store_instance();
    QJSEngine::setObjectOwnership(self, QJSEngine::ObjectOwnership::CppOwnership);
    return self;
}

QQmlPropertyMap* AppStore::wallpaperExtra(const QString& id) const {
    if (auto extend = wallpapers.query_extend(id); extend) {
        return extend->extra.get();
    }
    return nullptr;
}

} // namespace waywallen

#include "waywallen/msg/store.moc.cpp"
