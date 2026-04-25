module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/source_query.moc"
#endif

export module waywallen:query.source;
export import :query.query;

namespace waywallen
{

export class SourceListQuery : public Query, public QueryExtra<control::v1::Response, SourceListQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantList sources READ sources NOTIFY sourcesChanged FINAL)

public:
    SourceListQuery(QObject* parent = nullptr);

    auto sources() const -> const QVariantList&;

    void reload() override;

    Q_SIGNAL void sourcesChanged();

private:
    QVariantList m_sources;
};

} // namespace waywallen
