module;
#include "waywallen/model/list_models.moc.h"

module waywallen;
import qextra;
import :model.list_models;

namespace waywallen::model
{

WallpaperListModel::WallpaperListModel(QObject* parent)
    : GridListModel(this, parent), list_crtp_t() {
    setSelectionEnabled(true);
}

} // namespace waywallen::model

#include "waywallen/model/list_models.moc.cpp"
