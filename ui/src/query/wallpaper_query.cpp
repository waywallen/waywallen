module;
#include "waywallen/query/wallpaper_query.moc.h"
#include <qtprotobuftypes.h>
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.wallpaper;
import :app;
import :msg.store;

using namespace Qt::Literals::StringLiterals;

namespace proto = waywallen::control::v1;
using namespace qextra::prelude;

namespace waywallen
{

// ---------------------------------------------------------------------------
// WallpaperListQuery
// ---------------------------------------------------------------------------

WallpaperListQuery::WallpaperListQuery(QObject* parent): QueryList(parent) {
    setLimit(60);
    tdata()->set_store(tdata(), AppStore::instance()->wallpapers);
    connect_requet_reload(&WallpaperListQuery::wpTypeChanged, this);
}

auto WallpaperListQuery::wpType() const -> const QString& { return m_wp_type; }
void WallpaperListQuery::setWpType(const QString& v) {
    if (m_wp_type != v) {
        m_wp_type = v;
        setOffset(0);
        Q_EMIT wpTypeChanged();
    }
}

auto WallpaperListQuery::total() const -> qint32 { return m_total; }

void WallpaperListQuery::reload() {
    setOffset(0);
    setNoMore(false);
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperListRequest {};
    inner.setWpType(m_wp_type);
    initReqForReload(inner);
    req.setWallpaperList(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto&                   list_rsp = rsp.wallpaperList();
            std::vector<model::Wallpaper> items;
            items.reserve(list_rsp.wallpapers().size());
            for (const auto& wp : list_rsp.wallpapers()) {
                items.push_back(wp);
            }
            auto t = self->tdata();
            t->setHasMore(false);
            t->sync(items);

            const qint32 new_total = static_cast<qint32>(list_rsp.count());
            if (new_total != self->m_total) {
                self->m_total = new_total;
                Q_EMIT self->totalChanged();
            }
            const bool more = t->rowCount() < new_total && ! items.empty();
            self->setNoMore(! more);
            t->setHasMore(more);
        });
        co_return;
    });
}

void WallpaperListQuery::fetchMore(qint32) {
    if (noMore()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperListRequest {};
    inner.setWpType(m_wp_type);
    initReqForFetchMore(inner);
    req.setWallpaperList(std::move(inner));

    const qint32 next_offset = offset() + 1;
    auto         self        = QWatcher { this };
    spawn([self, backend, req = std::move(req), next_offset]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self, next_offset](const proto::Response& rsp) {
            const auto&                   list_rsp = rsp.wallpaperList();
            std::vector<model::Wallpaper> items;
            items.reserve(list_rsp.wallpapers().size());
            for (const auto& wp : list_rsp.wallpapers()) {
                items.push_back(wp);
            }
            auto t = self->tdata();
            t->insert(t->rowCount(), items);
            self->setOffset(next_offset);

            const qint32 new_total = static_cast<qint32>(list_rsp.count());
            if (new_total != self->m_total) {
                self->m_total = new_total;
                Q_EMIT self->totalChanged();
            }
            const bool more = t->rowCount() < new_total && ! items.empty();
            self->setNoMore(! more);
            t->setHasMore(more);
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperScanQuery
// ---------------------------------------------------------------------------

WallpaperScanQuery::WallpaperScanQuery(QObject* parent): Query(parent) {}

auto WallpaperScanQuery::count() const -> quint32 { return m_count; }

void WallpaperScanQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setWallpaperScan(proto::WallpaperScanRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_count = rsp.wallpaperScan().count();
            Q_EMIT self->countChanged();
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperApplyQuery
// ---------------------------------------------------------------------------

WallpaperApplyQuery::WallpaperApplyQuery(QObject* parent): Query(parent) {}

auto WallpaperApplyQuery::wallpaper() const -> const model::Wallpaper& { return m_wallpaper; }
void WallpaperApplyQuery::setWallpaper(const model::Wallpaper& v) {
    if (m_wallpaper.id_proto() != v.id_proto()) {
        m_wallpaper = v;
        Q_EMIT wallpaperChanged();
    }
}

auto WallpaperApplyQuery::displayIds() const -> const QVariantList& { return m_display_ids; }
void WallpaperApplyQuery::setDisplayIds(const QVariantList& v) {
    if (m_display_ids != v) {
        m_display_ids = v;
        Q_EMIT displayIdsChanged();
    }
}

auto WallpaperApplyQuery::rendererId() const -> const QString& { return m_renderer_id; }

void WallpaperApplyQuery::reload() {
    if (m_wallpaper.id_proto().isEmpty()) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperApplyRequest {};
    inner.setWallpaperId(m_wallpaper.id_proto());
    // Empty list is a legitimate value: daemon treats it as "apply to
    // all displays". Non-empty restricts the relink to named ids.
    QtProtobuf::uint64List ids;
    ids.reserve(m_display_ids.size());
    for (const auto& v : m_display_ids) {
        bool ok = false;
        auto id = v.toULongLong(&ok);
        if (ok) ids.append(id);
    }
    inner.setDisplayIds(ids);
    req.setWallpaperApply(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_renderer_id = rsp.wallpaperApply().rendererId();
            Q_EMIT self->rendererIdChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/wallpaper_query.moc.cpp"
