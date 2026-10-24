module;
#include "waywallen/query/health_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
import :query.health;
import :app;

using namespace qextra::prelude;
namespace proto = waywallen::control::v1;

namespace waywallen
{

HealthQuery::HealthQuery(QObject* parent): Query(parent) {}

auto HealthQuery::service() const -> const QString& { return m_service; }
auto HealthQuery::state() const -> const QString& { return m_state; }
auto HealthQuery::osName() const -> const QString& { return m_os_name; }

void HealthQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setHealth(proto::HealthRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_service = rsp.health().service();
            self->m_state   = rsp.health().state();
            self->m_os_name = rsp.health().osName();
            Q_EMIT self->serviceChanged();
            Q_EMIT self->stateChanged();
            Q_EMIT self->osNameChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/health_query.moc.cpp"
