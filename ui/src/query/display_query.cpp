module;
#include "waywallen/query/display_query.moc.h"
#undef assert
#include <rstd/macro.hpp>
#include <algorithm>

module waywallen;
import qextra;
import :query.display;
import :app;
import :display;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

DisplayListQuery::DisplayListQuery(QObject* parent): Query(parent) {}

auto DisplayListQuery::displays() const -> const QVariantList& { return m_displays; }

void DisplayListQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setDisplayList(proto::DisplayListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            auto& list_rsp = rsp.displayList();

            // Sync the global DisplayManager first so any consumer pulling
            // from the manager sees the freshly-fetched rows before this
            // query's own `displaysChanged` fires.
            if (auto* dm = DisplayManager::instance()) {
                dm->replaceAll(list_rsp.displays());
            }

            QVariantList items;
            for (const auto& d : list_rsp.displays()) {
                QVariantMap m;
                m[u"id"_s]         = QVariant::fromValue<quint64>(d.displayId());
                m[u"name"_s]       = d.name();
                m[u"width"_s]      = d.width();
                m[u"height"_s]     = d.height();
                m[u"refreshMhz"_s] = d.refreshMhz();

                QVariantList links;
                for (const auto& l : d.links()) {
                    QVariantMap lm;
                    lm[u"rendererId"_s] = l.rendererId();
                    lm[u"zOrder"_s]     = static_cast<int>(l.zOrder());
                    lm[u"active"_s]     = l.active();
                    links.append(lm);
                }
                m[u"links"_s] = links;
                items.append(m);
            }
            self->m_displays = std::move(items);
            Q_EMIT self->displaysChanged();
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// DisplayLayoutSetQuery
// ---------------------------------------------------------------------------

DisplayLayoutSetQuery::DisplayLayoutSetQuery(QObject* parent): Query(parent) {}

#define WW_SET(field, val)          \
    do {                            \
        if (this->field != val) {   \
            this->field = val;      \
            Q_EMIT paramsChanged(); \
        }                           \
    } while (0)

void DisplayLayoutSetQuery::setName(const QString& v) { WW_SET(m_name, v); }
void DisplayLayoutSetQuery::setDisplayId(quint64 v) { WW_SET(m_display_id, v); }
void DisplayLayoutSetQuery::setFillmodeSet(bool v) { WW_SET(m_fillmode_set, v); }
void DisplayLayoutSetQuery::setFillmode(int v) { WW_SET(m_fillmode, v); }
void DisplayLayoutSetQuery::setLocationSet(bool v) { WW_SET(m_location_set, v); }
void DisplayLayoutSetQuery::setLocationX(int v) { WW_SET(m_location_x, v); }
void DisplayLayoutSetQuery::setLocationY(int v) { WW_SET(m_location_y, v); }
void DisplayLayoutSetQuery::setAlignSet(bool v) { WW_SET(m_align_set, v); }
void DisplayLayoutSetQuery::setAlign(int v) { WW_SET(m_align, v); }
void DisplayLayoutSetQuery::setRotationSet(bool v) { WW_SET(m_rotation_set, v); }
void DisplayLayoutSetQuery::setRotation(int v) { WW_SET(m_rotation, v); }
void DisplayLayoutSetQuery::setFlip(int v) { WW_SET(m_flip, v); }
void DisplayLayoutSetQuery::setFlipSet(bool v) { WW_SET(m_flip_set, v); }
void DisplayLayoutSetQuery::setClearFlip(bool v) { WW_SET(m_clear_flip, v); }
void DisplayLayoutSetQuery::setClearFillmode(bool v) { WW_SET(m_clear_fillmode, v); }
void DisplayLayoutSetQuery::setClearLocation(bool v) { WW_SET(m_clear_location, v); }
void DisplayLayoutSetQuery::setClearAlign(bool v) { WW_SET(m_clear_align, v); }
void DisplayLayoutSetQuery::setClearRotation(bool v) { WW_SET(m_clear_rotation, v); }
#undef WW_SET

void DisplayLayoutSetQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    proto::LayoutOverride ovr;
    ovr.setFillmodeSet(m_fillmode_set);
    ovr.setFillmode(static_cast<proto::FillMode>(m_fillmode));
    ovr.setLocationSet(m_location_set);
    ovr.setLocationX(static_cast<quint32>(std::clamp(m_location_x, 0, 100)));
    ovr.setLocationY(static_cast<quint32>(std::clamp(m_location_y, 0, 100)));
    ovr.setAlignSet(m_align_set);
    ovr.setAlign(static_cast<proto::Align>(m_align));
    ovr.setFlipSet(m_flip_set);
    ovr.setFlip(static_cast<proto::Flip>(m_flip));
    ovr.setRotationSet(m_rotation_set);
    ovr.setRotation(static_cast<proto::Rotation>(m_rotation));

    proto::DisplayLayoutSetRequest inner;
    inner.setName(m_name);
    inner.setDisplayId(m_display_id);
    inner.setOverride(ovr);
    inner.setClearFillmode(m_clear_fillmode);
    inner.setClearLocation(m_clear_location);
    inner.setClearAlign(m_clear_align);
    inner.setClearRotation(m_clear_rotation);
    inner.setClearFlip(m_clear_flip);

    auto req = proto::Request {};
    req.setDisplayLayoutSet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [](const proto::Response& rsp) {
            // Daemon broadcasts DisplayChanged after the write; the
            // singleton DisplayManager picks it up via Backend events.
            // Nothing to do here beyond clearing query status.
            (void)rsp;
        });
        co_return;
    });
}

