module;
#include "waywallen/query/browser_login_query.moc.h"

module waywallen;
import qextra;
import :query.browser_login;
import :app;

using namespace qextra::prelude;
namespace proto = waywallen::control::v1;

namespace waywallen
{

BrowserLoginCancelQuery::BrowserLoginCancelQuery(QObject* parent): Query(parent) {}

auto BrowserLoginCancelQuery::sessionId() const -> const QString& { return m_session_id; }

void BrowserLoginCancelQuery::setSessionId(const QString& value) {
    if (m_session_id == value) return;
    m_session_id = value;
    Q_EMIT sessionIdChanged();
}

void BrowserLoginCancelQuery::reload() {
    if (m_session_id.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();
    auto req     = proto::Request {};
    auto inner   = proto::BrowserLoginCancelRequest {};
    inner.setSessionId(m_session_id);
    req.setBrowserLoginCancel(std::move(inner));
    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [](const proto::Response&) {
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/browser_login_query.moc.cpp"
