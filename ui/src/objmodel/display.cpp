module;
#include "waywallen/objmodel/display.moc.h"

#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
import :display;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;
using namespace rstd::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

auto Display::linksFromPb(const proto::DisplayInfo& info) -> QVariantList {
    QVariantList out;
    for (const auto& l : info.links()) {
        QVariantMap m;
        m[u"rendererId"_s] = l.rendererId();
        m[u"zOrder"_s]     = static_cast<int>(l.zOrder());
        m[u"active"_s]     = l.active();
        out.append(m);
    }
    return out;
}

auto Display::effectiveLayoutFromPb(const proto::DisplayInfo& info) -> QVariantMap {
    QVariantMap m;
    if (! info.hasEffectiveLayout()) return m;
    const auto& l     = info.effectiveLayout();
    m[u"fillmode"_s]  = static_cast<int>(l.fillmode());
    m[u"align"_s]     = static_cast<int>(l.align());
    m[u"locationX"_s] = l.locationX();
    m[u"locationY"_s] = l.locationY();
    m[u"rotation"_s]  = static_cast<int>(l.rotation());
    m[u"flip"_s]      = static_cast<int>(l.flip());
    return m;
}

auto Display::displayLayoutFromPb(const proto::DisplayInfo& info) -> QVariantMap {
    QVariantMap m;
    if (! info.hasDisplayLayout()) return m;
    const auto& l     = info.displayLayout();
    m[u"fillmode"_s]  = static_cast<int>(l.fillmode());
    m[u"align"_s]     = static_cast<int>(l.align());
    m[u"locationX"_s] = l.locationX();
    m[u"locationY"_s] = l.locationY();
    m[u"rotation"_s]  = static_cast<int>(l.rotation());
    m[u"flip"_s]      = static_cast<int>(l.flip());
    return m;
}

auto Display::layoutOverriddenByWallpaperFromPb(const proto::DisplayInfo& info) -> bool {
    return info.effectiveLayoutSource() == proto::LayoutSource::LAYOUT_SOURCE_WALLPAPER;
}

auto Display::layoutOverrideFromPb(const proto::DisplayInfo& info) -> QVariantMap {
    QVariantMap m;
    if (! info.hasLayoutOverride()) return m;
    const auto& o       = info.layoutOverride();
    m[u"fillmodeSet"_s] = o.fillmodeSet();
    m[u"fillmode"_s]    = static_cast<int>(o.fillmode());
    m[u"alignSet"_s]    = o.alignSet();
    m[u"align"_s]       = static_cast<int>(o.align());
    m[u"locationSet"_s] = o.locationSet();
    m[u"locationX"_s]   = o.locationX();
    m[u"locationY"_s]   = o.locationY();
    m[u"rotationSet"_s] = o.rotationSet();
    m[u"flipSet"_s]     = o.flipSet();
    m[u"flip"_s]        = static_cast<int>(o.flip());
    m[u"rotation"_s]    = static_cast<int>(o.rotation());
    return m;
}

auto Display::playlistStatusFromPb(const proto::PlaylistDisplayStatus* status) -> QVariantMap {
    QVariantMap m;
    if (! status || status->activeId() <= 0) return m;
    m[u"activeId"_s]      = static_cast<qint64>(status->activeId());
    m[u"mode"_s]          = static_cast<int>(status->mode());
    m[u"intervalSecs"_s]  = status->intervalSecs();
    m[u"currentId"_s]     = status->currentId();
    m[u"position"_s]      = status->position();
    m[u"count"_s]         = status->count();
    m[u"remainingSecs"_s] = status->remainingSecs();
    m[u"waiting"_s]       = status->waiting();
    m[u"error"_s]         = status->error();
    return m;
}

auto Display::canvasRectFromPb(const proto::CanvasRect& rect) -> QVariantMap {
    QVariantMap out;
    out[u"x"_s]      = rect.x();
    out[u"y"_s]      = rect.y();
    out[u"width"_s]  = rect.width();
    out[u"height"_s] = rect.height();
    return out;
}

