module;
#include "waywallen/query/audio_output_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
import :query.audio_output;
import :app;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

AudioOutputListQuery::AudioOutputListQuery(QObject* parent): Query(parent) {}

auto AudioOutputListQuery::outputs() const -> const QVariantList& { return m_outputs; }

auto AudioOutputListQuery::available() const -> bool { return m_available; }

void AudioOutputListQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setAudioOutputList(proto::AudioOutputListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            auto& list_rsp = rsp.audioOutputList();

            QVariantList items;
            for (const auto& o : list_rsp.outputs()) {
                QVariantMap m;
                m[u"name"_s]        = o.name();
                m[u"description"_s] = o.description();
                m[u"isDefault"_s]   = o.isDefault();
                items.append(m);
            }
            self->m_outputs   = std::move(items);
            self->m_available = list_rsp.available();
            Q_EMIT self->outputsChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/audio_output_query.moc.cpp"
