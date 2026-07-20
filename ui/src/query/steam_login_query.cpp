module;
#include "waywallen/query/steam_login_query.moc.h"

module waywallen;
import :query.steam_login;
import :app;

using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

SteamLoginStartQuery::SteamLoginStartQuery(QObject* parent): Query(parent) {}

void SteamLoginStartQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setSteamLoginStart(proto::SteamLoginStartRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [](const proto::Response&) {});
        co_return;
    });
}

SteamLoginCancelQuery::SteamLoginCancelQuery(QObject* parent): Query(parent) {}

void SteamLoginCancelQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setSteamLoginCancel(proto::SteamLoginCancelRequest {});

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

#include "waywallen/query/steam_login_query.moc.cpp"
