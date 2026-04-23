module;
#include "waywallen/renderer.moc.h"

#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :renderer;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;
using namespace rstd::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

Renderer::Renderer(const proto::RendererInstance& info, QObject* parent)
    : QObject(parent),
      m_id(info.rendererId()),
      m_fps(info.fps()),
      m_status(info.status()),
      m_name(info.name()),
      m_pid(info.pid()) {}

void Renderer::updateFrom(const proto::RendererInstance& info) {
    assert(info.rendererId() == m_id, "Renderer::updateFrom id mismatch");

    if (m_fps != info.fps()) {
        m_fps = info.fps();
        Q_EMIT fpsChanged();
    }
    if (m_status != info.status()) {
        m_status = info.status();
        Q_EMIT statusChanged();
    }
    if (m_name != info.name()) {
        m_name = info.name();
        Q_EMIT nameChanged();
    }
    if (m_pid != info.pid()) {
        m_pid = info.pid();
        Q_EMIT pidChanged();
    }
}

// ---------------------------------------------------------------------------
// RendererManager
// ---------------------------------------------------------------------------

static auto rm_instance(RendererManager* in = nullptr) -> RendererManager* {
    static RendererManager* instance { in };
    if (in && instance != in) instance = in;
    return instance;
}

RendererManager::RendererManager(QObject* parent): QObject(parent) { rm_instance(this); }

RendererManager::~RendererManager() {
    if (rm_instance() == this) {
        // best-effort: leave static pointer dangling only if app is tearing
        // down anyway; no other lifecycle consumer.
    }
}

auto RendererManager::instance() -> RendererManager* { return rm_instance(); }

auto RendererManager::renderers() const -> QVariantList {
    QVariantList out;
    out.reserve(m_ordered.size());
    for (auto* r : m_ordered) out.append(QVariant::fromValue(r));
    return out;
}

auto RendererManager::get(const QString& id) const -> Renderer* {
    auto it = m_by_id.find(id);
    return (it == m_by_id.end()) ? nullptr : it->second;
}

void RendererManager::replaceAll(const QList<proto::RendererInstance>& list) {
    cppstd::map<QString, Renderer*> next_by_id;
    QList<Renderer*>                next_ordered;
    next_ordered.reserve(list.size());

    for (const auto& info : list) {
        auto id = info.rendererId();
        auto it = m_by_id.find(id);
        if (it != m_by_id.end()) {
            it->second->updateFrom(info);
            next_by_id[id] = it->second;
            next_ordered.append(it->second);
            m_by_id.erase(it);
        } else {
            auto* r        = new Renderer(info, this);
            next_by_id[id] = r;
            next_ordered.append(r);
        }
    }
    // Anything left in m_by_id was not in the new snapshot → drop it.
    for (auto& [id, r] : m_by_id) r->deleteLater();
    m_by_id.clear();

    // Stable ordering by id for UI determinism.
    std::sort(next_ordered.begin(), next_ordered.end(), [](Renderer* a, Renderer* b) {
        return a->id() < b->id();
    });

    m_ordered = std::move(next_ordered);
    m_by_id   = std::move(next_by_id);
    Q_EMIT renderersChanged();
}

void RendererManager::upsert(const proto::RendererInstance& info) {
    auto id = info.rendererId();
    auto it = m_by_id.find(id);
    if (it != m_by_id.end()) {
        it->second->updateFrom(info);
        return;
    }
    auto* r     = new Renderer(info, this);
    m_by_id[id] = r;
    auto pos    = std::upper_bound(
        m_ordered.begin(), m_ordered.end(), id, [](const QString& v, Renderer* x) {
            return v < x->id();
        });
    m_ordered.insert(pos, r);
    Q_EMIT renderersChanged();
}

void RendererManager::remove(const QString& id) {
    auto it = m_by_id.find(id);
    if (it == m_by_id.end()) return;
    auto* r = it->second;
    m_by_id.erase(it);
    m_ordered.removeOne(r);
    r->deleteLater();
    Q_EMIT renderersChanged();
}

void RendererManager::attachTo(Backend* backend) {
    connect(backend, &Backend::eventReceived, this, &RendererManager::handleEvent,
            Qt::QueuedConnection);
}

void RendererManager::handleEvent(const proto::Event& evt) {
    if (evt.hasRendererSnapshot()) {
        const auto& snap = evt.rendererSnapshot();
        replaceAll(snap.renderers());
    } else if (evt.hasRendererChanged()) {
        upsert(evt.rendererChanged().renderer());
    } else if (evt.hasRendererRemoved()) {
        remove(evt.rendererRemoved().rendererId());
    }
}

} // namespace waywallen

#include "waywallen/renderer.moc.cpp"
