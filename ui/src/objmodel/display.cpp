module;
#include "waywallen/objmodel/display.moc.h"

#undef assert
#include <rstd/macro.hpp>

module waywallen;
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
    return m;
}

Display::Display(const proto::DisplayInfo& info, QObject* parent)
    : QObject(parent),
      m_id(info.displayId()),
      m_name(info.name()),
      m_alias(info.alias()),
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
      m_lock_wallpaper(info.lockWallpaper()),
      m_has_lock_screen(info.hasLockScreen()) {}

void Display::updateFrom(const proto::DisplayInfo& info) {
    rstd_assert(info.displayId() == m_id, "Display::updateFrom id mismatch");

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
    if (m_lock_wallpaper != info.lockWallpaper()) {
        m_lock_wallpaper = info.lockWallpaper();
        Q_EMIT lockWallpaperChanged();
    }
    if (m_has_lock_screen != info.hasLockScreen()) {
        m_has_lock_screen = info.hasLockScreen();
        Q_EMIT hasLockScreenChanged();
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
    if (had_active != hasActivePlaylistDisplays()) Q_EMIT playlistStatusChanged();
    Q_EMIT displaysChanged();
}

void DisplayManager::upsert(const proto::DisplayInfo& info) {
    auto id = info.displayId();
    auto it = m_by_id.find(id);
    if (it != m_by_id.end()) {
        it->second->updateFrom(info);
        return;
    }
    auto* d     = new Display(info, this);
    m_by_id[id] = d;
    auto pos = std::upper_bound(m_ordered.begin(), m_ordered.end(), id, [](quint64 v, Display* x) {
        return v < x->id();
    });
    m_ordered.insert(pos, d);
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
    if (had_active != hasActivePlaylistDisplays()) Q_EMIT playlistStatusChanged();
    Q_EMIT displaysChanged();
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
    if (had_active != hasActivePlaylistDisplays()) Q_EMIT playlistStatusChanged();
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
    }
}

} // namespace waywallen

#include "waywallen/objmodel/display.moc.cpp"
