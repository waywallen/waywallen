module;
#include "waywallen/query/autostart_query.moc.h"

module waywallen;
import qextra;
import :query.autostart;
import :app;

using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

AutostartGetQuery::AutostartGetQuery(QObject* parent): Query(parent) {}

bool AutostartGetQuery::enabled() const { return m_enabled; }

void AutostartGetQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setAutostartGet(proto::AutostartGetRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const bool enabled = rsp.autostartGet().enabled();
            if (self->m_enabled == enabled) return;
            self->m_enabled = enabled;
            Q_EMIT self->enabledChanged();
        });
        co_return;
    });
}

AutostartSetQuery::AutostartSetQuery(QObject* parent): Query(parent) {}

bool AutostartSetQuery::enabled() const { return m_enabled; }

void AutostartSetQuery::setEnabled(bool enabled) {
    if (m_enabled == enabled) return;
    m_enabled = enabled;
    Q_EMIT enabledChanged();
}

void AutostartSetQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::AutostartSetRequest {};
    inner.setEnabled(m_enabled);
    req.setAutostartSet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->setEnabled(rsp.autostartSet().enabled());
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/autostart_query.moc.cpp"
