module;
#include "waywallen/query/settings_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.settings;
import :app;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

namespace
{
auto layout_to_map(const proto::LayoutPrefs& l) -> QVariantMap {
    QVariantMap m;
    m[u"fillmode"_s]    = static_cast<int>(l.fillmode());
    m[u"align"_s]       = static_cast<int>(l.align());
    m[u"locationSet"_s] = l.locationSet();
    m[u"locationX"_s]   = l.locationX();
    m[u"locationY"_s]   = l.locationY();
    m[u"rotation"_s]    = static_cast<int>(l.rotation());
    return m;
}

auto map_to_layout(const QVariantMap& m) -> proto::LayoutPrefs {
    proto::LayoutPrefs l;
    l.setFillmode(static_cast<proto::FillMode>(m.value(u"fillmode"_s).toInt()));
    l.setAlign(static_cast<proto::Align>(m.value(u"align"_s).toInt()));
    l.setLocationSet(m.value(u"locationSet"_s).toBool());
    l.setLocationX(m.value(u"locationX"_s).toUInt());
    l.setLocationY(m.value(u"locationY"_s).toUInt());
    l.setRotation(static_cast<proto::Rotation>(m.value(u"rotation"_s).toInt()));
    return l;
}

auto auto_replay_to_map(const proto::AutoReplayPolicy& p) -> QVariantMap {
    QVariantMap m;
    m[u"anyWindow"_s]       = static_cast<int>(p.anyWindow());
    m[u"focused"_s]         = static_cast<int>(p.focused());
    m[u"maximized"_s]       = static_cast<int>(p.maximized());
    m[u"fullscreen"_s]      = static_cast<int>(p.fullscreen());
    m[u"sessionLocked"_s]   = static_cast<int>(p.sessionLocked());
    m[u"sessionInactive"_s] = static_cast<int>(p.sessionInactive());
    return m;
}

auto map_to_auto_replay(const QVariantMap& m) -> proto::AutoReplayPolicy {
    proto::AutoReplayPolicy p;
    p.setAnyWindow(static_cast<proto::AutoAction>(m.value(u"anyWindow"_s).toInt()));
    p.setFocused(static_cast<proto::AutoAction>(m.value(u"focused"_s).toInt()));
    p.setMaximized(static_cast<proto::AutoAction>(m.value(u"maximized"_s).toInt()));
    p.setFullscreen(static_cast<proto::AutoAction>(m.value(u"fullscreen"_s).toInt()));
    p.setSessionLocked(static_cast<proto::AutoAction>(m.value(u"sessionLocked"_s).toInt()));
    p.setSessionInactive(static_cast<proto::AutoAction>(m.value(u"sessionInactive"_s).toInt()));
    return p;
}

auto global_to_map(const proto::GlobalSettings& g) -> QVariantMap {
    QVariantMap  m;
    QVariantList wallpaper_filters;
    for (const auto& filter : g.wallpaperFilters()) {
        wallpaper_filters.append(QVariant::fromValue(filter));
    }
    m[u"wallpaperFilters"_s] = wallpaper_filters;
    QVariantList wallpaper_filter_logics;
    for (const auto& logic : g.wallpaperFilterLogics()) {
        wallpaper_filter_logics.append(QVariant::fromValue(logic));
    }
    m[u"wallpaperFilterLogics"_s] = wallpaper_filter_logics;
    QVariantList wallpaper_sorts;
    for (const auto& sort : g.wallpaperSorts()) {
        wallpaper_sorts.append(QVariant::fromValue(sort));
    }
    m[u"wallpaperSorts"_s] = wallpaper_sorts;
    if (g.hasLayoutDefaults()) {
        m[u"layoutDefaults"_s] = layout_to_map(g.layoutDefaults());
    }
    if (g.hasAutoReplay()) {
        m[u"autoReplay"_s] = auto_replay_to_map(g.autoReplay());
    }
    m[u"queueMode"_s]                 = g.queueMode();
    m[u"rotationSecs"_s]              = g.rotationSecs();
    m[u"audioFadeMs"_s]               = g.audioFadeMs();
    m[u"pluginUpdateNotifications"_s] = ! g.disablePluginUpdateNotifications();
    m[u"duplicateRenderers"_s]        = g.duplicateRenderersForSameWallpaper();
    m[u"hideTrayIcon"_s]              = g.hideTrayIcon();
    QStringList wallpaper_skip_types;
    for (const auto& t : g.wallpaperSkipTypes()) {
        wallpaper_skip_types.append(t);
    }
    m[u"wallpaperSkipTypes"_s] = wallpaper_skip_types;
    QStringList wallpaper_filter_tags;
    for (const auto& t : g.wallpaperFilterTags()) {
        wallpaper_filter_tags.append(t);
    }
    m[u"wallpaperFilterTags"_s] = wallpaper_filter_tags;
    QStringList wallpaper_skip_content_ratings;
    for (const auto& r : g.wallpaperSkipContentRatings()) {
        wallpaper_skip_content_ratings.append(r);
    }
    m[u"wallpaperSkipContentRatings"_s] = wallpaper_skip_content_ratings;
    return m;
}

auto plugins_to_map(const proto::SettingsGetResponse::PluginsEntry& src) -> QVariantMap {
    QVariantMap out;
    for (auto it = src.constBegin(); it != src.constEnd(); ++it) {
        QVariantMap inner;
        const auto& values = it.value().values();
        for (auto vit = values.constBegin(); vit != values.constEnd(); ++vit) {
            inner[vit.key()] = vit.value();
        }
        out[it.key()] = inner;
    }
    return out;
}

auto map_to_global(const QVariantMap& m) -> proto::GlobalSettings {
    proto::GlobalSettings             g;
    QList<proto::WallpaperFilterRule> wallpaper_filters;
    for (const auto& value : m.value(u"wallpaperFilters"_s).toList()) {
        wallpaper_filters.append(value.value<proto::WallpaperFilterRule>());
    }
    g.setWallpaperFilters(wallpaper_filters);
    QList<proto::FilterLogic> wallpaper_filter_logics;
    for (const auto& value : m.value(u"wallpaperFilterLogics"_s).toList()) {
        wallpaper_filter_logics.append(value.value<proto::FilterLogic>());
    }
    g.setWallpaperFilterLogics(wallpaper_filter_logics);
    QList<proto::WallpaperSortRule> wallpaper_sorts;
    for (const auto& value : m.value(u"wallpaperSorts"_s).toList()) {
        wallpaper_sorts.append(value.value<proto::WallpaperSortRule>());
    }
    g.setWallpaperSorts(wallpaper_sorts);
    // Round-trip layout_defaults so a single-plugin SettingsSet doesn't
    // wipe the daemon's current LayoutPrefs. UI never edits these; it
    // just forwards them.
    if (m.contains(u"layoutDefaults"_s)) {
        g.setLayoutDefaults(map_to_layout(m.value(u"layoutDefaults"_s).toMap()));
    }
    if (m.contains(u"autoReplay"_s)) {
        g.setAutoReplay(map_to_auto_replay(m.value(u"autoReplay"_s).toMap()));
    }
    if (m.contains(u"queueMode"_s)) {
        g.setQueueMode(m.value(u"queueMode"_s).toString());
    }
    if (m.contains(u"rotationSecs"_s)) {
        g.setRotationSecs(m.value(u"rotationSecs"_s).toUInt());
    }
    if (m.contains(u"audioFadeMs"_s)) {
        g.setAudioFadeMs(m.value(u"audioFadeMs"_s).toUInt());
    }
    if (m.contains(u"pluginUpdateNotifications"_s)) {
        g.setDisablePluginUpdateNotifications(! m.value(u"pluginUpdateNotifications"_s).toBool());
    }
    if (m.contains(u"duplicateRenderers"_s)) {
        g.setDuplicateRenderersForSameWallpaper(m.value(u"duplicateRenderers"_s).toBool());
    }
    if (m.contains(u"hideTrayIcon"_s)) {
        g.setHideTrayIcon(m.value(u"hideTrayIcon"_s).toBool());
    }
    if (m.contains(u"wallpaperSkipTypes"_s)) {
        QStringList skip;
        for (const auto& v : m.value(u"wallpaperSkipTypes"_s).toList()) {
            skip.append(v.toString());
        }
        g.setWallpaperSkipTypes(skip);
    }
    if (m.contains(u"wallpaperFilterTags"_s)) {
        QStringList tags;
        for (const auto& v : m.value(u"wallpaperFilterTags"_s).toList()) {
            tags.append(v.toString());
        }
        g.setWallpaperFilterTags(tags);
    }
    if (m.contains(u"wallpaperSkipContentRatings"_s)) {
        QStringList ratings;
        for (const auto& v : m.value(u"wallpaperSkipContentRatings"_s).toList()) {
            ratings.append(v.toString());
        }
        g.setWallpaperSkipContentRatings(ratings);
    }
    return g;
}

auto map_to_plugins(const QVariantMap& m) -> QHash<QString, proto::PluginSettings> {
    QHash<QString, proto::PluginSettings> out;
    for (auto it = m.constBegin(); it != m.constEnd(); ++it) {
        proto::PluginSettings              ps;
        proto::PluginSettings::ValuesEntry values;
        const auto                         inner = it.value().toMap();
        for (auto vit = inner.constBegin(); vit != inner.constEnd(); ++vit) {
            values.insert(vit.key(), vit.value().toString());
        }
        ps.setValues(values);
        out.insert(it.key(), ps);
    }
    return out;
}

} // namespace

