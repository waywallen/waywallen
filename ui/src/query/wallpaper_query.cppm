module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/wallpaper_query.moc"
#endif

export module waywallen:query.wallpaper;
export import :query.query;
export import :model.list_models;

namespace waywallen
{

export class WallpaperListQuery : public Query, public QueryExtra<WallpaperListQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString wpType READ wpType WRITE setWpType NOTIFY wpTypeChanged FINAL)
    Q_PROPERTY(waywallen::model::WallpaperListModel* model READ model CONSTANT FINAL)
    Q_PROPERTY(quint32 pageSize READ pageSize WRITE setPageSize NOTIFY pageSizeChanged FINAL)
    Q_PROPERTY(quint32 total READ total NOTIFY totalChanged FINAL)

public:
    WallpaperListQuery(QObject* parent = nullptr);

    auto wpType() const -> const QString&;
    void setWpType(const QString&);

    auto model() const -> model::WallpaperListModel*;

    auto pageSize() const -> quint32;
    void setPageSize(quint32);

    auto total() const -> quint32;

    void reload() override;

    Q_SIGNAL void wpTypeChanged();
    Q_SIGNAL void pageSizeChanged();
    Q_SIGNAL void totalChanged();

private:
    void fetchPage(quint32 seq);

    QString                    m_wp_type;
    model::WallpaperListModel* m_model;
    quint32                    m_page_size    { 60 };
    quint32                    m_offset       { 0 };
    quint32                    m_total        { 0 };
    quint32                    m_request_seq  { 0 };
};

export class WallpaperScanQuery : public Query, public QueryExtra<WallpaperScanQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(quint32 count READ count NOTIFY countChanged FINAL)

public:
    WallpaperScanQuery(QObject* parent = nullptr);

    auto count() const -> quint32;

    void reload() override;

    Q_SIGNAL void countChanged();

private:
    quint32 m_count { 0 };
};

export class WallpaperApplyQuery : public Query, public QueryExtra<WallpaperApplyQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(waywallen::model::Wallpaper wallpaper READ wallpaper WRITE setWallpaper NOTIFY wallpaperChanged FINAL)
    /// Target display ids. Empty list = "apply to all displays" (legacy default).
    Q_PROPERTY(QVariantList displayIds READ displayIds WRITE setDisplayIds NOTIFY displayIdsChanged FINAL)
    Q_PROPERTY(QString rendererId READ rendererId NOTIFY rendererIdChanged FINAL)

public:
    WallpaperApplyQuery(QObject* parent = nullptr);

    auto wallpaper() const -> const model::Wallpaper&;
    void setWallpaper(const model::Wallpaper&);

    auto displayIds() const -> const QVariantList&;
    void setDisplayIds(const QVariantList&);

    auto rendererId() const -> const QString&;

    void reload() override;

    Q_SIGNAL void wallpaperChanged();
    Q_SIGNAL void displayIdsChanged();
    Q_SIGNAL void rendererIdChanged();

private:
    model::Wallpaper m_wallpaper;
    QVariantList     m_display_ids;
    QString          m_renderer_id;
};

} // namespace waywallen
