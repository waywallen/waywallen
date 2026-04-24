module;
#include "waywallen/query/renderer_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.renderer;
import :app;

using namespace Qt::Literals::StringLiterals;

namespace proto = waywallen::control::v1;
using namespace qextra::prelude;

namespace waywallen
{

// ---------------------------------------------------------------------------
// RendererListQuery
// ---------------------------------------------------------------------------

RendererListQuery::RendererListQuery(QObject* parent): Query(parent) {}

auto RendererListQuery::renderers() const -> const QStringList& { return m_renderers; }
auto RendererListQuery::instances() const -> const QVariantList& { return m_instances; }

void RendererListQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setRendererList(proto::RendererListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self](const proto::Response& rsp) {
            auto& list_rsp = rsp.rendererList();

            // Sync the global RendererManager first so any consumer pulling
            // from the manager sees the freshly-fetched rows before this
            // query's own `renderersChanged` fires.
            if (auto* rm = RendererManager::instance()) {
                rm->replaceAll(list_rsp.instances());
            }

            QStringList ids;
            for (const auto& id : list_rsp.renderers()) {
                ids.append(id);
            }
            self->m_renderers = std::move(ids);
            Q_EMIT self->renderersChanged();

            QVariantList instances;
            for (const auto& inst : list_rsp.instances()) {
                QVariantMap m;
                m[u"id"_s]     = inst.rendererId();
                m[u"fps"_s]    = inst.fps();
                m[u"status"_s] = inst.status();
                m[u"name"_s]   = inst.name();
                m[u"pid"_s]    = inst.pid();
                instances.append(m);
            }
            self->m_instances = std::move(instances);
            Q_EMIT self->instancesChanged();
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// RendererPluginListQuery
// ---------------------------------------------------------------------------

RendererPluginListQuery::RendererPluginListQuery(QObject* parent): Query(parent) {}

auto RendererPluginListQuery::renderers() const -> const QVariantList& { return m_renderers; }
auto RendererPluginListQuery::supportedTypes() const -> const QStringList& {
    return m_supported_types;
}

void RendererPluginListQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setRendererPluginList(proto::RendererPluginListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [self](const proto::Response& rsp) {
            auto& list_rsp = rsp.rendererPluginList();

            QVariantList items;
            for (const auto& r : list_rsp.renderers()) {
                QVariantMap m;
                m[u"name"_s]     = r.name();
                m[u"bin"_s]      = r.bin();
                m[u"priority"_s] = r.priority();
                m[u"version"_s]  = r.version();
                QStringList types;
                for (const auto& t : r.types()) {
                    types.append(t);
                }
                m[u"types"_s] = types;
                items.append(m);
            }
            self->m_renderers = std::move(items);
            Q_EMIT self->renderersChanged();

            QStringList types;
            for (const auto& t : list_rsp.supportedTypes()) {
                types.append(t);
            }
            self->m_supported_types = std::move(types);
            Q_EMIT self->supportedTypesChanged();
        });
        co_return;
    });
}

// ---------------------------------------------------------------------------
// RendererKillQuery
// ---------------------------------------------------------------------------

RendererKillQuery::RendererKillQuery(QObject* parent): Query(parent) {}

auto RendererKillQuery::rendererId() const -> const QString& { return m_renderer_id; }
void RendererKillQuery::setRendererId(const QString& v) {
    if (m_renderer_id != v) {
        m_renderer_id = v;
        Q_EMIT rendererIdChanged();
    }
}

void RendererKillQuery::reload() {
    if (m_renderer_id.isEmpty()) return;

    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::RendererKillRequest {};
    inner.setRendererId(m_renderer_id);
    req.setRendererKill(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        co_await asio::post(asio::bind_executor(self->get_executor(), use_task));

        self->inspect_set(result, [](const proto::Response&) {});
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/renderer_query.moc.cpp"
