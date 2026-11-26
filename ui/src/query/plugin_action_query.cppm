module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/plugin_action_query.moc"
#endif

export module waywallen:query.plugin_action;
export import :query.query;

namespace waywallen
{

export class PluginActionQuery : public Query,
                                 public QueryExtra<control::v1::Response, PluginActionQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString pluginId READ pluginId WRITE setPluginId NOTIFY pluginIdChanged FINAL)
    Q_PROPERTY(QString actionId READ actionId WRITE setActionId NOTIFY actionIdChanged FINAL)
    Q_PROPERTY(QString browserHelperId READ browserHelperId WRITE setBrowserHelperId NOTIFY
                   browserHelperIdChanged FINAL)

public:
    PluginActionQuery(QObject* parent = nullptr);

    QString          pluginId() const;
    void             setPluginId(const QString& v);
    QString          actionId() const;
    void             setActionId(const QString& v);
    QString          browserHelperId() const;
    void             setBrowserHelperId(const QString& v);
    void             reload() override;
    Q_INVOKABLE void invoke(const QVariantMap& values);

    Q_SIGNAL void pluginIdChanged();
    Q_SIGNAL void actionIdChanged();
    Q_SIGNAL void browserHelperIdChanged();
    Q_SIGNAL void completed(bool accepted, const QString& error, const QString& sessionId);

private:
    QString m_plugin_id;
    QString m_action_id;
    QString m_browser_helper_id;
};

} // namespace waywallen
