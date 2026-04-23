module;
#include "waywallen/query/library_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.library;
import :app;
import :library;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

// ---------------------------------------------------------------------------
// LibraryListQuery
// ---------------------------------------------------------------------------

LibraryListQuery::LibraryListQuery(QObject* parent): Query(parent) {}

void LibraryListQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setLibraryList(proto::LibraryListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        if (! result) {
            self->setError(result.unwrap_err());
            co_return;
        }
        auto  rsp      = result.unwrap();
        auto& list_rsp = rsp.libraryList();

        if (auto* lm = LibraryManager::instance()) {
            lm->replaceAll(list_rsp.libraries());
        }

        self->setStatus(Status::Finished);
        co_return;
    });
}

// ---------------------------------------------------------------------------
// LibraryAddQuery
// ---------------------------------------------------------------------------

LibraryAddQuery::LibraryAddQuery(QObject* parent): Query(parent) {}

auto LibraryAddQuery::path() const -> const QString& { return m_path; }
void LibraryAddQuery::setPath(const QString& v) {
    if (m_path != v) {
        m_path = v;
        Q_EMIT pathChanged();
    }
}

auto LibraryAddQuery::pluginName() const -> const QString& { return m_plugin_name; }
void LibraryAddQuery::setPluginName(const QString& v) {
    if (m_plugin_name != v) {
        m_plugin_name = v;
        Q_EMIT pluginNameChanged();
    }
}

void LibraryAddQuery::reload() {
    if (m_path.isEmpty() || m_plugin_name.isEmpty()) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::LibraryAddRequest {};
    inner.setPath(m_path);
    inner.setPluginName(m_plugin_name);
    req.setLibraryAdd(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        if (! result) {
            self->setError(result.unwrap_err());
            co_return;
        }
        self->setStatus(Status::Finished);
        co_return;
    });
}

// ---------------------------------------------------------------------------
// LibraryAutoDetectQuery
// ---------------------------------------------------------------------------

LibraryAutoDetectQuery::LibraryAutoDetectQuery(QObject* parent): Query(parent) {}

auto LibraryAutoDetectQuery::addedCount() const -> qint32 { return m_added_count; }

void LibraryAutoDetectQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setLibraryAutoDetect(proto::LibraryAutoDetectRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        if (! result) {
            self->setError(result.unwrap_err());
            co_return;
        }
        auto  rsp   = result.unwrap();
        auto& inner = rsp.libraryAutoDetect();
        auto  added = static_cast<qint32>(inner.added().size());
        if (self->m_added_count != added) {
            self->m_added_count = added;
            Q_EMIT self->addedCountChanged();
        }
        self->setStatus(Status::Finished);
        co_return;
    });
}

// ---------------------------------------------------------------------------
// LibraryRemoveQuery
// ---------------------------------------------------------------------------

LibraryRemoveQuery::LibraryRemoveQuery(QObject* parent): Query(parent) {}

auto LibraryRemoveQuery::libraryId() const -> qint64 { return m_library_id; }
void LibraryRemoveQuery::setLibraryId(qint64 v) {
    if (m_library_id != v) {
        m_library_id = v;
        Q_EMIT libraryIdChanged();
    }
}

void LibraryRemoveQuery::reload() {
    if (m_library_id == 0) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::LibraryRemoveRequest {};
    inner.setId_proto(m_library_id);
    req.setLibraryRemove(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        if (! result) {
            self->setError(result.unwrap_err());
            co_return;
        }
        self->setStatus(Status::Finished);
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/library_query.moc.cpp"
