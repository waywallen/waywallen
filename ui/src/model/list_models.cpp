module;
#include "waywallen/model/list_models.moc.h"

module waywallen;
import :model.list_models;

namespace waywallen::model
{

WallpaperListModel::WallpaperListModel(QObject* parent)
    : kstore::QGadgetListModel(this, parent), list_crtp_t() {}

} // namespace waywallen::model

#include "waywallen/model/list_models.moc.cpp"
