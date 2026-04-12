module;
#include "waywallen/query/source_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.source;
import :app;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

SourceListQuery::SourceListQuery(QObject* parent): Query(parent) {}

auto SourceListQuery::sources() const -> const QVariantList& { return m_sources; }

void SourceListQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setSourceList(proto::SourceListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        if (! result) {
            self->setError(result.unwrap_err());
            co_return;
        }
        auto rsp = result.unwrap();
        QVariantList items;
        for (const auto& s : rsp.sourceList().sources()) {
            QVariantMap m;
            m[u"name"_s]    = s.name();
            m[u"version"_s] = s.version();
            QStringList types;
            for (const auto& t : s.types()) {
                types.append(t);
            }
            m[u"types"_s] = types;
            items.append(m);
        }
        self->m_sources = std::move(items);
        Q_EMIT self->sourcesChanged();
        self->setStatus(Status::Finished);
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/source_query.moc.cpp"
