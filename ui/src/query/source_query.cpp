module;
#include "waywallen/query/source_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
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
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setSourceList(proto::SourceListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            QVariantList items;
            for (const auto& s : rsp.sourceList().sources()) {
                QVariantMap m;
                m[u"name"_s]         = s.name();
                m[u"version"_s]      = s.version();
                m[u"libraryLabel"_s] = s.libraryLabel();
                m[u"libraryHint"_s]  = s.libraryHint();
                m[u"libraryLabelText"_s] =
                    pluginMessageFromPb(s.libraryLabelText(), s.libraryLabel());
                m[u"libraryHintText"_s] = pluginMessageFromPb(s.libraryHintText(), s.libraryHint());
                QStringList types;
                for (const auto& t : s.types()) {
                    types.append(t);
                }
                m[u"types"_s]    = types;
                m[u"pluginId"_s] = s.pluginId();

                QVariantList settings;
                for (const auto& ss : s.settings()) {
                    QVariantMap sm;
                    sm[u"key"_s]             = ss.key();
                    sm[u"type"_s]            = static_cast<int>(ss.type());
                    sm[u"default_value"_s]   = ss.defaultValue();
                    sm[u"identity"_s]        = ss.identity();
                    sm[u"label_key"_s]       = ss.labelKey();
                    sm[u"description_key"_s] = ss.descriptionKey();
                    sm[u"label"_s]           = pluginMessageFromPb(ss.label(), ss.labelKey());
                    sm[u"description"_s] =
                        pluginMessageFromPb(ss.description(), ss.descriptionKey());
                    sm[u"group_label"_s] = pluginMessageFromPb(ss.groupLabel(), ss.group());
                    sm[u"min"_s]         = ss.min();
                    sm[u"max"_s]         = ss.max();
                    sm[u"step"_s]        = ss.step();
                    QStringList choices;
                    for (const auto& c : ss.choices()) {
                        choices.append(c);
                    }
                    sm[u"choices"_s] = choices;
                    sm[u"group"_s]   = ss.group();
                    sm[u"order"_s]   = static_cast<int>(ss.order());
                    settings.append(sm);
                }
                m[u"settings"_s] = settings;
                items.append(m);
            }
            self->m_sources = std::move(items);
            Q_EMIT self->sourcesChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/source_query.moc.cpp"
