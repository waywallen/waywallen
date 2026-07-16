module;
#include "waywallen/query/wallpaper_query.moc.h"
#include <qtprotobuftypes.h>
#include <algorithm>
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
    setLimit(0);
    tdata()->set_store(tdata(), AppStore::instance()->wallpapers);
    connect_requet_reload(&WallpaperListQuery::wpTypeChanged, this);
    connect_requet_reload(&WallpaperListQuery::filterStateChanged, this);
    connect_requet_reload(&WallpaperListQuery::sortsChanged, this);
    connect_requet_reload(&WallpaperListQuery::searchTextChanged, this);
    connect_requet_reload(&WallpaperListQuery::skipTypesChanged, this);
    connect_requet_reload(&WallpaperListQuery::filterTagsChanged, this);
    connect_requet_reload(&WallpaperListQuery::skipContentRatingsChanged, this);
}

auto WallpaperListQuery::wpType() const -> const QString& { return m_wp_type; }
void WallpaperListQuery::setWpType(const QString& v) {
    if (m_wp_type != v) {
        m_wp_type = v;
        setOffset(0);
        Q_EMIT wpTypeChanged();
    }
}

auto WallpaperListQuery::filters() const -> const QList<control::v1::WallpaperFilterRule>& {
    return m_filters;
}

void WallpaperListQuery::setFilters(const QList<control::v1::WallpaperFilterRule>& v) {
    if (m_filters == v) return;
    const bool had_active = hasActiveFilters();
    m_filters             = v;
    setOffset(0);
    Q_EMIT filtersChanged();
    Q_EMIT filterStateChanged();
    if (had_active != hasActiveFilters()) Q_EMIT hasActiveFiltersChanged();
}

auto WallpaperListQuery::filterLogics() const -> const QList<control::v1::FilterLogic>& {
    return m_filter_logics;
}

void WallpaperListQuery::setFilterLogics(const QList<control::v1::FilterLogic>& v) {
    if (m_filter_logics == v) return;
    m_filter_logics = v;
    setOffset(0);
    Q_EMIT filterLogicsChanged();
    Q_EMIT filterStateChanged();
}

bool WallpaperListQuery::replaceFilterState(const QList<control::v1::WallpaperFilterRule>& filters,
                                            const QList<control::v1::FilterLogic>&         logics) {
    const bool filters_changed = m_filters != filters;
    const bool logics_changed  = m_filter_logics != logics;
    if (! filters_changed && ! logics_changed) return false;

    const bool had_active = hasActiveFilters();
    m_filters             = filters;
    m_filter_logics       = logics;
    setOffset(0);
    if (filters_changed) Q_EMIT filtersChanged();
    if (logics_changed) Q_EMIT filterLogicsChanged();
    Q_EMIT filterStateChanged();
    if (had_active != hasActiveFilters()) Q_EMIT hasActiveFiltersChanged();
    return true;
}

auto WallpaperListQuery::sorts() const -> const QList<control::v1::WallpaperSortRule>& {
    return m_sorts;
}

void WallpaperListQuery::setSorts(const QList<control::v1::WallpaperSortRule>& v) {
    if (m_sorts == v) return;
    m_sorts = v;
    setOffset(0);
    Q_EMIT sortsChanged();
}

auto WallpaperListQuery::searchText() const -> const QString& { return m_search_text; }

void WallpaperListQuery::setSearchText(const QString& v) {
    if (m_search_text == v) return;
    m_search_text = v;
    setOffset(0);
    Q_EMIT searchTextChanged();
}

auto WallpaperListQuery::skipTypes() const -> const QStringList& { return m_skip_types; }

void WallpaperListQuery::setSkipTypes(const QStringList& v) {
    if (m_skip_types == v) return;
    const bool had_active = hasActiveFilters();
    m_skip_types          = v;
    setOffset(0);
    Q_EMIT skipTypesChanged();
    if (had_active != hasActiveFilters()) Q_EMIT hasActiveFiltersChanged();
}

auto WallpaperListQuery::filterTags() const -> const QStringList& { return m_filter_tags; }

