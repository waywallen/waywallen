module;
#include "control.qpb.h"
#include <QString>

export module waywallen:msg.backend_msg;
export import :proto;
export import :model.share_store;
import rstd.cppstd;

export namespace waywallen::model
{
using Wallpaper = waywallen::control::v1::WallpaperEntry;
} // namespace waywallen::model

template<>
struct kstore::ItemTrait<waywallen::model::Wallpaper> {
    using Self       = waywallen::model::Wallpaper;
    using key_type   = QString;
    using store_type = waywallen::ShareStore<Self>;
    static auto key(const Self& el) noexcept -> QString { return el.id_proto(); }
};
