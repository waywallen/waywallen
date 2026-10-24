module;
#include "waywallen/query/playlist_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
import :query.playlist;
import :app;
import :msg.store;

using namespace Qt::Literals::StringLiterals;

namespace proto = waywallen::control::v1;
using namespace qextra::prelude;

namespace waywallen
{

static auto playlist_to_map(const proto::PlaylistSummary& playlist) -> QVariantMap {
    QVariantMap map;
    map[u"id"_s]                    = static_cast<qint64>(playlist.id_proto());
    map[u"name"_s]                  = playlist.name();
    map[u"mode"_s]                  = static_cast<int>(playlist.mode());
    map[u"intervalSecs"_s]          = playlist.intervalSecs();
    map[u"synchronizedSelection"_s] = playlist.synchronizedSelection();
    map[u"itemCount"_s]             = playlist.itemCount();
    map[u"revision"_s]              = static_cast<qint64>(playlist.revision());
    map[u"sourceKind"_s]            = playlist.sourceKind();
    if (playlist.hasSource() && playlist.source().hasSmart())
        map[u"smartSelection"_s] = QVariant::fromValue(playlist.source().smart());
    QStringList entry_ids;
    entry_ids.reserve(playlist.entryIds().size());
    for (const auto& entry_id : playlist.entryIds()) entry_ids.append(entry_id);
    map[u"entryIds"_s] = entry_ids;
    return map;
}

PlaylistListQuery::PlaylistListQuery(QObject* parent): Query(parent) {}
auto PlaylistListQuery::playlists() const -> const QVariantList& { return m_playlists; }

void PlaylistListQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();
    auto req     = proto::Request {};
    req.setPlaylistList(proto::PlaylistListRequest {});
    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;
        self->inspect_set(result, [self](const proto::Response& rsp) {
            QVariantList out;
            for (const auto& p : rsp.playlistList().playlists()) {
                out.append(playlist_to_map(p));
            }
            self->m_playlists = std::move(out);
            Q_EMIT self->playlistsChanged();
        });
        co_return;
    });
}

PlaylistDetailQuery::PlaylistDetailQuery(QObject* parent): QueryList(parent) {
    setLimit(0);
    tdata()->set_store(tdata(), AppStore::instance()->wallpapers);
}

auto PlaylistDetailQuery::playlistId() const -> qint64 { return m_playlist_id; }

void PlaylistDetailQuery::setPlaylistId(qint64 id) {
    if (m_playlist_id == id) return;
    invalidateRequests();
    m_playlist_id = id;
    Q_EMIT playlistIdChanged();
}

auto PlaylistDetailQuery::playlist() const -> const QVariantMap& { return m_playlist; }
auto PlaylistDetailQuery::revision() const -> qint64 { return m_revision; }
auto PlaylistDetailQuery::entryIds() const -> const QStringList& { return m_entry_ids; }

void PlaylistDetailQuery::classBegin() {}

void PlaylistDetailQuery::componentComplete() {
    connect_requet_reload(&PlaylistDetailQuery::playlistIdChanged, this);
    reload();
}

void PlaylistDetailQuery::reload() {
    const auto revision = beginRequest();
    if (m_playlist_id <= 0) {
        return;
    }

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();
    auto detail  = proto::PlaylistGetRequest {};
    detail.setId_proto(m_playlist_id);
    auto req = proto::Request {};
    req.setPlaylistGet(std::move(detail));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;
        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto&                   detail   = rsp.playlistGet();
            const auto&                   summary  = detail.playlist();
            auto                          playlist = playlist_to_map(summary);
            std::vector<model::Wallpaper> wallpapers;
            wallpapers.reserve(detail.wallpapers().size());
            for (const auto& wallpaper : detail.wallpapers()) wallpapers.push_back(wallpaper);
            auto data = self->tdata();
            data->setHasMore(false);
            data->sync(wallpapers);

            self->m_playlist = std::move(playlist);
            self->m_revision = static_cast<qint64>(summary.revision());
            self->m_entry_ids.clear();
            self->m_entry_ids.reserve(summary.entryIds().size());
            for (const auto& entry_id : summary.entryIds()) self->m_entry_ids.append(entry_id);
            Q_EMIT self->playlistChanged();
        });
        co_return;
    });
}

PlaylistMutationQuery::PlaylistMutationQuery(QObject* parent): Query(parent) {}

static QStringList toStr(const QVariantList& v) {
    QStringList out;
    for (const auto& x : v) out.append(x.toString());
    return out;
}

void PlaylistMutationQuery::send(proto::Request req, bool captureCreate) {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();
    auto self    = QWatcher { this };
    spawn([self, backend, req = std::move(req), captureCreate]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [self, captureCreate](const proto::Response& rsp) {
            if (captureCreate) {
                self->m_createdId = static_cast<qint64>(rsp.playlistCreate().id_proto());
                Q_EMIT self->createdIdChanged();
            }
        });
        Q_EMIT self->done();
        co_return;
    });
}

void PlaylistMutationQuery::create(const QString& name, int mode, int intervalSecs,
                                   bool synchronizedSelection, const QVariantList& itemIds) {
    proto::PlaylistCreateRequest r;
    r.setName(name);
    r.setMode(static_cast<proto::PlaylistMode>(mode));
    r.setIntervalSecs(static_cast<QtProtobuf::uint32>(intervalSecs));
    r.setSynchronizedSelection(synchronizedSelection);
    r.setEntryIds(toStr(itemIds));
    proto::Request req;
    req.setPlaylistCreate(std::move(r));
    send(std::move(req), true);
}

