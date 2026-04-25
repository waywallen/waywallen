module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/health_query.moc"
#endif

export module waywallen:query.health;
export import :query.query;

namespace waywallen
{

export class HealthQuery : public Query, public QueryExtra<control::v1::Response, HealthQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString service READ service NOTIFY serviceChanged FINAL)
    Q_PROPERTY(QString state READ state NOTIFY stateChanged FINAL)

public:
    HealthQuery(QObject* parent = nullptr);

    auto service() const -> const QString&;
    auto state() const -> const QString&;

    void reload() override;

    Q_SIGNAL void serviceChanged();
    Q_SIGNAL void stateChanged();

private:
    QString m_service;
    QString m_state;
};

} // namespace waywallen
