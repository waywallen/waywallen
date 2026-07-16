module;
#include "QExtra/macro_qt.hpp"
#include <QtCore/QStringList>

#ifdef Q_MOC_RUN
#    include "waywallen/query/wallpaper_query.moc"
#endif

export module waywallen:query.wallpaper;
export import :query.query;
export import :model.list_models;

namespace waywallen
{

export class WallpaperListQuery
    : public QueryList,
      public ::QueryExtra<model::WallpaperListModel, WallpaperListQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString wpType READ wpType WRITE setWpType NOTIFY wpTypeChanged FINAL)
    Q_PROPERTY(QList<waywallen::control::v1::WallpaperFilterRule> filters READ filters WRITE
                   setFilters NOTIFY filtersChanged FINAL)
    Q_PROPERTY(QList<waywallen::control::v1::FilterLogic> filterLogics READ filterLogics WRITE
                   setFilterLogics NOTIFY filterLogicsChanged FINAL)
    Q_PROPERTY(QList<waywallen::control::v1::WallpaperSortRule> sorts READ sorts WRITE setSorts
                   NOTIFY sortsChanged FINAL)
    Q_PROPERTY(
        QString searchText READ searchText WRITE setSearchText NOTIFY searchTextChanged FINAL)
    Q_PROPERTY(
        QStringList skipTypes READ skipTypes WRITE setSkipTypes NOTIFY skipTypesChanged FINAL)
    Q_PROPERTY(
        QStringList filterTags READ filterTags WRITE setFilterTags NOTIFY filterTagsChanged FINAL)
    Q_PROPERTY(QStringList skipContentRatings READ skipContentRatings WRITE setSkipContentRatings
                   NOTIFY skipContentRatingsChanged FINAL)
    Q_PROPERTY(bool hasActiveFilters READ hasActiveFilters NOTIFY hasActiveFiltersChanged FINAL)
    Q_PROPERTY(qint32 total READ total NOTIFY totalChanged FINAL)

public:
    WallpaperListQuery(QObject* parent = nullptr);

    auto wpType() const -> const QString&;
    void setWpType(const QString&);

    auto filters() const -> const QList<control::v1::WallpaperFilterRule>&;
    void setFilters(const QList<control::v1::WallpaperFilterRule>&);

    auto             filterLogics() const -> const QList<control::v1::FilterLogic>&;
    void             setFilterLogics(const QList<control::v1::FilterLogic>&);
    Q_INVOKABLE bool replaceFilterState(const QList<control::v1::WallpaperFilterRule>&,
                                        const QList<control::v1::FilterLogic>&);

    auto sorts() const -> const QList<control::v1::WallpaperSortRule>&;
    void setSorts(const QList<control::v1::WallpaperSortRule>&);

    auto searchText() const -> const QString&;
    void setSearchText(const QString&);

    auto skipTypes() const -> const QStringList&;
    void setSkipTypes(const QStringList&);

    auto filterTags() const -> const QStringList&;
    void setFilterTags(const QStringList&);

    auto skipContentRatings() const -> const QStringList&;
    void setSkipContentRatings(const QStringList&);

    auto hasActiveFilters() const -> bool;

    auto total() const -> qint32;

    void reload() override;
    void fetchMore(qint32) override;

    Q_SIGNAL void wpTypeChanged();
    Q_SIGNAL void filterStateChanged();
    Q_SIGNAL void filtersChanged();
    Q_SIGNAL void filterLogicsChanged();
    Q_SIGNAL void sortsChanged();
    Q_SIGNAL void searchTextChanged();
    Q_SIGNAL void skipTypesChanged();
    Q_SIGNAL void filterTagsChanged();
    Q_SIGNAL void skipContentRatingsChanged();
    Q_SIGNAL void hasActiveFiltersChanged();
    Q_SIGNAL void totalChanged();

private:
    QString                                 m_wp_type;
    QList<control::v1::WallpaperFilterRule> m_filters;
    QList<control::v1::FilterLogic>         m_filter_logics;
    QList<control::v1::WallpaperSortRule>   m_sorts;
    QString                                 m_search_text;
    QStringList                             m_skip_types;
    QStringList                             m_filter_tags;
    QStringList                             m_skip_content_ratings;
    qint32                                  m_total { 0 };
};