void PlaylistMutationQuery::remove(qint64 id) {
    proto::PlaylistDeleteRequest r;
    r.setId_proto(id);
    proto::Request req;
    req.setPlaylistDelete(std::move(r));
    send(std::move(req), false);
}

void PlaylistMutationQuery::saveSmart(qint64 id, const QString& name, int mode, int intervalSecs,
                                      bool synchronizedSelection, const QVariant& selection,
                                      qint64 expectedRevision) {
    if (! selection.canConvert<proto::WallpaperSelection>()) {
        setError(u"invalid smart playlist selection"_s);
        setStatus(Status::Error);
        Q_EMIT done();
        return;
    }
    proto::PlaylistSource source;
    source.setSmart(selection.value<proto::WallpaperSelection>());
    proto::Request req;
    if (id > 0) {
        proto::PlaylistUpdateRequest r;
        r.setId_proto(id);
        r.setName(name);
        r.setMode(static_cast<proto::PlaylistMode>(mode));
        r.setIntervalSecs(static_cast<QtProtobuf::uint32>(intervalSecs));
        r.setSynchronizedSelection(synchronizedSelection);
        r.setExpectedRevision(expectedRevision);
        r.setSource(source);
        req.setPlaylistUpdate(std::move(r));
    } else {
        proto::PlaylistCreateRequest r;
        r.setName(name);
        r.setMode(static_cast<proto::PlaylistMode>(mode));
        r.setIntervalSecs(static_cast<QtProtobuf::uint32>(intervalSecs));
        r.setSynchronizedSelection(synchronizedSelection);
        r.setSource(source);
        req.setPlaylistCreate(std::move(r));
    }
    send(std::move(req), id <= 0);
}

void PlaylistMutationQuery::update(qint64 id, const QString& name, int mode, int intervalSecs,
                                   bool synchronizedSelection, const QVariantList& itemIds,
                                   qint64 expectedRevision) {
    proto::PlaylistUpdateRequest r;
    r.setId_proto(id);
    r.setName(name);
    r.setMode(static_cast<proto::PlaylistMode>(mode));
    r.setIntervalSecs(static_cast<QtProtobuf::uint32>(intervalSecs));
    r.setSynchronizedSelection(synchronizedSelection);
    r.setEntryIds(toStr(itemIds));
    r.setExpectedRevision(expectedRevision);
    proto::Request req;
    req.setPlaylistUpdate(std::move(r));
    send(std::move(req), false);
}

void PlaylistMutationQuery::rename(qint64 id, const QString& name) {
    proto::PlaylistRenameRequest r;
    r.setId_proto(id);
    r.setName(name);
    proto::Request req;
    req.setPlaylistRename(std::move(r));
    send(std::move(req), false);
}

void PlaylistMutationQuery::setItems(qint64 id, const QVariantList& itemIds,
                                     qint64 expectedRevision) {
    proto::PlaylistSetItemsRequest r;
    r.setId_proto(id);
    r.setEntryIds(toStr(itemIds));
    r.setExpectedRevision(expectedRevision);
    proto::Request req;
    req.setPlaylistSetItems(std::move(r));
    send(std::move(req), false);
}

void PlaylistMutationQuery::setMode(qint64 id, int mode) {
    proto::PlaylistSetModeRequest r;
    r.setId_proto(id);
    r.setMode(static_cast<proto::PlaylistMode>(mode));
    proto::Request req;
    req.setPlaylistSetMode(std::move(r));
    send(std::move(req), false);
}

void PlaylistMutationQuery::setInterval(qint64 id, int intervalSecs) {
    proto::PlaylistSetIntervalRequest r;
    r.setId_proto(id);
    r.setIntervalSecs(static_cast<QtProtobuf::uint32>(intervalSecs));
    proto::Request req;
    req.setPlaylistSetInterval(std::move(r));
    send(std::move(req), false);
}

void PlaylistMutationQuery::activate(qint64 id, const QVariantList& targets, bool autoAttach) {
    proto::PlaylistActivateRequest r;
    r.setId_proto(id);
    r.setTargets(presentationTargetsFromVariant(targets));
    r.setAutoAttach(autoAttach);
    proto::Request req;
    req.setPlaylistActivate(std::move(r));
    send(std::move(req), false);
}

void PlaylistMutationQuery::deactivate(const QVariantList& targets, qint64 clearAutoAttach) {
    proto::PlaylistDeactivateRequest r;
    r.setTargets(presentationTargetsFromVariant(targets));
    r.setClearAutoAttach(clearAutoAttach);
    proto::Request req;
    req.setPlaylistDeactivate(std::move(r));
    send(std::move(req), false);
}

void PlaylistMutationQuery::jumpTo(qint64 id, const QString& entryId) {
    proto::PlaylistJumpToRequest r;
    r.setId_proto(id);
    r.setEntryId(entryId);
    proto::Request req;
    req.setPlaylistJumpTo(std::move(r));
    send(std::move(req), false);
}

} // namespace waywallen

#include "waywallen/query/playlist_query.moc.cpp"