Display::Display(const proto::DisplayInfo& info, QObject* parent)
    : QObject(parent),
      m_id(info.displayId()),
      m_name(info.name()),
      m_alias(info.alias()),
      m_instance_id(info.instanceId()),
      m_settings_key(info.settingsKey()),
      m_width(info.width()),
      m_height(info.height()),
      m_refresh_mhz(info.refreshMhz()),
      m_links(linksFromPb(info)),
      m_effective_layout(effectiveLayoutFromPb(info)),
      m_display_layout(displayLayoutFromPb(info)),
      m_layout_overridden_by_wallpaper(layoutOverriddenByWallpaperFromPb(info)),
      m_layout_override(layoutOverrideFromPb(info)),
      m_drm_render_major(info.drmRenderMajor()),
      m_drm_render_minor(info.drmRenderMinor()),
      m_runtime_conditions(runtimeConditionsFromPb(info.conditions())),
      m_canvas_id(info.canvasId()),
      m_canvas_rect(info.hasCanvasRect() ? canvasRectFromPb(info.canvasRect()) : QVariantMap {}),
      m_canvas_overlap_count(info.canvasOverlapCount()),
      m_selectable_target(info.selectableTarget()),
      m_manual_paused(info.manualPaused()),
      m_effective_paused(info.effectivePaused()),
      m_window_exclusion_support(static_cast<int>(info.windowExclusionSupport())),
      m_unsupported_window_exclusions(info.unsupportedWindowExclusions()) {}

void Display::updateFrom(const proto::DisplayInfo& info) {
    rstd_assert(info.displayId() == m_id, "Display::updateFrom id mismatch");
    if (m_window_exclusion_support != static_cast<int>(info.windowExclusionSupport()) ||
        m_unsupported_window_exclusions != info.unsupportedWindowExclusions()) {
        m_window_exclusion_support      = static_cast<int>(info.windowExclusionSupport());
        m_unsupported_window_exclusions = info.unsupportedWindowExclusions();
        Q_EMIT windowExclusionSupportChanged();
    }
    if (m_manual_paused != info.manualPaused() || m_effective_paused != info.effectivePaused()) {
        m_manual_paused    = info.manualPaused();
        m_effective_paused = info.effectivePaused();
        Q_EMIT pauseChanged();
    }

    bool label_changed = false;
    if (m_name != info.name()) {
        m_name = info.name();
        Q_EMIT nameChanged();
        label_changed = true;
    }
    if (m_alias != info.alias()) {
        m_alias = info.alias();
        Q_EMIT aliasChanged();
        label_changed = true;
    }
    if (label_changed) Q_EMIT displayLabelChanged();
    if (m_instance_id != info.instanceId() || m_settings_key != info.settingsKey()) {
        m_instance_id  = info.instanceId();
        m_settings_key = info.settingsKey();
        Q_EMIT identityChanged();
    }
    bool size_changed = false;
    if (m_width != info.width()) {
        m_width      = info.width();
        size_changed = true;
    }
    if (m_height != info.height()) {
        m_height     = info.height();
        size_changed = true;
    }
    if (size_changed) Q_EMIT sizeChanged();
    if (m_refresh_mhz != info.refreshMhz()) {
        m_refresh_mhz = info.refreshMhz();
        Q_EMIT refreshMhzChanged();
    }
    auto new_links = linksFromPb(info);
    if (m_links != new_links) {
        m_links = std::move(new_links);
        Q_EMIT linksChanged();
    }
    auto new_eff            = effectiveLayoutFromPb(info);
    auto new_display_layout = displayLayoutFromPb(info);
    auto new_overridden     = layoutOverriddenByWallpaperFromPb(info);
    auto new_ovr            = layoutOverrideFromPb(info);
    if (m_effective_layout != new_eff || m_display_layout != new_display_layout ||
        m_layout_overridden_by_wallpaper != new_overridden || m_layout_override != new_ovr) {
        m_effective_layout               = std::move(new_eff);
        m_display_layout                 = std::move(new_display_layout);
        m_layout_overridden_by_wallpaper = new_overridden;
        m_layout_override                = std::move(new_ovr);
        Q_EMIT layoutChanged();
    }
    auto conditions = runtimeConditionsFromPb(info.conditions());
    if (m_runtime_conditions != conditions) {
        m_runtime_conditions = std::move(conditions);
        Q_EMIT runtimeConditionsChanged();
    }
    auto canvas_rect = info.hasCanvasRect() ? canvasRectFromPb(info.canvasRect()) : QVariantMap {};
    if (m_canvas_id != info.canvasId() || m_canvas_rect != canvas_rect ||
        m_canvas_overlap_count != info.canvasOverlapCount() ||
        m_selectable_target != info.selectableTarget()) {
        m_canvas_id            = info.canvasId();
        m_canvas_rect          = std::move(canvas_rect);
        m_canvas_overlap_count = info.canvasOverlapCount();
        m_selectable_target    = info.selectableTarget();
        Q_EMIT canvasChanged();
    }
}