export class WallpaperScanQuery : public Query,
                                  public QueryExtra<control::v1::Response, WallpaperScanQuery> {
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

export class WallpaperGetQuery : public Query,
                                 public QueryExtra<control::v1::Response, WallpaperGetQuery> {
    Q_OBJECT
    QML_ELEMENT

    /// Source id; setting this triggers a reload. Empty = no-op.
    Q_PROPERTY(
        QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged FINAL)
    /// Latest server-side view (entry + DB media-meta + tags). Read-only.
    Q_PROPERTY(waywallen::model::Wallpaper wallpaper READ wallpaper NOTIFY wallpaperChanged FINAL)

public:
    WallpaperGetQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString&;
    void setWallpaperId(const QString&);

    auto wallpaper() const -> const model::Wallpaper&;

    void reload() override;

    Q_SIGNAL void wallpaperIdChanged();
    Q_SIGNAL void wallpaperChanged();

private:
    QString          m_wallpaper_id;
    model::Wallpaper m_wallpaper;
};

export class WallpaperRemoveQuery : public Query,
                                    public QueryExtra<control::v1::Response, WallpaperRemoveQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(
        QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged FINAL)

public:
    WallpaperRemoveQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString&;
    void setWallpaperId(const QString&);

    void             reload() override;
    Q_INVOKABLE void remove(const QStringList& wallpaperIds);

    Q_SIGNAL void wallpaperIdChanged();
    Q_SIGNAL void removed(const QString& wallpaperId);
    Q_SIGNAL void removedMany(const QStringList& wallpaperIds, quint32 removedCount);

private:
    QString m_wallpaper_id;
};

export class WallpaperPropertySetQuery
    : public Query,
      public QueryExtra<control::v1::Response, WallpaperPropertySetQuery> {
    Q_OBJECT
    QML_ELEMENT

    /// Wallpaper to edit. Empty disables `reload()`.
    Q_PROPERTY(
        QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged FINAL)
    /// Property key (matches a key in the schema, e.g. `u_brightness`).
    Q_PROPERTY(
        QString propertyKey READ propertyKey WRITE setPropertyKey NOTIFY propertyKeyChanged FINAL)
    /// Serialized value: color → "r g b" / "r g b a"; slider → number;
    /// bool → "true"/"false"; string → raw. Empty is a valid string value.
    Q_PROPERTY(QString propertyValue READ propertyValue WRITE setPropertyValue NOTIFY
                   propertyValueChanged FINAL)
    Q_PROPERTY(bool reset READ reset WRITE setReset NOTIFY resetChanged FINAL)

public:
    WallpaperPropertySetQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString&;
    void setWallpaperId(const QString&);

    auto propertyKey() const -> const QString&;
    void setPropertyKey(const QString&);

    auto propertyValue() const -> const QString&;
    void setPropertyValue(const QString&);

    auto reset() const -> bool;
    void setReset(bool);

    void reload() override;

    Q_SIGNAL void wallpaperIdChanged();
    Q_SIGNAL void propertyKeyChanged();
    Q_SIGNAL void propertyValueChanged();
    Q_SIGNAL void resetChanged();

private:
    QString m_wallpaper_id;
    QString m_property_key;
    QString m_property_value;
    bool    m_reset { false };
};

export class WallpaperApplyQuery : public Query,
                                   public QueryExtra<control::v1::Response, WallpaperApplyQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(waywallen::model::Wallpaper wallpaper READ wallpaper WRITE setWallpaper NOTIFY
                   wallpaperChanged FINAL)
    /// Target display ids. Empty list = "apply to all displays" (legacy default).
    Q_PROPERTY(
        QVariantList displayIds READ displayIds WRITE setDisplayIds NOTIFY displayIdsChanged FINAL)
    /// Optional renderer plugin name. Empty (default) lets the daemon pick
    /// the highest-priority renderer for this wallpaper's type.
    Q_PROPERTY(QString rendererName READ rendererName WRITE setRendererName NOTIFY
                   rendererNameChanged FINAL)
    Q_PROPERTY(QString rendererId READ rendererId NOTIFY rendererIdChanged FINAL)

public:
    WallpaperApplyQuery(QObject* parent = nullptr);

    auto wallpaper() const -> const model::Wallpaper&;
    void setWallpaper(const model::Wallpaper&);

    auto displayIds() const -> const QVariantList&;
    void setDisplayIds(const QVariantList&);

    auto rendererName() const -> const QString&;
    void setRendererName(const QString&);

    auto rendererId() const -> const QString&;

    void reload() override;

    Q_SIGNAL void wallpaperChanged();
    Q_SIGNAL void displayIdsChanged();
    Q_SIGNAL void rendererNameChanged();
    Q_SIGNAL void rendererIdChanged();

private:
    model::Wallpaper m_wallpaper;
    QVariantList     m_display_ids;
    QString          m_renderer_name;
    QString          m_renderer_id;
};

/// Apply via `org.freedesktop.portal.Wallpaper` — image-only, no
/// renderer / router / display surface involvement. Independent of
/// `WallpaperApplyQuery`; the daemon enforces wp_type == "image".
export class WallpaperApplyViaPortalQuery
    : public Query,
      public QueryExtra<control::v1::Response, WallpaperApplyViaPortalQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(
        QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged FINAL)
    Q_PROPERTY(QString uri READ uri NOTIFY uriChanged FINAL)

public:
    WallpaperApplyViaPortalQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString&;
    void setWallpaperId(const QString&);

    auto uri() const -> const QString&;

    void reload() override;

    Q_SIGNAL void wallpaperIdChanged();
    Q_SIGNAL void uriChanged();

private:
    QString m_wallpaper_id;
    QString m_uri;
};

} // namespace waywallen