// ---------------------------------------------------------------------------
// SettingsGetQuery
// ---------------------------------------------------------------------------

SettingsGetQuery::SettingsGetQuery(QObject* parent): Query(parent) {}

auto SettingsGetQuery::global() const -> const QVariantMap& { return m_global; }
auto SettingsGetQuery::plugins() const -> const QVariantMap& { return m_plugins; }

void SettingsGetQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setSettingsGet(proto::SettingsGetRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto& get_rsp = rsp.settingsGet();
            self->m_global      = global_to_map(get_rsp.global());
            self->m_plugins     = plugins_to_map(get_rsp.plugins());
            Q_EMIT self->globalChanged();
            Q_EMIT self->pluginsChanged();
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// SettingsSetQuery
// ---------------------------------------------------------------------------

SettingsSetQuery::SettingsSetQuery(QObject* parent): Query(parent) {}

auto SettingsSetQuery::global() const -> const QVariantMap& { return m_global; }
void SettingsSetQuery::setGlobal(const QVariantMap& v) {
    if (m_global != v) {
        m_global = v;
        Q_EMIT globalChanged();
    }
}

auto SettingsSetQuery::plugins() const -> const QVariantMap& { return m_plugins; }
void SettingsSetQuery::setPlugins(const QVariantMap& v) {
    if (m_plugins != v) {
        m_plugins = v;
        Q_EMIT pluginsChanged();
    }
}

void SettingsSetQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::SettingsSetRequest {};
    inner.setGlobal(map_to_global(m_global));
    inner.setPlugins(map_to_plugins(m_plugins));
    req.setSettingsSet(std::move(inner));

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

} // namespace waywallen

#include "waywallen/query/settings_query.moc.cpp"
