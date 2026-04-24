module;
#include "waywallen/query/display_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.display;
import :app;
import :display;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

DisplayListQuery::DisplayListQuery(QObject* parent): Query(parent) {}

auto DisplayListQuery::displays() const -> const QVariantList& { return m_displays; }

void DisplayListQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setDisplayList(proto::DisplayListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self](const proto::Response& rsp) {
            auto& list_rsp = rsp.displayList();

            // Sync the global DisplayManager first so any consumer pulling
            // from the manager sees the freshly-fetched rows before this
            // query's own `displaysChanged` fires.
            if (auto* dm = DisplayManager::instance()) {
                dm->replaceAll(list_rsp.displays());
            }

            QVariantList items;
            for (const auto& d : list_rsp.displays()) {
                QVariantMap m;
                m[u"id"_s]         = QVariant::fromValue<quint64>(d.displayId());
                m[u"name"_s]       = d.name();
                m[u"width"_s]      = d.width();
                m[u"height"_s]     = d.height();
                m[u"refreshMhz"_s] = d.refreshMhz();

                QVariantList links;
                for (const auto& l : d.links()) {
                    QVariantMap lm;
                    lm[u"rendererId"_s] = l.rendererId();
                    lm[u"zOrder"_s]     = static_cast<int>(l.zOrder());
                    links.append(lm);
                }
                m[u"links"_s] = links;
                items.append(m);
            }
            self->m_displays = std::move(items);
            Q_EMIT self->displaysChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/display_query.moc.cpp"