CanvasLayoutSetQuery::CanvasLayoutSetQuery(QObject* parent): Query(parent) {}

#define WW_SET(field, val)          \
    do {                            \
        if (this->field != val) {   \
            this->field = val;      \
            Q_EMIT paramsChanged(); \
        }                           \
    } while (0)

void CanvasLayoutSetQuery::setCanvasId(const QString& v) { WW_SET(m_canvas_id, v); }
void CanvasLayoutSetQuery::setFillmodeSet(bool v) { WW_SET(m_fillmode_set, v); }
void CanvasLayoutSetQuery::setFillmode(int v) { WW_SET(m_fillmode, v); }
void CanvasLayoutSetQuery::setLocationSet(bool v) { WW_SET(m_location_set, v); }
void CanvasLayoutSetQuery::setLocationX(int v) { WW_SET(m_location_x, v); }
void CanvasLayoutSetQuery::setLocationY(int v) { WW_SET(m_location_y, v); }
void CanvasLayoutSetQuery::setRotationSet(bool v) { WW_SET(m_rotation_set, v); }
void CanvasLayoutSetQuery::setRotation(int v) { WW_SET(m_rotation, v); }
void CanvasLayoutSetQuery::setFlip(int v) { WW_SET(m_flip, v); }
void CanvasLayoutSetQuery::setFlipSet(bool v) { WW_SET(m_flip_set, v); }
void CanvasLayoutSetQuery::setClearFlip(bool v) { WW_SET(m_clear_flip, v); }
void CanvasLayoutSetQuery::setClearFillmode(bool v) { WW_SET(m_clear_fillmode, v); }
void CanvasLayoutSetQuery::setClearLocation(bool v) { WW_SET(m_clear_location, v); }
void CanvasLayoutSetQuery::setClearRotation(bool v) { WW_SET(m_clear_rotation, v); }
#undef WW_SET

void CanvasLayoutSetQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    proto::LayoutOverride ovr;
    ovr.setFillmodeSet(m_fillmode_set);
    ovr.setFillmode(static_cast<proto::FillMode>(m_fillmode));
    ovr.setLocationSet(m_location_set);
    ovr.setLocationX(static_cast<quint32>(std::clamp(m_location_x, 0, 100)));
    ovr.setLocationY(static_cast<quint32>(std::clamp(m_location_y, 0, 100)));
    ovr.setFlipSet(m_flip_set);
    ovr.setFlip(static_cast<proto::Flip>(m_flip));
    ovr.setRotationSet(m_rotation_set);
    ovr.setRotation(static_cast<proto::Rotation>(m_rotation));

    proto::CanvasLayoutSetRequest inner;
    inner.setCanvasId(m_canvas_id);
    inner.setOverride(std::move(ovr));
    inner.setClearFillmode(m_clear_fillmode);
    inner.setClearLocation(m_clear_location);
    inner.setClearRotation(m_clear_rotation);
    inner.setClearFlip(m_clear_flip);

    auto req = proto::Request {};
    req.setCanvasLayoutSet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [](const proto::Response& rsp) {
            (void)rsp;
        });
        co_return;
    });
}

