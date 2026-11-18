module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/browser_login_query.moc"
#endif

export module waywallen:query.browser_login;
export import :query.query;

namespace waywallen
{

export class BrowserLoginCancelQuery
    : public Query,
      public QueryExtra<control::v1::Response, BrowserLoginCancelQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString sessionId READ sessionId WRITE setSessionId NOTIFY sessionIdChanged FINAL)

public:
    BrowserLoginCancelQuery(QObject* parent = nullptr);
    auto sessionId() const -> const QString&;
    void setSessionId(const QString& value);
    void reload() override;

    Q_SIGNAL void sessionIdChanged();

private:
    QString m_session_id;
};

} // namespace waywallen
