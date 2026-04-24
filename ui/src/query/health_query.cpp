module;
#include "waywallen/query/health_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.health;
import :app;

using namespace qextra::prelude; 
namespace proto = waywallen::control::v1;

namespace waywallen
{

HealthQuery::HealthQuery(QObject* parent): Query(parent) {}

auto HealthQuery::service() const -> const QString& { return m_service; }
auto HealthQuery::state() const -> const QString& { return m_state; }

void HealthQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setHealth(proto::HealthRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_service = rsp.health().service();
            self->m_state   = rsp.health().state();
            Q_EMIT self->serviceChanged();
            Q_EMIT self->stateChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/health_query.moc.cpp"
