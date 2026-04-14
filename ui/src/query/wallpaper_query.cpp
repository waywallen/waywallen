module;
#include "waywallen/query/wallpaper_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.wallpaper;
import :app;

using namespace Qt::Literals::StringLiterals;

namespace proto = waywallen::control::v1;
using namespace qextra::prelude;

namespace waywallen
{

// ---------------------------------------------------------------------------
// WallpaperListQuery
// ---------------------------------------------------------------------------

WallpaperListQuery::WallpaperListQuery(QObject* parent): Query(parent) {
    connect_requet_reload(&WallpaperListQuery::wpTypeChanged);
}

auto WallpaperListQuery::wpType() const -> const QString& { return m_wp_type; }
void WallpaperListQuery::setWpType(const QString& v) {
    if (m_wp_type != v) {
        m_wp_type = v;
        Q_EMIT wpTypeChanged();
    }
}

auto WallpaperListQuery::wallpapers() const -> const QVariantList& { return m_wallpapers; }

void WallpaperListQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperListRequest {};
    inner.setWpType(m_wp_type);
    req.setWallpaperList(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        if (! result) {
            self->setError(result.unwrap_err());
            co_return;
        }
        auto rsp = result.unwrap();
        auto& list_rsp = rsp.wallpaperList();
        QVariantList items;
        for (const auto& wp : list_rsp.wallpapers()) {
            QVariantMap m;
            m[u"id"_s]       = wp.id_proto();
            m[u"name"_s]     = wp.name();
            m[u"wpType"_s]   = wp.wpType();
            m[u"resource"_s] = wp.resource();
            m[u"preview"_s]  = wp.preview();
            items.append(m);
        }
        self->m_wallpapers = std::move(items);
        Q_EMIT self->wallpapersChanged();
        self->setStatus(Status::Finished);
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

        if (! result) {
            self->setError(result.unwrap_err());
            co_return;
        }
        auto rsp = result.unwrap();
        self->m_count = rsp.wallpaperScan().count();
        Q_EMIT self->countChanged();
        self->setStatus(Status::Finished);
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperApplyQuery
// ---------------------------------------------------------------------------

WallpaperApplyQuery::WallpaperApplyQuery(QObject* parent): Query(parent) {}

auto WallpaperApplyQuery::wallpaperId() const -> const QString& { return m_wallpaper_id; }
void WallpaperApplyQuery::setWallpaperId(const QString& v) {
    if (m_wallpaper_id != v) {
        m_wallpaper_id = v;
        Q_EMIT wallpaperIdChanged();
    }
}

auto WallpaperApplyQuery::width() const -> quint32 { return m_width; }
void WallpaperApplyQuery::setWidth(quint32 v) {
    if (m_width != v) {
        m_width = v;
        Q_EMIT widthChanged();
    }
}

auto WallpaperApplyQuery::height() const -> quint32 { return m_height; }
void WallpaperApplyQuery::setHeight(quint32 v) {
    if (m_height != v) {
        m_height = v;
        Q_EMIT heightChanged();
    }
}

auto WallpaperApplyQuery::fps() const -> quint32 { return m_fps; }
void WallpaperApplyQuery::setFps(quint32 v) {
    if (m_fps != v) {
        m_fps = v;
        Q_EMIT fpsChanged();
    }
}

auto WallpaperApplyQuery::rendererId() const -> const QString& { return m_renderer_id; }

void WallpaperApplyQuery::reload() {
    if (m_wallpaper_id.isEmpty()) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperApplyRequest {};
    inner.setWallpaperId(m_wallpaper_id);
    inner.setWidth(m_width);
    inner.setHeight(m_height);
    inner.setFps(m_fps);
    req.setWallpaperApply(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        if (! result) {
            self->setError(result.unwrap_err());
            co_return;
        }
        auto rsp = result.unwrap();
        self->m_renderer_id = rsp.wallpaperApply().rendererId();
        Q_EMIT self->rendererIdChanged();
        self->setStatus(Status::Finished);
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/wallpaper_query.moc.cpp"
