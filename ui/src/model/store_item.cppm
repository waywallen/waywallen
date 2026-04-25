module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/model/store_item.moc"
#endif

export module waywallen:model.store_item;
export import :msg.backend_msg;
export import qextra;
import rstd;
import rstd.cppstd;

export namespace waywallen::model
{

template<typename Store, typename CRTP>
class StoreItem : public QObject {
public:
    using store_type      = Store;
    using item_type       = typename store_type::item_type;
    using store_item_type = typename store_type::store_item_type;

    StoreItem(Store store, QObject* parent): QObject(parent), m_item(store) {}
    ~StoreItem() { unreg(); }

    auto item() const -> item_type {
        if (auto it = m_item.item()) {
            return *it;
        }
        return {};
    }
    void setItem(const item_type& v) {
        auto key = kstore::ItemTrait<item_type>::key(v);
        if (key != m_item.key()) {
            m_item = m_item.store().store_insert(v).first;
            m_item.store().store_changed_callback(cppstd::span { &key, 1 },
                                                  m_handle ? *m_handle : 0);
            static_cast<CRTP*>(this)->itemChanged();

            unreg();
            m_handle = rstd::Some(m_item.store().store_reg_notify([this](auto) {
                static_cast<CRTP*>(this)->itemChanged();
            }));
        }
    }
    auto extra() const -> QQmlPropertyMap* {
        if (auto key = m_item.key()) {
            if (auto extend = m_item.store().query_extend(*key); extend) {
                return extend->extra.get();
            }
        }
        return nullptr;
    }

private:
    void unreg() {
        if (m_handle) {
            m_item.store().store_unreg_notify(*m_handle);
        }
    }

    rstd::Option<rstd::i64> m_handle;
    store_item_type         m_item;
};

class WallpaperStoreItem
    : public StoreItem<kstore::ItemTrait<model::Wallpaper>::store_type, WallpaperStoreItem> {
    Q_OBJECT
    QML_NAMED_ELEMENT(WallpaperStoreItem)
    Q_PROPERTY(waywallen::model::Wallpaper item READ item NOTIFY itemChanged)
public:
    using base_type =
        StoreItem<kstore::ItemTrait<model::Wallpaper>::store_type, WallpaperStoreItem>;
    WallpaperStoreItem(QObject* parent = nullptr);
    Q_SIGNAL void itemChanged();
};

} // namespace waywallen::model
