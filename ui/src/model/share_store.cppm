module;
#include <QtQml/QQmlPropertyMap>
#include <QtQml/QJSEngine>
#include <memory>

export module waywallen:model.share_store;
export import qextra;
import rstd.cppstd;

export namespace waywallen
{

struct ShareStoreExt {
    using Deleter = void (*)(QQmlPropertyMap*);
    using ptr     = std::unique_ptr<QQmlPropertyMap, Deleter>;
    ShareStoreExt()
        : extra(ptr(QQmlPropertyMap::create(), [](QQmlPropertyMap* p) {
              p->deleteLater();
          })) {
        QJSEngine::setObjectOwnership(extra.get(), QJSEngine::ObjectOwnership::CppOwnership);
    }
    ptr extra;
};

template<typename T>
class ShareStore
    : public kstore::ShareStore<T, cppstd::pmr::polymorphic_allocator<T>, ShareStoreExt> {
public:
    using base_type = kstore::ShareStore<T, cppstd::pmr::polymorphic_allocator<T>, ShareStoreExt>;
    ShareStore(): base_type() {}
};

} // namespace waywallen