void Display::updatePlaylistStatus(const proto::PlaylistDisplayStatus* status) {
    const auto new_active = status ? static_cast<qint64>(status->activeId()) : 0;
    auto       new_status = playlistStatusFromPb(status);
    if (m_active_playlist_id == new_active && m_playlist_status == new_status) return;
    m_active_playlist_id = new_active;
    m_playlist_status    = std::move(new_status);
    Q_EMIT playlistStatusChanged();
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

auto Canvas::rectFromPb(const proto::CanvasRect& rect) -> QVariantMap {
    QVariantMap out;
    out[u"x"_s]      = rect.x();
    out[u"y"_s]      = rect.y();
    out[u"width"_s]  = rect.width();
    out[u"height"_s] = rect.height();
    return out;
}

auto Canvas::membersFromPb(const proto::CanvasInfo& info) -> QVariantList {
    QVariantList out;
    out.reserve(info.members().size());
    for (const auto& member : info.members()) {
        QVariantMap row;
        row[u"settingsKey"_s]  = member.settingsKey();
        const auto authored    = member.hasRect() ? rectFromPb(member.rect()) : QVariantMap {};
        row[u"authoredRect"_s] = authored;
        row[u"rect"_s] = member.hasEffectiveRect() ? rectFromPb(member.effectiveRect()) : authored;
        QVariantMap minimum_scale_to;
        if (member.hasMinimumScaleTo()) {
            minimum_scale_to[u"width"_s]  = member.minimumScaleTo().width();
            minimum_scale_to[u"height"_s] = member.minimumScaleTo().height();
        }
        row[u"minimumScaleTo"_s] = minimum_scale_to;
        row[u"aspectLocked"_s]   = member.aspectLocked();
        QVariantList display_ids;
        display_ids.reserve(member.displayIds().size());
        for (const auto display_id : member.displayIds()) {
            display_ids.append(QVariant::fromValue<quint64>(display_id));
        }
        row[u"displayIds"_s]  = display_ids;
        row[u"onlineCount"_s] = display_ids.size();
        row[u"overlap"_s]     = display_ids.size() > 1;
        out.append(row);
    }
    return out;
}

auto Canvas::layoutOverrideFromPb(const proto::CanvasInfo& info) -> QVariantMap {
    QVariantMap out;
    if (! info.hasLayoutOverride()) return out;
    const auto& layout    = info.layoutOverride();
    out[u"fillmodeSet"_s] = layout.fillmodeSet();
    out[u"fillmode"_s]    = static_cast<int>(layout.fillmode());
    out[u"locationSet"_s] = layout.locationSet();
    out[u"locationX"_s]   = layout.locationX();
    out[u"locationY"_s]   = layout.locationY();
    out[u"rotationSet"_s] = layout.rotationSet();
    out[u"flipSet"_s]     = layout.flipSet();
    out[u"flip"_s]        = static_cast<int>(layout.flip());
    out[u"rotation"_s]    = static_cast<int>(layout.rotation());
    return out;
}

auto Canvas::effectiveLayoutFromPb(const proto::CanvasInfo& info) -> QVariantMap {
    QVariantMap out;
    if (! info.hasEffectiveLayout()) return out;
    const auto& layout  = info.effectiveLayout();
    out[u"fillmode"_s]  = static_cast<int>(layout.fillmode());
    out[u"locationX"_s] = layout.locationX();
    out[u"locationY"_s] = layout.locationY();
    out[u"rotation"_s]  = static_cast<int>(layout.rotation());
    out[u"flip"_s]      = static_cast<int>(layout.flip());
    return out;
}

Canvas::Canvas(const proto::CanvasInfo& info, QObject* parent)
    : QObject(parent), m_id(info.canvasId()) {
    updateFrom(info);
}

void Canvas::updateFrom(const proto::CanvasInfo& info) {
    rstd_assert(info.canvasId() == m_id, "Canvas::updateFrom id mismatch");
    auto       members          = membersFromPb(info);
    auto       extent           = info.hasExtent() ? rectFromPb(info.extent()) : QVariantMap {};
    const auto width            = info.hasExtent() ? info.extent().width() : 0;
    const auto height           = info.hasExtent() ? info.extent().height() : 0;
    auto       layout_override  = layoutOverrideFromPb(info);
    auto       effective_layout = effectiveLayoutFromPb(info);
    int        online_count     = 0;
    for (const auto& member : members) {
        online_count += member.toMap().value(u"onlineCount"_s).toInt();
    }
    if (m_name == info.name() && m_members == members && m_extent == extent && m_width == width &&
        m_height == height && m_layout_override == layout_override &&
        m_effective_layout == effective_layout && m_wallpaper_id == info.wallpaperId() &&
        m_revision == info.revision() && m_online_count == online_count) {
        return;
    }
    m_name             = info.name();
    m_members          = std::move(members);
    m_extent           = std::move(extent);
    m_width            = width;
    m_height           = height;
    m_layout_override  = std::move(layout_override);
    m_effective_layout = std::move(effective_layout);
    m_wallpaper_id     = info.wallpaperId();
    m_revision         = info.revision();
    m_online_count     = online_count;
    Q_EMIT changed();
}

void Canvas::updateRuntime(const QList<Display*>& displays) {
    std::map<QString, QVariantMap> links_by_renderer;
    QVariantList                   conditions;
    qint64                         active_playlist_id { 0 };
    QVariantMap                    playlist_status;
    for (const auto* display : displays) {
        if (! display || display->canvasId() != m_id) continue;

        for (const auto& value : display->links()) {
            auto       link  = value.toMap();
            const auto key   = link.value(u"rendererId"_s).toString();
            auto [it, added] = links_by_renderer.emplace(key, link);
            if (! added && link.value(u"active"_s).toBool()) it->second[u"active"_s] = true;
        }
        for (const auto& condition : display->runtimeConditions()) {
            if (! conditions.contains(condition)) conditions.append(condition);
        }
        if (active_playlist_id == 0 && display->activePlaylistId() > 0) {
            active_playlist_id = display->activePlaylistId();
            playlist_status    = display->playlistStatus();
        }
    }

    QVariantList links;
    links.reserve(static_cast<qsizetype>(links_by_renderer.size()));
    for (auto& entry : links_by_renderer) links.append(std::move(entry.second));

    if (m_links == links && m_active_playlist_id == active_playlist_id &&
        m_playlist_status == playlist_status && m_runtime_conditions == conditions) {
        return;
    }
    m_links              = std::move(links);
    m_active_playlist_id = active_playlist_id;
    m_playlist_status    = std::move(playlist_status);
    m_runtime_conditions = std::move(conditions);
    Q_EMIT runtimeChanged();
}

// ---------------------------------------------------------------------------
// DisplayManager
// ---------------------------------------------------------------------------

static auto dm_instance(DisplayManager* in = nullptr) -> DisplayManager* {
    static DisplayManager* instance { in };
    if (in && instance != in) instance = in;
    return instance;
}

DisplayManager::DisplayManager(QObject* parent): QObject(parent) { dm_instance(this); }

DisplayManager::~DisplayManager() {
    if (dm_instance() == this) {
        // best-effort: leave static pointer dangling only if app is tearing
        // down anyway; no other lifecycle consumer.
    }
}

auto DisplayManager::instance() -> DisplayManager* { return dm_instance(); }

auto DisplayManager::displays() const -> QVariantList {
    QVariantList out;
    out.reserve(m_ordered.size());
    for (auto* d : m_ordered) out.append(QVariant::fromValue(d));
    return out;
}

auto DisplayManager::canvases() const -> QVariantList {
    QVariantList out;
    out.reserve(m_canvases.size());
    for (auto* canvas : m_canvases) out.append(QVariant::fromValue(canvas));
    return out;
}

auto DisplayManager::hasActivePlaylistDisplays() const -> bool {
    for (const auto* display : m_ordered) {
        if (display->activePlaylistId() > 0) return true;
    }
    return false;
}

auto DisplayManager::get(quint64 id) const -> Display* {
    auto it = m_by_id.find(id);
    return (it == m_by_id.end()) ? nullptr : it->second;
}

auto DisplayManager::getCanvas(const QString& id) const -> Canvas* {
    auto it = m_canvas_by_id.find(id);
    return it == m_canvas_by_id.end() ? nullptr : it->second;
}

void DisplayManager::replaceAll(const QList<proto::DisplayInfo>& list) {
    const auto                  had_active = hasActivePlaylistDisplays();
    std::map<quint64, Display*> next_by_id;
    QList<Display*>             next_ordered;
    next_ordered.reserve(list.size());

    for (const auto& info : list) {
        auto id = info.displayId();
        auto it = m_by_id.find(id);
        if (it != m_by_id.end()) {
            it->second->updateFrom(info);
            next_by_id[id] = it->second;
            next_ordered.append(it->second);
            m_by_id.erase(it);
        } else {
            auto* d        = new Display(info, this);
            next_by_id[id] = d;
            next_ordered.append(d);
        }
    }
    // Anything left in m_by_id was not in the new snapshot → drop it.
    for (auto& [id, d] : m_by_id) d->deleteLater();
    m_by_id.clear();

    // Stable ordering by id for UI determinism.
    std::sort(next_ordered.begin(), next_ordered.end(), [](Display* a, Display* b) {
        return a->id() < b->id();
    });

    m_ordered = std::move(next_ordered);
    m_by_id   = std::move(next_by_id);
    refreshCanvasRuntime();
    if (had_active != hasActivePlaylistDisplays()) Q_EMIT playlistStatusChanged();
    Q_EMIT displaysChanged();
}

void DisplayManager::upsert(const proto::DisplayInfo& info) {
    auto id = info.displayId();
    auto it = m_by_id.find(id);
    if (it != m_by_id.end()) {
        it->second->updateFrom(info);
        refreshCanvasRuntime();
        return;
    }
    auto* d     = new Display(info, this);
    m_by_id[id] = d;
    auto pos = std::upper_bound(m_ordered.begin(), m_ordered.end(), id, [](quint64 v, Display* x) {
        return v < x->id();
    });
    m_ordered.insert(pos, d);
    refreshCanvasRuntime();
    Q_EMIT displaysChanged();
}

void DisplayManager::remove(quint64 id) {
    auto it = m_by_id.find(id);
    if (it == m_by_id.end()) return;
    const auto had_active = hasActivePlaylistDisplays();
    auto*      d          = it->second;
    m_by_id.erase(it);
    m_ordered.removeOne(d);
    d->deleteLater();
    refreshCanvasRuntime();
    if (had_active != hasActivePlaylistDisplays()) Q_EMIT playlistStatusChanged();
    Q_EMIT displaysChanged();
}

void DisplayManager::replaceCanvases(const QList<proto::CanvasInfo>& list, quint64 revision) {
    std::map<QString, Canvas*> next_by_id;
    QList<Canvas*>             next_canvases;
    next_canvases.reserve(list.size());
    for (const auto& info : list) {
        auto  it     = m_canvas_by_id.find(info.canvasId());
        auto* canvas = it == m_canvas_by_id.end() ? new Canvas(info, this) : it->second;
        if (it != m_canvas_by_id.end()) {
            canvas->updateFrom(info);
            m_canvas_by_id.erase(it);
        }
        next_by_id[info.canvasId()] = canvas;
        next_canvases.append(canvas);
    }
    for (auto& [id, canvas] : m_canvas_by_id) canvas->deleteLater();
    std::sort(next_canvases.begin(), next_canvases.end(), [](Canvas* a, Canvas* b) {
        if (a->name() == b->name()) return a->id() < b->id();
        return a->name() < b->name();
    });
    m_canvas_by_id    = std::move(next_by_id);
    m_canvases        = std::move(next_canvases);
    m_canvas_revision = revision;
    refreshCanvasRuntime();
    Q_EMIT canvasesChanged();
}

void DisplayManager::replacePlaylistStatuses(const QList<proto::PlaylistDisplayStatus>& list) {
    const auto                                             had_active = hasActivePlaylistDisplays();
    std::map<quint64, const proto::PlaylistDisplayStatus*> by_id;
    for (const auto& status : list) {
        by_id[status.displayId()] = &status;
    }
    for (auto* display : m_ordered) {
        auto it = by_id.find(display->id());
        display->updatePlaylistStatus(it == by_id.end() ? nullptr : it->second);
    }
    refreshCanvasRuntime();
    if (had_active != hasActivePlaylistDisplays()) Q_EMIT playlistStatusChanged();
}

void DisplayManager::refreshCanvasRuntime() {
    for (auto* canvas : m_canvases) canvas->updateRuntime(m_ordered);
}

void DisplayManager::attachTo(Backend* backend) {
    connect(
        backend, &Backend::eventReceived, this, &DisplayManager::handleEvent, Qt::QueuedConnection);
}

void DisplayManager::handleEvent(const proto::Event& evt) {
    if (evt.hasDisplaySnapshot()) {
        const auto& snap = evt.displaySnapshot();
        replaceAll(snap.displays());
    } else if (evt.hasDisplayChanged()) {
        upsert(evt.displayChanged().display());
    } else if (evt.hasDisplayRemoved()) {
        remove(evt.displayRemoved().displayId());
    } else if (evt.hasPlaylistChanged()) {
        replacePlaylistStatuses(evt.playlistChanged().displays());
    } else if (evt.hasCanvasSnapshot()) {
        replaceCanvases(evt.canvasSnapshot().canvases(), evt.canvasSnapshot().revision());
    }
}

} // namespace waywallen

#include "waywallen/objmodel/display.moc.cpp"
