module;
#include "waywallen/library.moc.h"

#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :library;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;
using namespace rstd::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

Library::Library(const proto::LibraryInstance& info, QObject* parent)
    : QObject(parent),
      m_id(info.id_proto()),
      m_path(info.path()),
      m_plugin_name(info.pluginName()) {}

void Library::updateFrom(const proto::LibraryInstance& info) {
    assert(info.id_proto() == m_id, "Library::updateFrom id mismatch");

    if (m_path != info.path()) {
        m_path = info.path();
        Q_EMIT pathChanged();
    }
    if (m_plugin_name != info.pluginName()) {
        m_plugin_name = info.pluginName();
        Q_EMIT pluginNameChanged();
    }
}

// ---------------------------------------------------------------------------
// LibraryManager
// ---------------------------------------------------------------------------

static auto lm_instance(LibraryManager* in = nullptr) -> LibraryManager* {
    static LibraryManager* instance { in };
    if (in && instance != in) instance = in;
    return instance;
}

LibraryManager::LibraryManager(QObject* parent): QObject(parent) {
    lm_instance(this);
}

LibraryManager::~LibraryManager() {
    if (lm_instance() == this) {
        lm_instance(nullptr);
    }
}

auto LibraryManager::instance() -> LibraryManager* {
    return lm_instance();
}

auto LibraryManager::libraries() const -> QVariantList {
    QVariantList out;
    out.reserve(m_ordered.size());
    for (auto* r : m_ordered) out.append(QVariant::fromValue(r));
    return out;
}

auto LibraryManager::get(qint64 id) const -> Library* {
    auto it = m_by_id.find(id);
    return (it == m_by_id.end()) ? nullptr : it->second;
}

void LibraryManager::replaceAll(const QList<proto::LibraryInstance>& list) {
    cppstd::map<qint64, Library*> next_by_id;
    QList<Library*>               next_ordered;
    next_ordered.reserve(list.size());

    for (const auto& info : list) {
        auto id = info.id_proto();
        auto it = m_by_id.find(id);
        if (it != m_by_id.end()) {
            it->second->updateFrom(info);
            next_by_id[id] = it->second;
            next_ordered.append(it->second);
            m_by_id.erase(it);
        } else {
            auto* r        = new Library(info, this);
            next_by_id[id] = r;
            next_ordered.append(r);
        }
    }
    // Anything left in m_by_id was not in the new snapshot → drop it.
    for (auto& [id, r] : m_by_id) r->deleteLater();
    m_by_id.clear();

    // Stable ordering by id for UI determinism.
    std::sort(next_ordered.begin(), next_ordered.end(), [](Library* a, Library* b) {
        return a->id() < b->id();
    });

    m_ordered = std::move(next_ordered);
    m_by_id   = std::move(next_by_id);
    Q_EMIT librariesChanged();
}

void LibraryManager::upsert(const proto::LibraryInstance& info) {
    auto id = info.id_proto();
    auto it = m_by_id.find(id);
    if (it != m_by_id.end()) {
        it->second->updateFrom(info);
        return;
    }
    auto* r     = new Library(info, this);
    m_by_id[id] = r;
    auto pos    = std::upper_bound(
        m_ordered.begin(), m_ordered.end(), id, [](qint64 v, Library* x) {
            return v < x->id();
        });
    m_ordered.insert(pos, r);
    Q_EMIT librariesChanged();
}

void LibraryManager::remove(qint64 id) {
    auto it = m_by_id.find(id);
    if (it == m_by_id.end()) return;
    auto* r = it->second;
    m_by_id.erase(it);
    m_ordered.removeOne(r);
    r->deleteLater();
    Q_EMIT librariesChanged();
}

void LibraryManager::attachTo(Backend* backend) {
    connect(backend, &Backend::eventReceived, this, &LibraryManager::handleEvent,
            Qt::QueuedConnection);
}

void LibraryManager::handleEvent(const proto::Event& evt) {
    if (evt.hasLibrarySnapshot()) {
        const auto& snap = evt.librarySnapshot();
        replaceAll(snap.libraries());
    } else if (evt.hasLibraryChanged()) {
        upsert(evt.libraryChanged().library());
    } else if (evt.hasLibraryRemoved()) {
        remove(evt.libraryRemoved().id_proto());
    }
}

} // namespace waywallen

#include "waywallen/library.moc.cpp"
