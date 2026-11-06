module;
#include "waywallen/objmodel/presentation.moc.h"

module waywallen;
import qextra;
import :presentation;

using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

NowPlayingModel::NowPlayingModel(QObject* parent)
    : kstore::QGadgetListModel(this, parent), list_crtp_t() {}

void NowPlayingModel::replace(QList<model::NowPlayingItem> items) {
    const auto previous_count = count();
    sync(std::move(items));
    if (previous_count != count()) Q_EMIT countChanged();
}

PresentationManager::PresentationManager(QObject* parent): QObject(parent), m_model(this) {
    m_model.set_store(&m_model, m_store);
    connect(&m_model, &NowPlayingModel::countChanged, this, &PresentationManager::countChanged);
}

void PresentationManager::attachTo(Backend* backend, DisplayManager* displays) {
    m_backend  = backend;
    m_displays = displays;
    connect(backend,
            &Backend::eventReceived,
            this,
            &PresentationManager::handleEvent,
            Qt::QueuedConnection);
    connect(backend, &Backend::disconnected, this, [this] {
        ++m_request_generation;
        m_requests.cancel();
        m_presentations.clear();
        m_wallpapers.clear();
        rebuildRows();
    });
    connect(displays, &DisplayManager::displaysChanged, this, &PresentationManager::rebuildRows);
    connect(displays, &DisplayManager::canvasesChanged, this, &PresentationManager::rebuildRows);
}

void PresentationManager::handleEvent(const proto::Event& event) {
    if (event.hasWallpaperPresentationSnapshot()) {
        replaceSnapshot(event.wallpaperPresentationSnapshot());
    }
}

void PresentationManager::replaceSnapshot(const proto::WallpaperPresentationSnapshot& snapshot) {
    m_presentations = snapshot.presentations();
    ++m_request_generation;
    m_requests.cancel();
    QList<AppStore::wallpaper_store::store_item_type> wallpapers;
    auto&                                             store = AppStore::instance()->wallpapers;
    for (const auto& presentation : m_presentations) {
        if (auto wallpaper = store.store_item(presentation.wallpaperId())) {
            wallpapers.append(std::move(*wallpaper));
        }
    }
    m_wallpapers = std::move(wallpapers);
    rebuildRows();
    requestMissingWallpapers();
}

auto PresentationManager::targetSummary(const proto::WallpaperPresentationInfo& presentation) const
    -> QString {
    QStringList labels;
    for (const auto& target : presentation.targets()) {
        if (target.hasDisplayId()) {
            const auto  display_id = target.displayId();
            const auto* display    = m_displays ? m_displays->get(display_id) : nullptr;
            labels.append(display ? display->displayLabel() : tr("Display #%1").arg(display_id));
        } else if (target.hasCanvasId()) {
            const auto  canvas_id = target.canvasId();
            const auto* canvas    = m_displays ? m_displays->getCanvas(canvas_id) : nullptr;
            labels.append(canvas ? canvas->name() : tr("Canvas %1").arg(canvas_id));
        }
    }
    labels.removeDuplicates();
    return labels.join(QStringLiteral(" · "));
}

void PresentationManager::rebuildRows() {
    QList<model::NowPlayingItem> rows;
    rows.reserve(m_presentations.size());
    auto& wallpapers = AppStore::instance()->wallpapers;
    for (const auto& presentation : m_presentations) {
        model::NowPlayingItem row;
        row.wallpaperId   = presentation.wallpaperId();
        row.targetSummary = targetSummary(presentation);
        row.state         = static_cast<int>(presentation.state());
        if (const auto* wallpaper = wallpapers.store_query(row.wallpaperId)) {
            row.wallpaper = *wallpaper;
        } else {
            row.wallpaper.setId_proto(row.wallpaperId);
            row.wallpaper.setName(row.wallpaperId);
        }
        rows.append(std::move(row));
    }
    m_model.replace(std::move(rows));
}

void PresentationManager::requestMissingWallpapers() {
    if (! m_backend) return;

    QStringList missing;
    auto&       wallpapers = AppStore::instance()->wallpapers;
    for (const auto& presentation : m_presentations) {
        if (! wallpapers.store_query(presentation.wallpaperId())) {
            missing.append(presentation.wallpaperId());
        }
    }
    missing.removeDuplicates();
    if (missing.isEmpty()) return;

    auto request = proto::Request {};
    auto lookup  = proto::WallpaperLookupRequest {};
    lookup.setWallpaperIds(missing);
    request.setWallpaperLookup(std::move(lookup));

    const auto generation = m_request_generation;
    auto       self       = QWatcher { this };
    auto*      backend    = m_backend;
    m_requests.spawn(
        [self, backend, request = std::move(request), generation]() mutable -> task<void> {
            auto result = co_await backend->send(std::move(request));
            if (! co_await QAsyncResult::qexecutor()) co_return;
            if (! self || generation != self->m_request_generation) co_return;
            if (! result) {
                qWarning("wallpaper presentation lookup failed: %s",
                         qPrintable(result.unwrap_err_unchecked()));
                co_return;
            }

            auto        response = result.unwrap_unchecked();
            auto&       store    = AppStore::instance()->wallpapers;
            QStringList changed;
            for (const auto& wallpaper : response.wallpaperLookup().wallpapers()) {
                self->m_wallpapers.append(store.store_insert(wallpaper).first);
                changed.append(wallpaper.id_proto());
            }
            if (! changed.isEmpty()) store.store_changed_callback(changed);
            self->rebuildRows();
            co_return;
        });
}

} // namespace waywallen

#include "waywallen/objmodel/presentation.moc.cpp"
