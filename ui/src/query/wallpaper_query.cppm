module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/wallpaper_query.moc"
#endif

export module waywallen:query.wallpaper;
export import :query.query;

namespace waywallen
{

export class WallpaperListQuery : public Query {
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

export class WallpaperScanQuery : public Query {
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

export class WallpaperApplyQuery : public Query {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged FINAL)
    Q_PROPERTY(quint32 width READ width WRITE setWidth NOTIFY widthChanged FINAL)
    Q_PROPERTY(quint32 height READ height WRITE setHeight NOTIFY heightChanged FINAL)
    Q_PROPERTY(quint32 fps READ fps WRITE setFps NOTIFY fpsChanged FINAL)
    Q_PROPERTY(QString rendererId READ rendererId NOTIFY rendererIdChanged FINAL)

public:
    WallpaperApplyQuery(QObject* parent = nullptr);

    auto wallpaperId() const -> const QString&;
    void setWallpaperId(const QString&);

    auto width() const -> quint32;
    void setWidth(quint32);

    auto height() const -> quint32;
    void setHeight(quint32);

    auto fps() const -> quint32;
    void setFps(quint32);

    auto rendererId() const -> const QString&;

    void reload() override;

    Q_SIGNAL void wallpaperIdChanged();
    Q_SIGNAL void widthChanged();
    Q_SIGNAL void heightChanged();
    Q_SIGNAL void fpsChanged();
    Q_SIGNAL void rendererIdChanged();

private:
    QString m_wallpaper_id;
    quint32 m_width { 1920 };
    quint32 m_height { 1080 };
    quint32 m_fps { 30 };
    QString m_renderer_id;
};

} // namespace waywallen
