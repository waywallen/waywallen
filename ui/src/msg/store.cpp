module;
#include "waywallen/msg/store.moc.h"

module waywallen;
import :msg.store;

namespace waywallen
{

namespace
{
auto store_instance(AppStore* in = nullptr) -> AppStore* {
    static AppStore* instance { in };
    if (in != nullptr) instance = in;
    return instance;
}
} // namespace

AppStore::AppStore(QObject* parent): QObject(parent), wallpapers() {
    store_instance(this);
}

AppStore::~AppStore() {
    if (store_instance() == this) {
        store_instance(nullptr);
    }
}

auto AppStore::instance() -> AppStore* { return store_instance(); }

AppStore* AppStore::create(QQmlEngine*, QJSEngine*) {
    auto self = store_instance();
    if (self == nullptr) {
        self = new AppStore();
    }
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
