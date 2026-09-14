module;
#include "waywallen/query/settings_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
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
    m[u"resumeDelayMs"_s]   = p.hasResumeDelayMs() ? p.resumeDelayMs() : 250;
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
    p.setResumeDelayMs(m.value(u"resumeDelayMs"_s, 250).toUInt());
    return p;
}

auto pause_effect_to_map(const proto::PauseEffectConfig& p) -> QVariantMap {
    QVariantMap m;
    m[u"kind"_s] = static_cast<int>(p.kind());
    QVariantMap blur;
    if (p.hasBlur()) {
        blur[u"radius"_s] = p.blur().radius();
    }
    m[u"blur"_s] = blur;
    return m;
}

auto map_to_pause_effect(const QVariantMap& m) -> proto::PauseEffectConfig {
    proto::PauseEffectConfig p;
    p.setKind(static_cast<proto::PauseEffectKind>(m.value(u"kind"_s).toInt()));
    proto::BlurEffectConfig blur;
    blur.setRadius(m.value(u"blur"_s).toMap().value(u"radius"_s).toUInt());
    p.setBlur(std::move(blur));
    return p;
}

auto transition_to_map(const proto::TransitionConfig& p) -> QVariantMap {
    QVariantMap m;
    m[u"kind"_s]       = static_cast<int>(p.kind());
    m[u"durationMs"_s] = p.durationMs();
    m[u"angle"_s]      = p.angle();
    m[u"originX"_s]    = p.originX();
    m[u"originY"_s]    = p.originY();
    return m;
}

auto map_to_transition(const QVariantMap& m) -> proto::TransitionConfig {
    proto::TransitionConfig p;
    p.setKind(static_cast<proto::TransitionKind>(m.value(u"kind"_s).toInt()));
    p.setDurationMs(m.value(u"durationMs"_s, 500).toUInt());
    p.setAngle(m.value(u"angle"_s).toUInt());
    p.setOriginX(m.value(u"originX"_s, 50).toUInt());
    p.setOriginY(m.value(u"originY"_s, 50).toUInt());
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
    if (g.hasPauseEffect()) {
        m[u"pauseEffect"_s] = pause_effect_to_map(g.pauseEffect());
    }
    if (g.hasTransition()) {
        m[u"transition"_s] = transition_to_map(g.transition());
    }
    m[u"queueMode"_s]                 = g.queueMode();
    m[u"rotationSecs"_s]              = g.rotationSecs();
    m[u"audioFadeMs"_s]               = g.audioFadeMs();
    m[u"muteWhenOtherAudio"_s]        = g.hasMuteWhenOtherAudio() && g.muteWhenOtherAudio();
    m[u"audioCaptureEnabled"_s]       = g.audioCaptureEnabled();
    m[u"pointerForwardingEnabled"_s]  = g.pointerForwardingEnabled();
    m[u"pluginUpdateNotifications"_s] = ! g.disablePluginUpdateNotifications();
    m[u"duplicateRenderers"_s]        = g.duplicateRenderersForSameWallpaper();
    const auto has_renderer           = g.hasRenderer();
    m[u"renderer.enable_audio"_s] =
        ! has_renderer || ! g.renderer().hasEnableAudio() || g.renderer().enableAudio();
    m[u"renderer.volume"_s] =
        has_renderer && g.renderer().hasVolume() ? g.renderer().volume() : 100;
    m[u"hideTrayIcon"_s]        = g.hideTrayIcon();
    m[u"debugLoggingEnabled"_s] = g.debugLoggingEnabled();
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
    if (m.contains(u"pauseEffect"_s)) {
        g.setPauseEffect(map_to_pause_effect(m.value(u"pauseEffect"_s).toMap()));
    }
    if (m.contains(u"transition"_s)) {
        g.setTransition(map_to_transition(m.value(u"transition"_s).toMap()));
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
    if (m.contains(u"muteWhenOtherAudio"_s)) {
        g.setMuteWhenOtherAudio(m.value(u"muteWhenOtherAudio"_s).toBool());
    }
    g.setAudioCaptureEnabled(m.value(u"audioCaptureEnabled"_s).toBool());
    if (m.contains(u"pointerForwardingEnabled"_s)) {
        g.setPointerForwardingEnabled(m.value(u"pointerForwardingEnabled"_s).toBool());
    }
    if (m.contains(u"pluginUpdateNotifications"_s)) {
        g.setDisablePluginUpdateNotifications(! m.value(u"pluginUpdateNotifications"_s).toBool());
    }
    if (m.contains(u"duplicateRenderers"_s)) {
        g.setDuplicateRenderersForSameWallpaper(m.value(u"duplicateRenderers"_s).toBool());
    }
    if (m.contains(u"renderer.enable_audio"_s) || m.contains(u"renderer.volume"_s)) {
        proto::GlobalRendererSettings renderer;
        if (m.contains(u"renderer.enable_audio"_s)) {
            renderer.setEnableAudio(m.value(u"renderer.enable_audio"_s).toBool());
        }
        if (m.contains(u"renderer.volume"_s)) {
            renderer.setVolume(m.value(u"renderer.volume"_s).toUInt());
        }
        g.setRenderer(std::move(renderer));
    }
    if (m.contains(u"hideTrayIcon"_s)) {
        g.setHideTrayIcon(m.value(u"hideTrayIcon"_s).toBool());
    }
    if (m.contains(u"debugLoggingEnabled"_s)) {
        g.setDebugLoggingEnabled(m.value(u"debugLoggingEnabled"_s).toBool());
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
auto SettingsGetQuery::logDir() const -> const QString& { return m_log_dir; }
auto SettingsGetQuery::wwLogActive() const -> bool { return m_ww_log_active; }

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
            const auto& get_rsp   = rsp.settingsGet();
            self->m_global        = global_to_map(get_rsp.global());
            self->m_plugins       = plugins_to_map(get_rsp.plugins());
            self->m_log_dir       = get_rsp.logDir();
            self->m_ww_log_active = get_rsp.wwLogActive();
            Q_EMIT self->globalChanged();
            Q_EMIT self->pluginsChanged();
            Q_EMIT self->logDirChanged();
            Q_EMIT self->wwLogActiveChanged();
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
