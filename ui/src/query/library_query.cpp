module;
#include "waywallen/query/library_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
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
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setLibraryList(proto::LibraryListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [](const proto::Response& rsp) {
            if (auto* lm = LibraryManager::instance()) {
                lm->replaceAll(rsp.libraryList().libraries());
            }
        });
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

auto LibraryAddQuery::name() const -> const QString& { return m_name; }
void LibraryAddQuery::setName(const QString& v) {
    if (m_name != v) {
        m_name = v;
        Q_EMIT nameChanged();
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
    if (! m_name.trimmed().isEmpty()) inner.setName(m_name.trimmed());
    req.setLibraryAdd(std::move(inner));

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
// LibraryRenameQuery
// ---------------------------------------------------------------------------

LibraryRenameQuery::LibraryRenameQuery(QObject* parent): Query(parent) {}

auto LibraryRenameQuery::libraryId() const -> qint64 { return m_library_id; }
void LibraryRenameQuery::setLibraryId(qint64 v) {
    if (m_library_id != v) {
        m_library_id = v;
        Q_EMIT libraryIdChanged();
    }
}

auto LibraryRenameQuery::name() const -> const QString& { return m_name; }
void LibraryRenameQuery::setName(const QString& v) {
    if (m_name != v) {
        m_name = v;
        Q_EMIT nameChanged();
    }
}

void LibraryRenameQuery::reload() {
    if (m_library_id <= 0 || m_name.trimmed().isEmpty()) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();
    auto req     = proto::Request {};
    auto inner   = proto::LibraryRenameRequest {};
    inner.setId_proto(m_library_id);
    inner.setName(m_name.trimmed());
    req.setLibraryRename(std::move(inner));

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
// LibraryAutoDetectQuery
// ---------------------------------------------------------------------------

LibraryAutoDetectQuery::LibraryAutoDetectQuery(QObject* parent): Query(parent) {}

auto LibraryAutoDetectQuery::addedCount() const -> qint32 { return m_added_count; }

void LibraryAutoDetectQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setLibraryAutoDetect(proto::LibraryAutoDetectRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            auto added = static_cast<qint32>(rsp.libraryAutoDetect().added().size());
            if (self->m_added_count != added) {
                self->m_added_count = added;
                Q_EMIT self->addedCountChanged();
            }
        });
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
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [](const proto::Response&) {
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/library_query.moc.cpp"
