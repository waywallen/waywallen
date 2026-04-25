module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/model/list_models.moc"
#endif

export module waywallen:model.list_models;
export import :msg.backend_msg;
export import qextra;
import rstd.cppstd;

export namespace waywallen::model
{

template<typename TItem, typename CRTP>
using MetaListCRTP = kstore::QMetaListModelCRTP<TItem, CRTP, kstore::ListStoreType::Share,
                                                cppstd::pmr::polymorphic_allocator<TItem>>;

class WallpaperListModel : public kstore::QGadgetListModel,
                           public MetaListCRTP<model::Wallpaper, WallpaperListModel> {
    Q_OBJECT
    QML_ANONYMOUS

    using list_crtp_t = MetaListCRTP<model::Wallpaper, WallpaperListModel>;
    using value_type  = model::Wallpaper;

public:
    WallpaperListModel(QObject* parent = nullptr);

    Q_INVOKABLE QQmlPropertyMap* extra(qint32 idx) const;
};

} // namespace waywallen::model
