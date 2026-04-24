module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/display_query.moc"
#endif

export module waywallen:query.display;
export import :query.query;

namespace waywallen
{

export class DisplayListQuery : public Query, public QueryExtra<DisplayListQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantList displays READ displays NOTIFY displaysChanged FINAL)

public:
    DisplayListQuery(QObject* parent = nullptr);

    auto displays() const -> const QVariantList&;

    void reload() override;

    Q_SIGNAL void displaysChanged();

private:
    QVariantList m_displays;
};

} // namespace waywallen
