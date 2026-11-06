module;
#include "waywallen/query/wallpaper_query.moc.h"
#include <algorithm>
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
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
    connect(this, &WallpaperListQuery::wpTypeChanged, this, &WallpaperListQuery::invalidateList);
    connect(
        this, &WallpaperListQuery::filterStateChanged, this, &WallpaperListQuery::invalidateList);
    connect(this, &WallpaperListQuery::sortsChanged, this, &WallpaperListQuery::invalidateList);
    connect(
        this, &WallpaperListQuery::searchTextChanged, this, &WallpaperListQuery::invalidateList);
    connect(this, &WallpaperListQuery::skipTypesChanged, this, &WallpaperListQuery::invalidateList);
    connect(
        this, &WallpaperListQuery::filterTagsChanged, this, &WallpaperListQuery::invalidateList);
    connect(this,
            &WallpaperListQuery::skipContentRatingsChanged,
            this,
            &WallpaperListQuery::invalidateList);
    connect(
        this, &WallpaperListQuery::hiddenFilterChanged, this, &WallpaperListQuery::invalidateList);
    connect(this,
            &WallpaperListQuery::hiddenLibraryIdsChanged,
            this,
            &WallpaperListQuery::invalidateList);
    connect(this, &WallpaperListQuery::limitChanged, this, &WallpaperListQuery::invalidateList);
    connect(this,
            &WallpaperListQuery::filterStateChanged,
            this,
            &WallpaperListQuery::hasOtherFiltersChanged);
    connect(this,
            &WallpaperListQuery::skipTypesChanged,
            this,
            &WallpaperListQuery::hasOtherFiltersChanged);
    connect(this,
            &WallpaperListQuery::filterTagsChanged,
            this,
            &WallpaperListQuery::hasOtherFiltersChanged);
    connect(this,
            &WallpaperListQuery::skipContentRatingsChanged,
            this,
            &WallpaperListQuery::hasOtherFiltersChanged);
    connect(this,
            &WallpaperListQuery::hiddenFilterChanged,
            this,
            &WallpaperListQuery::hasOtherFiltersChanged);
}

