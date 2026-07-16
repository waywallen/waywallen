module;
#include "waywallen/query/display_query.moc.h"
#include <qtprotobuftypes.h>
#undef assert
#include <rstd/macro.hpp>
#include <algorithm>

module waywallen;
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
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setDisplayList(proto::DisplayListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;

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
void DisplayLayoutSetQuery::setClearFillmode(bool v) { WW_SET(m_clear_fillmode, v); }
void DisplayLayoutSetQuery::setClearLocation(bool v) { WW_SET(m_clear_location, v); }
void DisplayLayoutSetQuery::setClearAlign(bool v) { WW_SET(m_clear_align, v); }
void DisplayLayoutSetQuery::setClearRotation(bool v) { WW_SET(m_clear_rotation, v); }
void DisplayLayoutSetQuery::setLocation(int v) { WW_SET(m_location, v); }
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
    inner.setLocation(static_cast<proto::LayoutLocation>(m_location));

    auto req = proto::Request {};
    req.setDisplayLayoutSet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
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
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;

        self->inspect_set(result, [](const proto::Response& rsp) {
            (void)rsp;
        });
        co_return;
    });
}

DisplayLockWallpaperSetQuery::DisplayLockWallpaperSetQuery(QObject* parent): Query(parent) {}

#define WW_SET(field, val)          \
    do {                            \
        if (this->field != val) {   \
            this->field = val;      \
            Q_EMIT paramsChanged(); \
        }                           \
    } while (0)

void DisplayLockWallpaperSetQuery::setDisplayIds(const QVariantList& v) { WW_SET(m_display_ids, v); }
void DisplayLockWallpaperSetQuery::setWallpaperId(const QString& v) { WW_SET(m_wallpaper_id, v); }
#undef WW_SET

void DisplayLockWallpaperSetQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    proto::DisplayLockWallpaperSetRequest inner;
    QtProtobuf::uint64List ids;
    ids.reserve(m_display_ids.size());
    for (const auto& v : m_display_ids) {
        bool ok = false;
        auto id = v.toULongLong(&ok);
        if (ok) ids.append(id);
    }
    inner.setDisplayIds(ids);
    inner.setWallpaperId(m_wallpaper_id);

    auto req = proto::Request {};
    req.setDisplayLockWallpaperSet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(QAsyncResult::get_executor(), use_task));
        if (! self) co_return;

        self->inspect_set(result, [](const proto::Response& rsp) {
            (void)rsp;
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/display_query.moc.cpp"
