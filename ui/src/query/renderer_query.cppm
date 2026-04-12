module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/renderer_query.moc"
#endif

export module waywallen:query.renderer;
export import :query.query;

namespace waywallen
{

export class RendererListQuery : public Query {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QStringList renderers READ renderers NOTIFY renderersChanged FINAL)

public:
    RendererListQuery(QObject* parent = nullptr);

    auto renderers() const -> const QStringList&;

    void reload() override;

    Q_SIGNAL void renderersChanged();

private:
    QStringList m_renderers;
};

export class RendererPluginListQuery : public Query {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantList renderers READ renderers NOTIFY renderersChanged FINAL)
    Q_PROPERTY(QStringList supportedTypes READ supportedTypes NOTIFY supportedTypesChanged FINAL)

public:
    RendererPluginListQuery(QObject* parent = nullptr);

    auto renderers() const -> const QVariantList&;
    auto supportedTypes() const -> const QStringList&;

    void reload() override;

    Q_SIGNAL void renderersChanged();
    Q_SIGNAL void supportedTypesChanged();

private:
    QVariantList m_renderers;
    QStringList  m_supported_types;
};

export class RendererKillQuery : public Query {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString rendererId READ rendererId WRITE setRendererId NOTIFY rendererIdChanged FINAL)

public:
    RendererKillQuery(QObject* parent = nullptr);

    auto rendererId() const -> const QString&;
    void setRendererId(const QString&);

    void reload() override;

    Q_SIGNAL void rendererIdChanged();

private:
    QString m_renderer_id;
};

} // namespace waywallen