DisplayPauseSetQuery::DisplayPauseSetQuery(QObject* parent): Query(parent) {}

void DisplayPauseSetQuery::setDisplayId(quint64 value) {
    if (m_display_id == value) return;
    m_display_id = value;
    Q_EMIT paramsChanged();
}

void DisplayPauseSetQuery::setPaused(bool value) {
    if (m_paused == value) return;
    m_paused = value;
    Q_EMIT paramsChanged();
}

void DisplayPauseSetQuery::reload() {
    setStatus(Status::Querying);
    auto                          backend = App::instance()->backend();
    proto::DisplayPauseSetRequest inner;
    inner.setDisplayId(m_display_id);
    inner.setPaused(m_paused);
    proto::Request request;
    request.setDisplayPauseSet(std::move(inner));
    auto self = QWatcher { this };
    spawn([self, backend, request = std::move(request)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(request));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [](const proto::Response&) {
            // DisplayManager consumes the daemon's ordered DisplayChanged events.
        });
        co_return;
    });
}

DisplayRenameQuery::DisplayRenameQuery(QObject* parent): Query(parent) {}

#define WW_SET(field, val)          \
    do {                            \
        if (this->field != val) {   \
            this->field = val;      \
            Q_EMIT paramsChanged(); \
        }                           \
    } while (0)

void DisplayRenameQuery::setName(const QString& v) { WW_SET(m_name, v); }
void DisplayRenameQuery::setDisplayId(quint64 v) { WW_SET(m_display_id, v); }
void DisplayRenameQuery::setAlias(const QString& v) { WW_SET(m_alias, v); }
void DisplayRenameQuery::setClear(bool v) { WW_SET(m_clear, v); }
#undef WW_SET

void DisplayRenameQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    proto::DisplayRenameRequest inner;
    inner.setName(m_name);
    inner.setDisplayId(m_display_id);
    inner.setAlias(m_alias);
    inner.setClear(m_clear);

    auto req = proto::Request {};
    req.setDisplayRename(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [](const proto::Response& rsp) {
            (void)rsp;
        });
        co_return;
    });
}

CanvasListQuery::CanvasListQuery(QObject* parent): Query(parent) {}

void CanvasListQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    proto::Request req;
    req.setCanvasList(proto::CanvasListRequest {});
    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;
        self->inspect_set(result, [](const proto::Response& response) {
            if (auto* manager = DisplayManager::instance()) {
                manager->replaceCanvases(response.canvasList().canvases(),
                                         response.canvasList().revision());
            }
        });
        co_return;
    });
}

CanvasMutationQuery::CanvasMutationQuery(QObject* parent): Query(parent) {}

static auto canvasMembersFromVariant(const QVariantList& values)
    -> QList<proto::CanvasMemberInput> {
    QList<proto::CanvasMemberInput> members;
    members.reserve(values.size());
    for (const auto& value : values) {
        const auto        map = value.toMap();
        proto::CanvasRect rect;
        rect.setX(map.value(u"x"_s).toInt());
        rect.setY(map.value(u"y"_s).toInt());
        rect.setWidth(map.value(u"width"_s).toUInt());
        rect.setHeight(map.value(u"height"_s).toUInt());
        proto::CanvasMemberInput member;
        member.setSettingsKey(map.value(u"settingsKey"_s).toString());
        member.setRect(std::move(rect));
        member.setAspectLocked(map.value(u"aspectLocked"_s, true).toBool());
        members.append(std::move(member));
    }
    return members;
}

void CanvasMutationQuery::createCanvas(const QString& name, const QVariantList& members) {
    proto::CanvasCreateRequest inner;
    inner.setName(name);
    inner.setMembers(canvasMembersFromVariant(members));
    proto::Request request;
    request.setCanvasCreate(std::move(inner));
    m_request = std::move(request);
    reload();
}

