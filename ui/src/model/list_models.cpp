module;
#include "waywallen/model/list_models.moc.h"

module waywallen;
import :model.list_models;
import :msg.store;

namespace waywallen::model
{

WallpaperListModel::WallpaperListModel(QObject* parent)
    : kstore::QGadgetListModel(this, parent), list_crtp_t() {}

QQmlPropertyMap* WallpaperListModel::extra(qint32 idx) const {
    if (idx < 0 || static_cast<std::size_t>(idx) >= this->size()) return nullptr;
    if (auto extend = AppStore::instance()->wallpapers.query_extend(this->key_at(idx)); extend) {
        return extend->extra.get();
    }
    return nullptr;
}

} // namespace waywallen::model

#include "waywallen/model/list_models.moc.cpp"