void WallpaperListQuery::invalidateList() {
    Q_EMIT selectionChanged();
    m_reload_pending = true;
    delayReload();
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

auto WallpaperListQuery::hiddenFilter() const -> int { return m_hidden_filter; }

void WallpaperListQuery::setHiddenFilter(int v) {
    if (m_hidden_filter == v) return;
    const bool had_active = hasActiveFilters();
    m_hidden_filter       = v;
    setOffset(0);
    Q_EMIT hiddenFilterChanged();
    if (had_active != hasActiveFilters()) Q_EMIT hasActiveFiltersChanged();
}

auto WallpaperListQuery::hasActiveFilters() const -> bool {
    return ! m_hidden_library_ids.isEmpty() || hasOtherFilters();
}

auto WallpaperListQuery::hasOtherFilters() const -> bool {
    return ! m_filters.isEmpty() || ! m_skip_types.isEmpty() || ! m_filter_tags.isEmpty() ||
           ! m_skip_content_ratings.isEmpty() || m_hidden_filter != 0;
}

auto WallpaperListQuery::total() const -> qint32 { return m_total; }

auto WallpaperListQuery::hiddenLibraryIds() const -> const QList<qlonglong>& {
    return m_hidden_library_ids;
}

void WallpaperListQuery::setHiddenLibraryIds(const QList<qlonglong>& ids) {
    auto normalized = ids;
    std::sort(normalized.begin(), normalized.end());
    normalized.erase(std::unique(normalized.begin(), normalized.end()), normalized.end());
    if (normalized == m_hidden_library_ids) return;
    const bool had_active = hasActiveFilters();
    m_hidden_library_ids  = std::move(normalized);
    setOffset(0);
    Q_EMIT hiddenLibraryIdsChanged();
    if (had_active != hasActiveFilters()) Q_EMIT hasActiveFiltersChanged();
}

void WallpaperListQuery::toggleLibraryHidden(qlonglong id) {
    auto ids = m_hidden_library_ids;
    if (! ids.removeOne(id)) ids.append(id);
    setHiddenLibraryIds(ids);
}

auto WallpaperListQuery::listRequest() const -> proto::WallpaperListRequest {
    auto inner = proto::WallpaperListRequest {};
    inner.setWpType(m_wp_type);
    inner.setFilters(m_filters);
    inner.setFilterLogics(m_filter_logics);
    inner.setSorts(m_sorts);
    inner.setSearchText(m_search_text);
    inner.setSkipTypes(m_skip_types);
    inner.setFilterTags(m_filter_tags);
    inner.setSkipContentRatings(m_skip_content_ratings);
    inner.setHiddenFilter(static_cast<control::v1::WallpaperHiddenFilter>(m_hidden_filter));
    auto selection = proto::WallpaperLibraryVisibility {};
    auto ids       = selection.hiddenLibraryIds();
    for (auto id : m_hidden_library_ids) ids.append(id);
    selection.setHiddenLibraryIds(std::move(ids));
    inner.setLibraryVisibility(selection);
    return inner;
}

auto WallpaperListQuery::selection() const -> QVariant {
    proto::WallpaperSelection s;
    s.setFilters(m_filters);
    s.setFilterLogics(m_filter_logics);
    s.setSorts(m_sorts);
    s.setSearchText(m_search_text);
    s.setWpType(m_wp_type);
    s.setSkipTypes(m_skip_types);
    s.setFilterTags(m_filter_tags);
    s.setSkipContentRatings(m_skip_content_ratings);
    s.setHiddenFilter(static_cast<proto::WallpaperHiddenFilter>(m_hidden_filter));
    auto ids = s.hiddenLibraryIds();
    for (auto id : m_hidden_library_ids) ids.append(id);
    s.setHiddenLibraryIds(std::move(ids));
    return QVariant::fromValue(s);
}

bool WallpaperListQuery::matchesSelection(const QVariant& value) const {
    return value.canConvert<proto::WallpaperSelection>() &&
           selection().value<proto::WallpaperSelection>() ==
               value.value<proto::WallpaperSelection>();
}

void WallpaperListQuery::restoreSelection(const QVariant& value) {
    if (! value.canConvert<proto::WallpaperSelection>()) return;
    const auto s = value.value<proto::WallpaperSelection>();
    replaceFilterState(s.filters(), s.filterLogics());
    setSorts(s.sorts());
    setSearchText(s.searchText());
    setWpType(s.wpType());
    setSkipTypes(s.skipTypes());
    setFilterTags(s.filterTags());
    setSkipContentRatings(s.skipContentRatings());
    setHiddenFilter(static_cast<int>(s.hiddenFilter()));
    QList<qlonglong> ids;
    for (auto id : s.hiddenLibraryIds()) ids.append(static_cast<qint64>(id));
    setHiddenLibraryIds(ids);
}

void WallpaperListQuery::reload() {
    const auto revision = beginRequest();
    m_reload_pending    = false;
    m_failed_append     = false;
    setOffset(0);
    setNoMore(false);
    tdata()->setHasMore(false);
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();
    auto req     = proto::Request {};
    auto inner   = listRequest();
    initReqForReload(inner);
    req.setWallpaperList(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

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
    if (noMore() || m_reload_pending || status() == Status::Querying) return;
    const auto revision = beginRequest();
    m_failed_append     = true;
    tdata()->setHasMore(false);
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = listRequest();
    initReqForFetchMore(inner);
    req.setWallpaperList(std::move(inner));

    const qint32 next_offset = offset() + 1;
    auto         self        = QWatcher { this };
    spawn([self, backend, req = std::move(req), next_offset, revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

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
            self->m_failed_append = false;

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

void WallpaperListQuery::retry() {
    if (status() != Status::Error) return;
    if (m_failed_append)
        fetchMore(tdata()->rowCount());
    else
        reload();
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
        if (! co_await QAsyncResult::qexecutor()) co_return;
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
    const auto revision = beginRequest();
    if (m_wallpaper_id.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    const auto wallpaper_id = m_wallpaper_id;
    auto       req          = proto::Request {};
    auto       inner        = proto::WallpaperGetRequest {};
    inner.setWallpaperId(wallpaper_id);
    req.setWallpaperGet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

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
        if (! co_await QAsyncResult::qexecutor()) co_return;
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
// WallpaperUnsubscribeQuery
// ---------------------------------------------------------------------------

WallpaperUnsubscribeQuery::WallpaperUnsubscribeQuery(QObject* parent): Query(parent) {}

auto WallpaperUnsubscribeQuery::wallpaperId() const -> const QString& { return m_wallpaper_id; }
void WallpaperUnsubscribeQuery::setWallpaperId(const QString& v) {
    if (m_wallpaper_id != v) {
        m_wallpaper_id = v;
        Q_EMIT wallpaperIdChanged();
    }
}

void WallpaperUnsubscribeQuery::reload() {
    const auto wallpaper_id = m_wallpaper_id;
    if (wallpaper_id.isEmpty()) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperUnsubscribeRequest {};
    inner.setWallpaperId(wallpaper_id);
    req.setWallpaperUnsubscribe(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), wallpaper_id]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        if (self->m_wallpaper_id != wallpaper_id) co_return;
        self->inspect_set(result, [self, wallpaper_id](const proto::Response&) {
            Q_EMIT self->unsubscribed(wallpaper_id);
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperHideQuery
// ---------------------------------------------------------------------------

WallpaperHideQuery::WallpaperHideQuery(QObject* parent): Query(parent) {}

auto WallpaperHideQuery::wallpaperId() const -> const QString& { return m_wallpaper_id; }
void WallpaperHideQuery::setWallpaperId(const QString& v) {
    if (m_wallpaper_id != v) {
        m_wallpaper_id = v;
        Q_EMIT wallpaperIdChanged();
    }
}

auto WallpaperHideQuery::hidden() const -> bool { return m_hidden; }
void WallpaperHideQuery::setHidden(bool v) {
    if (m_hidden != v) {
        m_hidden = v;
        Q_EMIT hiddenChanged();
    }
}

void WallpaperHideQuery::reload() { setHiddenForIds(QStringList { m_wallpaper_id }, m_hidden); }

void WallpaperHideQuery::setHiddenForIds(const QStringList& wallpaperIds, bool hidden) {
    QStringList ids;
    ids.reserve(wallpaperIds.size());
    for (const auto& id : wallpaperIds) {
        if (! id.isEmpty()) ids.append(id);
    }
    if (ids.isEmpty()) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperHideRequest {};
    if (ids.size() == 1)
        inner.setWallpaperId(ids.first());
    else
        inner.setWallpaperIds(ids);
    inner.setHidden(hidden);
    req.setWallpaperHide(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), ids = std::move(ids), hidden]() mutable
              -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [self, ids, hidden](const proto::Response& rsp) {
            Q_EMIT self->updated(ids, hidden, rsp.wallpaperHide().updatedCount());
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
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [](const proto::Response&) {
            // No payload; success is just the absence of an error.
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// WallpaperLayoutSetQuery
// ---------------------------------------------------------------------------

WallpaperLayoutSetQuery::WallpaperLayoutSetQuery(QObject* parent): Query(parent) {}

#define WW_SET(field, val)          \
    do {                            \
        if (this->field != val) {   \
            this->field = val;      \
            Q_EMIT paramsChanged(); \
        }                           \
    } while (0)

void WallpaperLayoutSetQuery::setWallpaperId(const QString& v) { WW_SET(m_wallpaper_id, v); }
void WallpaperLayoutSetQuery::setClear(bool v) { WW_SET(m_clear, v); }
void WallpaperLayoutSetQuery::setFillmode(int v) { WW_SET(m_fillmode, v); }
void WallpaperLayoutSetQuery::setLocationX(int v) { WW_SET(m_location_x, v); }
void WallpaperLayoutSetQuery::setLocationY(int v) { WW_SET(m_location_y, v); }
void WallpaperLayoutSetQuery::setRotation(int v) { WW_SET(m_rotation, v); }
void WallpaperLayoutSetQuery::setFlip(int v) { WW_SET(m_flip, v); }
#undef WW_SET

void WallpaperLayoutSetQuery::reload() {
    if (m_wallpaper_id.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto inner = proto::WallpaperLayoutSetRequest {};
    inner.setWallpaperId(m_wallpaper_id);
    inner.setClear(m_clear);
    if (! m_clear) {
        auto layout = proto::LayoutPrefs {};
        layout.setFillmode(static_cast<proto::FillMode>(m_fillmode));
        layout.setLocationX(static_cast<quint32>(std::clamp(m_location_x, 0, 100)));
        layout.setLocationY(static_cast<quint32>(std::clamp(m_location_y, 0, 100)));
        layout.setLocationSet(true);
        layout.setRotation(static_cast<proto::Rotation>(m_rotation));
        layout.setFlip(static_cast<proto::Flip>(m_flip));
        inner.setLayout(std::move(layout));
    }

    auto req = proto::Request {};
    req.setWallpaperLayoutSet(std::move(inner));

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

auto WallpaperApplyQuery::targets() const -> const QVariantList& { return m_targets; }
void WallpaperApplyQuery::setTargets(const QVariantList& v) {
    if (m_targets != v) {
        m_targets = v;
        Q_EMIT targetsChanged();
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
auto WallpaperApplyQuery::stoppedPlaylists() const -> const QVariantList& {
    return m_stopped_playlists;
}

void WallpaperApplyQuery::reload() {
    if (m_wallpaper.id_proto().isEmpty()) return;

    if (! m_stopped_playlists.isEmpty()) {
        m_stopped_playlists.clear();
        Q_EMIT stoppedPlaylistsChanged();
    }
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::WallpaperApplyRequest {};
    inner.setWallpaperId(m_wallpaper.id_proto());
    inner.setTargets(presentationTargetsFromVariant(m_targets));
    inner.setRendererName(m_renderer_name);
    req.setWallpaperApply(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            self->m_renderer_id = rsp.wallpaperApply().rendererId();
            Q_EMIT self->rendererIdChanged();

            QVariantList stopped;
            for (const auto& playlist : rsp.wallpaperApply().stoppedPlaylists()) {
                QVariantMap item;
                item[u"playlistId"_s]   = static_cast<qint64>(playlist.playlistId());
                item[u"playlistName"_s] = playlist.playlistName();
                item[u"allDisplays"_s]  = playlist.allDisplays();
                QVariantList display_ids;
                for (const auto id : playlist.displayIds()) {
                    display_ids.append(QVariant::fromValue(static_cast<quint64>(id)));
                }
                item[u"displayIds"_s] = display_ids;
                stopped.append(item);
            }
            self->m_stopped_playlists = std::move(stopped);
            Q_EMIT self->stoppedPlaylistsChanged();
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
        if (! co_await QAsyncResult::qexecutor()) co_return;
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
