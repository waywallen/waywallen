module;
#include "waywallen/query/plugin_action_query.moc.h"

module waywallen;
import :query.plugin_action;
import :app;

using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

PluginActionQuery::PluginActionQuery(QObject* parent): Query(parent) {}

QString PluginActionQuery::pluginId() const { return m_plugin_id; }
void    PluginActionQuery::setPluginId(const QString& v) {
    if (m_plugin_id == v) return;
    m_plugin_id = v;
    Q_EMIT pluginIdChanged();
}

QString PluginActionQuery::actionId() const { return m_action_id; }
void    PluginActionQuery::setActionId(const QString& v) {
    if (m_action_id == v) return;
    m_action_id = v;
    Q_EMIT actionIdChanged();
}

void PluginActionQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::PluginActionRequest {};
    inner.setPluginId(m_plugin_id);
    inner.setActionId(m_action_id);
    req.setPluginAction(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [](const proto::Response&) {});
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/plugin_action_query.moc.cpp"
