module;
#include "waywallen/query/log_query.moc.h"

module waywallen;
import qextra;
import :query.log;
import :app;

using namespace qextra::prelude;
namespace proto = waywallen::control::v1;

namespace waywallen
{

DaemonLogQuery::DaemonLogQuery(QObject* parent): Query(parent) {}

auto DaemonLogQuery::path() const -> const QString& { return m_path; }
auto DaemonLogQuery::content() const -> const QString& { return m_content; }
auto DaemonLogQuery::truncated() const -> bool { return m_truncated; }

void DaemonLogQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setLogRead(proto::LogReadRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_path      = rsp.logRead().path();
            self->m_content   = rsp.logRead().content();
            self->m_truncated = rsp.logRead().truncated();
            Q_EMIT self->pathChanged();
            Q_EMIT self->contentChanged();
            Q_EMIT self->truncatedChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/log_query.moc.cpp"
