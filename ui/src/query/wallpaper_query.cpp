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

WallpaperListQuery::WallpaperListQuery(QObject* parent)
    : Query(parent), m_model(new model::WallpaperListModel(this)) {
    m_model->set_store(m_model, AppStore::instance()->wallpapers);
    connect_requet_reload(&WallpaperListQuery::wpTypeChanged);
    connect_requet_reload(&WallpaperListQuery::pageSizeChanged);
    connect(m_model, &kstore::QMetaListModel::reqFetchMore, this, [this](qint32) {
        fetchPage(m_request_seq);
    });
}

auto WallpaperListQuery::wpType() const -> const QString& { return m_wp_type; }
void WallpaperListQuery::setWpType(const QString& v) {
    if (m_wp_type != v) {
        m_wp_type = v;
        Q_EMIT wpTypeChanged();
    }
}

auto WallpaperListQuery::model() const -> model::WallpaperListModel* { return m_model; }

auto WallpaperListQuery::pageSize() const -> quint32 { return m_page_size; }
void WallpaperListQuery::setPageSize(quint32 v) {
    if (m_page_size != v && v > 0) {
        m_page_size = v;
        Q_EMIT pageSizeChanged();
    }
}

auto WallpaperListQuery::total() const -> quint32 { return m_total; }

void WallpaperListQuery::reload() {
    ++m_request_seq;
    m_offset = 0;
    if (m_total != 0) {
        m_total = 0;
        Q_EMIT totalChanged();
    }
    m_model->setHasMore(false);
    m_model->resetModel();
    fetchPage(m_request_seq);
}

void WallpaperListQuery::fetchPage(quint32 seq) {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperListRequest {};
    inner.setWpType(m_wp_type);
    inner.setOffset(m_offset);
    inner.setLimit(m_page_size);
    req.setWallpaperList(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), seq]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self, seq](const proto::Response& rsp) {
            if (self->m_request_seq != seq) return; // superseded by newer reload
            auto&                         list_rsp = rsp.wallpaperList();
            std::vector<model::Wallpaper> items;
            items.reserve(list_rsp.wallpapers().size());
            for (const auto& wp : list_rsp.wallpapers()) {
                items.push_back(wp);
            }
            self->m_model->insert(self->m_model->rowCount(), items);

            const quint32 new_total = list_rsp.count();
            if (new_total != self->m_total) {
                self->m_total = new_total;
                Q_EMIT self->totalChanged();
            }
            self->m_offset += static_cast<quint32>(items.size());
            self->m_model->setHasMore(self->m_offset < self->m_total && ! items.empty());
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
