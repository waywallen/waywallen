module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/model/list_models.moc"
#endif

export module waywallen:model.list_models;
export import :msg.backend_msg;
export import :model.grid;
import qextra;
import rstd.cppstd;

export namespace waywallen::model
{

template<typename TItem, typename CRTP>
using MetaListCRTP = kstore::QMetaListModelCRTP<TItem, CRTP, kstore::ListStoreType::Share,
                                                std::pmr::polymorphic_allocator<TItem>>;

class WallpaperListModel : public GridListModel,
                           public MetaListCRTP<model::Wallpaper, WallpaperListModel> {
    Q_OBJECT
    QML_ANONYMOUS

    using list_crtp_t = MetaListCRTP<model::Wallpaper, WallpaperListModel>;
    using value_type  = model::Wallpaper;

public:
    WallpaperListModel(QObject* parent = nullptr);

    // qtprotobuf marks `QtProtobuf::int64` Q_PROPERTYs as SCRIPTABLE
    // false, so QML can't read `wallpaper.size` directly. Return as
    // qreal (double): safe for any sane file size (< 2^53 bytes).
    Q_INVOKABLE qreal sizeOf(const waywallen::model::Wallpaper& w) const {
        return static_cast<qreal>(static_cast<std::int64_t>(w.size()));
    }
};

} // namespace waywallen::model
