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
    Q_PROPERTY(
        int hiddenFilter READ hiddenFilter WRITE setHiddenFilter NOTIFY hiddenFilterChanged FINAL)
    Q_PROPERTY(bool hasActiveFilters READ hasActiveFilters NOTIFY hasActiveFiltersChanged FINAL)
    Q_PROPERTY(bool hasOtherFilters READ hasOtherFilters NOTIFY hasOtherFiltersChanged FINAL)
    Q_PROPERTY(QList<qlonglong> hiddenLibraryIds READ hiddenLibraryIds WRITE setHiddenLibraryIds
                   NOTIFY hiddenLibraryIdsChanged FINAL)
    Q_PROPERTY(qint32 total READ total NOTIFY totalChanged FINAL)
    Q_PROPERTY(QVariant selection READ selection NOTIFY selectionChanged FINAL)

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

    auto hiddenFilter() const -> int;
    void setHiddenFilter(int);

    auto             hasActiveFilters() const -> bool;
    auto             hasOtherFilters() const -> bool;
    auto             hiddenLibraryIds() const -> const QList<qlonglong>&;
    void             setHiddenLibraryIds(const QList<qlonglong>&);
    Q_INVOKABLE void toggleLibraryHidden(qlonglong id);

    auto             total() const -> qint32;
    auto             selection() const -> QVariant;
    Q_INVOKABLE void restoreSelection(const QVariant&);
    Q_INVOKABLE bool matchesSelection(const QVariant&) const;
    Q_SIGNAL void    selectionChanged();

    void             reload() override;
    void             fetchMore(qint32) override;
    Q_INVOKABLE void retry();

    Q_SIGNAL void wpTypeChanged();
    Q_SIGNAL void filterStateChanged();
    Q_SIGNAL void filtersChanged();
    Q_SIGNAL void filterLogicsChanged();
    Q_SIGNAL void sortsChanged();
    Q_SIGNAL void searchTextChanged();
    Q_SIGNAL void skipTypesChanged();
    Q_SIGNAL void filterTagsChanged();
    Q_SIGNAL void skipContentRatingsChanged();
    Q_SIGNAL void hiddenFilterChanged();
    Q_SIGNAL void hasActiveFiltersChanged();
    Q_SIGNAL void hasOtherFiltersChanged();
    Q_SIGNAL void hiddenLibraryIdsChanged();
    Q_SIGNAL void totalChanged();

private:
    auto listRequest() const -> control::v1::WallpaperListRequest;
    void invalidateList();

    QList<qlonglong>                        m_hidden_library_ids;
    bool                                    m_reload_pending { false };
    bool                                    m_failed_append { false };
    QString                                 m_wp_type;
    QList<control::v1::WallpaperFilterRule> m_filters;
    QList<control::v1::FilterLogic>         m_filter_logics;
    QList<control::v1::WallpaperSortRule>   m_sorts;
    QString                                 m_search_text;
    QStringList                             m_skip_types;
    QStringList                             m_filter_tags;
    QStringList                             m_skip_content_ratings;
    int                                     m_hidden_filter { 0 };
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

export class WallpaperUnsubscribeQuery
    : public Query,
      public QueryExtra<control::v1::Response, WallpaperUnsubscribeQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(
        QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged FINAL)

public:
    WallpaperUnsubscribeQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString&;
    void setWallpaperId(const QString&);

    void reload() override;

    Q_SIGNAL void wallpaperIdChanged();
    Q_SIGNAL void unsubscribed(const QString& wallpaperId);

private:
    QString m_wallpaper_id;
};

export class WallpaperHideQuery : public Query,
                                  public QueryExtra<control::v1::Response, WallpaperHideQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(
        QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged FINAL)
    Q_PROPERTY(bool hidden READ hidden WRITE setHidden NOTIFY hiddenChanged FINAL)

public:
    WallpaperHideQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString&;
    void setWallpaperId(const QString&);

    auto hidden() const -> bool;
    void setHidden(bool);

    void             reload() override;
    Q_INVOKABLE void setHiddenForIds(const QStringList& wallpaperIds, bool hidden);

    Q_SIGNAL void wallpaperIdChanged();
    Q_SIGNAL void hiddenChanged();
    Q_SIGNAL void updated(const QStringList& wallpaperIds, bool hidden, quint32 updatedCount);

