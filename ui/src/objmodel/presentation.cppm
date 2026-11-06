module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/objmodel/presentation.moc"
#endif

export module waywallen:presentation;
export import :backend;
export import :display;
export import :msg.store;
export import :proto;
import qextra;
import rstd;
import rstd.cppstd;

namespace proto = waywallen::control::v1;

export namespace waywallen::model
{

struct NowPlayingItem {
    Q_GADGET
    Q_PROPERTY(QString wallpaperId MEMBER wallpaperId)
    Q_PROPERTY(waywallen::model::Wallpaper wallpaper MEMBER wallpaper)
    Q_PROPERTY(QString targetSummary MEMBER targetSummary)
    Q_PROPERTY(int state MEMBER state)

public:
    QString   wallpaperId;
    Wallpaper wallpaper;
    QString   targetSummary;
    int       state { 0 };
    bool      operator==(const NowPlayingItem&) const = default;
};

} // namespace waywallen::model

template<>
struct kstore::ItemTrait<waywallen::model::NowPlayingItem> {
    using Self       = waywallen::model::NowPlayingItem;
    using key_type   = QString;
    using store_type = waywallen::ShareStore<Self>;

    static auto key(const Self& item) -> QString { return item.wallpaperId; }
};

export namespace waywallen
{

class NowPlayingModel : public kstore::QGadgetListModel,
                        public kstore::QMetaListModelCRTP<model::NowPlayingItem, NowPlayingModel,
                                                          kstore::ListStoreType::Share> {
    Q_OBJECT
    QML_ANONYMOUS
    Q_PROPERTY(int count READ count NOTIFY countChanged FINAL)

    using list_crtp_t = kstore::QMetaListModelCRTP<model::NowPlayingItem, NowPlayingModel,
                                                   kstore::ListStoreType::Share>;

public:
    explicit NowPlayingModel(QObject* parent = nullptr);

    auto count() const -> int { return static_cast<int>(size()); }
    void replace(QList<model::NowPlayingItem> items);

    Q_SIGNAL void countChanged();
};

class PresentationManager : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("PresentationManager is owned by App")
    Q_PROPERTY(NowPlayingModel* model READ model CONSTANT FINAL)
    Q_PROPERTY(int count READ count NOTIFY countChanged FINAL)

public:
    explicit PresentationManager(QObject* parent = nullptr);

    auto model() -> NowPlayingModel* { return &m_model; }
    auto count() const -> int { return m_model.count(); }
    void attachTo(Backend* backend, DisplayManager* displays);

    Q_SIGNAL void countChanged();

private:
    void handleEvent(const proto::Event& event);
    void replaceSnapshot(const proto::WallpaperPresentationSnapshot& snapshot);
    void rebuildRows();
    void requestMissingWallpapers();
    auto targetSummary(const proto::WallpaperPresentationInfo& presentation) const -> QString;

    Backend*                                          m_backend { nullptr };
    DisplayManager*                                   m_displays { nullptr };
    QList<proto::WallpaperPresentationInfo>           m_presentations;
    QList<AppStore::wallpaper_store::store_item_type> m_wallpapers;
    ShareStore<model::NowPlayingItem>                 m_store;
    NowPlayingModel                                   m_model;
    QAsyncScope                                       m_requests;
    quint64                                           m_request_generation { 0 };
};

} // namespace waywallen
