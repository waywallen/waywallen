module;
#include "waywallen/query/tag_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
import :query.tag;
import :app;

using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

TagListQuery::TagListQuery(QObject* parent): Query(parent) {}

auto TagListQuery::tags() const -> const QStringList& { return m_tags; }

void TagListQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setTagList(proto::TagListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            QStringList tags;
            for (const auto& t : rsp.tagList().tags()) {
                tags.append(t);
            }
            self->m_tags = std::move(tags);
            Q_EMIT self->tagsChanged();
        });
        co_return;
    });
}

ContentRatingListQuery::ContentRatingListQuery(QObject* parent): Query(parent) {}

auto ContentRatingListQuery::ratings() const -> const QStringList& { return m_ratings; }

void ContentRatingListQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setContentRatingList(proto::ContentRatingListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            QStringList ratings;
            for (const auto& r : rsp.contentRatingList().ratings()) {
                ratings.append(r);
            }
            self->m_ratings = std::move(ratings);
            Q_EMIT self->ratingsChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/tag_query.moc.cpp"
