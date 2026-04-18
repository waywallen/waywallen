module;
#include "waywallen/display.moc.h"

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

Display::Display(const proto::DisplayInfo& info, QObject* parent)
    : QObject(parent),
      m_id(info.displayId()),
      m_name(info.name()),
      m_width(info.width()),
      m_height(info.height()),
      m_refresh_mhz(info.refreshMhz()),
      m_links(linksFromPb(info)) {}

void Display::updateFrom(const proto::DisplayInfo& info) {
    assert(info.displayId() == m_id, "Display::updateFrom id mismatch");

    if (m_name != info.name()) {
        m_name = info.name();
        Q_EMIT nameChanged();
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

auto DisplayManager::create(QQmlEngine*, QJSEngine*) -> DisplayManager* {
    auto* m = dm_instance();
    assert(m != nullptr, "DisplayManager must be constructed by App before QML loads");
    QJSEngine::setObjectOwnership(m, QJSEngine::CppOwnership);
    return m;
}

auto DisplayManager::displays() const -> QVariantList {
    QVariantList out;
    out.reserve(m_ordered.size());
    for (auto* d : m_ordered) out.append(QVariant::fromValue(d));
    return out;
}

auto DisplayManager::get(quint64 id) const -> Display* {
    auto it = m_by_id.find(id);
    return (it == m_by_id.end()) ? nullptr : it->second;
}

void DisplayManager::replaceAll(const QList<proto::DisplayInfo>& list) {
    cppstd::map<quint64, Display*> next_by_id;
    QList<Display*>                next_ordered;
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
            auto* d         = new Display(info, this);
            next_by_id[id]  = d;
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
    Q_EMIT displaysChanged();
}

void DisplayManager::upsert(const proto::DisplayInfo& info) {
    auto id = info.displayId();
    auto it = m_by_id.find(id);
    if (it != m_by_id.end()) {
        it->second->updateFrom(info);
        return;
    }
    auto* d    = new Display(info, this);
    m_by_id[id] = d;
    auto pos   = std::upper_bound(
        m_ordered.begin(), m_ordered.end(), id, [](quint64 v, Display* x) {
            return v < x->id();
        });
    m_ordered.insert(pos, d);
    Q_EMIT displaysChanged();
}

void DisplayManager::remove(quint64 id) {
    auto it = m_by_id.find(id);
    if (it == m_by_id.end()) return;
    auto* d = it->second;
    m_by_id.erase(it);
    m_ordered.removeOne(d);
    d->deleteLater();
    Q_EMIT displaysChanged();
}

void DisplayManager::attachTo(Backend* backend) {
    connect(backend, &Backend::eventReceived, this, &DisplayManager::handleEvent,
            Qt::QueuedConnection);
}

void DisplayManager::handleEvent(const proto::Event& evt) {
    if (evt.hasDisplaySnapshot()) {
        const auto& snap = evt.displaySnapshot();
        replaceAll(snap.displays());
    } else if (evt.hasDisplayChanged()) {
        upsert(evt.displayChanged().display());
    } else if (evt.hasDisplayRemoved()) {
        remove(evt.displayRemoved().displayId());
    }
}

} // namespace waywallen

#include "waywallen/display.moc.cpp"