private:
    QString m_wallpaper_id;
    bool    m_hidden { true };
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

export class WallpaperLayoutSetQuery
    : public Query,
      public QueryExtra<control::v1::Response, WallpaperLayoutSetQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY paramsChanged FINAL)
    Q_PROPERTY(bool clear READ clear WRITE setClear NOTIFY paramsChanged FINAL)
    Q_PROPERTY(int fillmode READ fillmode WRITE setFillmode NOTIFY paramsChanged FINAL)
    Q_PROPERTY(int locationX READ locationX WRITE setLocationX NOTIFY paramsChanged FINAL)
    Q_PROPERTY(int locationY READ locationY WRITE setLocationY NOTIFY paramsChanged FINAL)
    Q_PROPERTY(int rotation READ rotation WRITE setRotation NOTIFY paramsChanged FINAL)
    Q_PROPERTY(int flip READ flip WRITE setFlip NOTIFY paramsChanged FINAL)

public:
    WallpaperLayoutSetQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString& { return m_wallpaper_id; }
    void setWallpaperId(const QString& v);
    auto clear() const -> bool { return m_clear; }
    void setClear(bool v);
    auto fillmode() const -> int { return m_fillmode; }
    void setFillmode(int v);
    auto locationX() const -> int { return m_location_x; }
    void setLocationX(int v);
    auto locationY() const -> int { return m_location_y; }
    void setLocationY(int v);
    auto rotation() const -> int { return m_rotation; }
    void setRotation(int v);
    auto flip() const -> int { return m_flip; }
    void setFlip(int v);

    void reload() override;

    Q_SIGNAL void paramsChanged();

private:
    QString m_wallpaper_id;
    bool    m_clear { false };
    int     m_fillmode { 3 };
    int     m_location_x { 50 };
    int     m_location_y { 50 };
    int     m_rotation { 1 };
    int     m_flip { 1 };
};

export class WallpaperApplyQuery : public Query,
                                   public QueryExtra<control::v1::Response, WallpaperApplyQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(waywallen::model::Wallpaper wallpaper READ wallpaper WRITE setWallpaper NOTIFY
                   wallpaperChanged FINAL)
    /// Typed targets. Each map contains either `displayId` or `canvasId`.
    /// Empty means every selectable target.
    Q_PROPERTY(QVariantList targets READ targets WRITE setTargets NOTIFY targetsChanged FINAL)
    /// Optional renderer plugin name. Empty (default) lets the daemon pick
    /// the highest-priority renderer for this wallpaper's type.
    Q_PROPERTY(QString rendererName READ rendererName WRITE setRendererName NOTIFY
                   rendererNameChanged FINAL)
    Q_PROPERTY(QString rendererId READ rendererId NOTIFY rendererIdChanged FINAL)
    Q_PROPERTY(
        QVariantList stoppedPlaylists READ stoppedPlaylists NOTIFY stoppedPlaylistsChanged FINAL)

public:
    WallpaperApplyQuery(QObject* parent = nullptr);

    auto wallpaper() const -> const model::Wallpaper&;
    void setWallpaper(const model::Wallpaper&);

    auto targets() const -> const QVariantList&;
    void setTargets(const QVariantList&);

    auto rendererName() const -> const QString&;
    void setRendererName(const QString&);

    auto rendererId() const -> const QString&;
    auto stoppedPlaylists() const -> const QVariantList&;

    void reload() override;

    Q_SIGNAL void wallpaperChanged();
    Q_SIGNAL void targetsChanged();
    Q_SIGNAL void rendererNameChanged();
    Q_SIGNAL void rendererIdChanged();
    Q_SIGNAL void stoppedPlaylistsChanged();

private:
    model::Wallpaper m_wallpaper;
    QVariantList     m_targets;
    QString          m_renderer_name;
    QString          m_renderer_id;
    QVariantList     m_stopped_playlists;
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