void CanvasMutationQuery::updateCanvas(const QString& id, quint64 expectedRevision,
                                       const QString& name, const QVariantList& members) {
    proto::CanvasUpdateRequest inner;
    inner.setCanvasId(id);
    inner.setExpectedRevision(expectedRevision);
    inner.setName(name);
    inner.setMembers(canvasMembersFromVariant(members));
    proto::Request request;
    request.setCanvasUpdate(std::move(inner));
    m_request = std::move(request);
    reload();
}

void CanvasMutationQuery::removeCanvas(const QString& id, quint64 expectedRevision) {
    proto::CanvasDeleteRequest inner;
    inner.setCanvasId(id);
    inner.setExpectedRevision(expectedRevision);
    proto::Request request;
    request.setCanvasDelete(std::move(inner));
    m_request = std::move(request);
    reload();
}

void CanvasMutationQuery::reload() {
    if (! m_request) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();
    auto request = std::move(*m_request);
    m_request.reset();
    const auto deleted_id =
        request.hasCanvasDelete() ? request.canvasDelete().canvasId() : QString {};

    auto self = QWatcher { this };
    spawn([self, backend, request = std::move(request), deleted_id]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(request));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [self, deleted_id](const proto::Response& response) {
            if (response.hasCanvasCreate() && response.canvasCreate().hasCanvas()) {
                Q_EMIT self->canvasCreated(response.canvasCreate().canvas().canvasId());
            } else if (response.hasCanvasUpdate() && response.canvasUpdate().hasCanvas()) {
                Q_EMIT self->canvasUpdated(response.canvasUpdate().canvas().canvasId(),
                                           response.canvasUpdate().revision());
            } else if (response.hasCanvasDelete()) {
                Q_EMIT self->canvasDeleted(deleted_id);
            }
        });
        co_return;
    });
}

CanvasMemberConfigureQuery::CanvasMemberConfigureQuery(QObject* parent): Query(parent) {}

static auto canvasMemberConfigureRequest(const QString& canvasId, quint64 expectedRevision,
                                         const QString& settingsKey)
    -> proto::CanvasMemberConfigureRequest {
    proto::CanvasMemberConfigureRequest inner;
    inner.setCanvasId(canvasId);
    inner.setExpectedRevision(expectedRevision);
    inner.setSettingsKey(settingsKey);
    return inner;
}

void CanvasMemberConfigureQuery::configureWidth(const QString& canvasId, quint64 expectedRevision,
                                                const QString& settingsKey, quint32 width) {
    auto inner = canvasMemberConfigureRequest(canvasId, expectedRevision, settingsKey);
    inner.setWidth(width);
    proto::Request request;
    request.setCanvasMemberConfigure(std::move(inner));
    m_request = std::move(request);
    reload();
}

void CanvasMemberConfigureQuery::configureHeight(const QString& canvasId, quint64 expectedRevision,
                                                 const QString& settingsKey, quint32 height) {
    auto inner = canvasMemberConfigureRequest(canvasId, expectedRevision, settingsKey);
    inner.setHeight(height);
    proto::Request request;
    request.setCanvasMemberConfigure(std::move(inner));
    m_request = std::move(request);
    reload();
}

void CanvasMemberConfigureQuery::configureAspectLocked(const QString& canvasId,
                                                       quint64        expectedRevision,
                                                       const QString& settingsKey, bool locked) {
    auto inner = canvasMemberConfigureRequest(canvasId, expectedRevision, settingsKey);
    inner.setAspectLockedSet(true);
    inner.setAspectLocked(locked);
    proto::Request request;
    request.setCanvasMemberConfigure(std::move(inner));
    m_request = std::move(request);
    reload();
}

void CanvasMemberConfigureQuery::resetSize(const QString& canvasId, quint64 expectedRevision,
                                           const QString& settingsKey) {
    auto inner = canvasMemberConfigureRequest(canvasId, expectedRevision, settingsKey);
    inner.setResetSize(true);
    proto::Request request;
    request.setCanvasMemberConfigure(std::move(inner));
    m_request = std::move(request);
    reload();
}

void CanvasMemberConfigureQuery::reload() {
    if (! m_request) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();
    auto request = std::move(*m_request);
    m_request.reset();

    auto self = QWatcher { this };
    spawn([self, backend, request = std::move(request)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(request));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        self->inspect_set(result, [](const proto::Response&) {
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/display_query.moc.cpp"