void WallpaperListQuery::setFilterTags(const QStringList& v) {
    if (m_filter_tags == v) return;
    const bool had_active = hasActiveFilters();
    m_filter_tags         = v;
    setOffset(0);
    Q_EMIT filterTagsChanged();
    if (had_active != hasActiveFilters()) Q_EMIT hasActiveFiltersChanged();
}

auto WallpaperListQuery::skipContentRatings() const -> const QStringList& {
    return m_skip_content_ratings;
}

void WallpaperListQuery::setSkipContentRatings(const QStringList& v) {
    if (m_skip_content_ratings == v) return;
    const bool had_active  = hasActiveFilters();
    m_skip_content_ratings = v;
    setOffset(0);
    Q_EMIT skipContentRatingsChanged();
    if (had_active != hasActiveFilters()) Q_EMIT hasActiveFiltersChanged();
}

auto WallpaperListQuery::hasActiveFilters() const -> bool {
    return ! m_filters.isEmpty() || ! m_skip_types.isEmpty() || ! m_filter_tags.isEmpty() ||
           ! m_skip_content_ratings.isEmpty();
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
    inner.setFilters(m_filters);
    inner.setFilterLogics(m_filter_logics);
    inner.setSorts(m_sorts);
    inner.setSearchText(m_search_text);
    inner.setSkipTypes(m_skip_types);
    inner.setFilterTags(m_filter_tags);
    inner.setSkipContentRatings(m_skip_content_ratings);
    initReqForReload(inner);
    req.setWallpaperList(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;

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
    inner.setFilters(m_filters);
    inner.setFilterLogics(m_filter_logics);
    inner.setSorts(m_sorts);
    inner.setSearchText(m_search_text);
    inner.setSkipTypes(m_skip_types);
    inner.setFilterTags(m_filter_tags);
    inner.setSkipContentRatings(m_skip_content_ratings);
    initReqForFetchMore(inner);
    req.setWallpaperList(std::move(inner));

    const qint32 next_offset = offset() + 1;
    auto         self        = QWatcher { this };
    spawn([self, backend, req = std::move(req), next_offset]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;

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
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_count = rsp.wallpaperScan().count();
            Q_EMIT self->countChanged();
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperGetQuery
// ---------------------------------------------------------------------------

WallpaperGetQuery::WallpaperGetQuery(QObject* parent): Query(parent) {
    connect_requet_reload(&WallpaperGetQuery::wallpaperIdChanged, this);
}

auto WallpaperGetQuery::wallpaperId() const -> const QString& { return m_wallpaper_id; }
void WallpaperGetQuery::setWallpaperId(const QString& v) {
    if (m_wallpaper_id != v) {
        m_wallpaper_id = v;
        Q_EMIT wallpaperIdChanged();
    }
}

auto WallpaperGetQuery::wallpaper() const -> const model::Wallpaper& { return m_wallpaper; }

void WallpaperGetQuery::reload() {
    if (m_wallpaper_id.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    const auto wallpaper_id = m_wallpaper_id;
    auto       req          = proto::Request {};
    auto       inner        = proto::WallpaperGetRequest {};
    inner.setWallpaperId(wallpaper_id);
    req.setWallpaperGet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), wallpaper_id]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;
        if (self->m_wallpaper_id != wallpaper_id) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_wallpaper = rsp.wallpaperGet().entry();
            Q_EMIT self->wallpaperChanged();
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperRemoveQuery
// ---------------------------------------------------------------------------

WallpaperRemoveQuery::WallpaperRemoveQuery(QObject* parent): Query(parent) {}

auto WallpaperRemoveQuery::wallpaperId() const -> const QString& { return m_wallpaper_id; }
void WallpaperRemoveQuery::setWallpaperId(const QString& v) {
    if (m_wallpaper_id != v) {
        m_wallpaper_id = v;
        Q_EMIT wallpaperIdChanged();
    }
}

void WallpaperRemoveQuery::reload() { remove(QStringList { m_wallpaper_id }); }

void WallpaperRemoveQuery::remove(const QStringList& wallpaperIds) {
    QStringList ids;
    ids.reserve(wallpaperIds.size());
    for (const auto& id : wallpaperIds) {
        if (! id.isEmpty()) ids.append(id);
    }
    if (ids.isEmpty()) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperRemoveRequest {};
    if (ids.size() == 1)
        inner.setWallpaperId(ids.first());
    else
        inner.setWallpaperIds(ids);
    req.setWallpaperRemove(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), ids = std::move(ids)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;
        self->inspect_set(result, [self, ids](const proto::Response& rsp) {
            const auto removedCount = rsp.wallpaperRemove().removedCount();
            if (ids.size() == 1) Q_EMIT self->removed(ids.first());
            Q_EMIT self->removedMany(ids, removedCount);
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperPropertySetQuery
// ---------------------------------------------------------------------------

WallpaperPropertySetQuery::WallpaperPropertySetQuery(QObject* parent): Query(parent) {}

auto WallpaperPropertySetQuery::wallpaperId() const -> const QString& { return m_wallpaper_id; }
void WallpaperPropertySetQuery::setWallpaperId(const QString& v) {
    if (m_wallpaper_id != v) {
        m_wallpaper_id = v;
        Q_EMIT wallpaperIdChanged();
    }
}

auto WallpaperPropertySetQuery::propertyKey() const -> const QString& { return m_property_key; }
void WallpaperPropertySetQuery::setPropertyKey(const QString& v) {
    if (m_property_key != v) {
        m_property_key = v;
        Q_EMIT propertyKeyChanged();
    }
}

auto WallpaperPropertySetQuery::propertyValue() const -> const QString& { return m_property_value; }
void WallpaperPropertySetQuery::setPropertyValue(const QString& v) {
    if (m_property_value != v) {
        m_property_value = v;
        Q_EMIT propertyValueChanged();
    }
}

auto WallpaperPropertySetQuery::reset() const -> bool { return m_reset; }
void WallpaperPropertySetQuery::setReset(bool v) {
    if (m_reset != v) {
        m_reset = v;
        Q_EMIT resetChanged();
    }
}

void WallpaperPropertySetQuery::reload() {
    if (m_wallpaper_id.isEmpty() || m_property_key.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperPropertySetRequest {};
    inner.setWallpaperId(m_wallpaper_id);
    inner.setKey(m_property_key);
    if (m_reset)
        inner.setReset(true);
    else
        inner.setValue(m_property_value);
    req.setWallpaperPropertySet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;
        self->inspect_set(result, [](const proto::Response&) {
            // No payload; success is just the absence of an error.
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

auto WallpaperApplyQuery::rendererName() const -> const QString& { return m_renderer_name; }
void WallpaperApplyQuery::setRendererName(const QString& v) {
    if (m_renderer_name != v) {
        m_renderer_name = v;
        Q_EMIT rendererNameChanged();
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
    inner.setRendererName(m_renderer_name);
    req.setWallpaperApply(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_renderer_id = rsp.wallpaperApply().rendererId();
            Q_EMIT self->rendererIdChanged();
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperApplyViaPortalQuery
// ---------------------------------------------------------------------------

WallpaperApplyViaPortalQuery::WallpaperApplyViaPortalQuery(QObject* parent): Query(parent) {}

auto WallpaperApplyViaPortalQuery::wallpaperId() const -> const QString& { return m_wallpaper_id; }
void WallpaperApplyViaPortalQuery::setWallpaperId(const QString& v) {
    if (m_wallpaper_id != v) {
        m_wallpaper_id = v;
        Q_EMIT wallpaperIdChanged();
    }
}

auto WallpaperApplyViaPortalQuery::uri() const -> const QString& { return m_uri; }

void WallpaperApplyViaPortalQuery::reload() {
    if (m_wallpaper_id.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperApplyViaPortalRequest {};
    inner.setWallpaperId(m_wallpaper_id);
    req.setWallpaperApplyViaPortal(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;
        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_uri = rsp.wallpaperApplyViaPortal().uri();
            Q_EMIT self->uriChanged();
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/wallpaper_query.moc.cpp"
