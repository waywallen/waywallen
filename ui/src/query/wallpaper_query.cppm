module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/wallpaper_query.moc"
#endif

export module waywallen:query.wallpaper;
export import :query.query;

namespace waywallen
{

export class WallpaperListQuery : public Query, public QueryExtra<WallpaperListQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString wpType READ wpType WRITE setWpType NOTIFY wpTypeChanged FINAL)
    Q_PROPERTY(QVariantList wallpapers READ wallpapers NOTIFY wallpapersChanged FINAL)

public:
    WallpaperListQuery(QObject* parent = nullptr);

    auto wpType() const -> const QString&;
    void setWpType(const QString&);

    auto wallpapers() const -> const QVariantList&;

    void reload() override;

    Q_SIGNAL void wpTypeChanged();
    Q_SIGNAL void wallpapersChanged();

private:
    QString      m_wp_type;
    QVariantList m_wallpapers;
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

    Q_PROPERTY(QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged FINAL)
    /// Target display ids. Empty list = "apply to all displays" (legacy default).
    Q_PROPERTY(QVariantList displayIds READ displayIds WRITE setDisplayIds NOTIFY displayIdsChanged FINAL)
    Q_PROPERTY(QString rendererId READ rendererId NOTIFY rendererIdChanged FINAL)

public:
    WallpaperApplyQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString&;
    void setWallpaperId(const QString&);

    auto displayIds() const -> const QVariantList&;
    void setDisplayIds(const QVariantList&);

    auto rendererId() const -> const QString&;

    void reload() override;

    Q_SIGNAL void wallpaperIdChanged();
    Q_SIGNAL void displayIdsChanged();
    Q_SIGNAL void rendererIdChanged();

private:
    QString      m_wallpaper_id;
    QVariantList m_display_ids;
    QString      m_renderer_id;
};

} // namespace waywallen
