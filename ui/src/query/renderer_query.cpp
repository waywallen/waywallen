module;
#include "waywallen/query/renderer_query.moc.h"
#undef assert
#include <rstd/macro.hpp>
#include <algorithm>

module waywallen;
import qextra;
import :query.renderer;
import :app;
import :renderer;

using namespace Qt::Literals::StringLiterals;

namespace proto = waywallen::control::v1;
using namespace qextra::prelude;

namespace waywallen
{

static auto renderer_map_name(const QVariant& item) -> QString {
    return item.toMap().value(u"name"_s).toString();
}

static void sort_renderer_maps(QVariantList& items) {
    std::sort(items.begin(), items.end(), [](const QVariant& a, const QVariant& b) {
        const auto an  = renderer_map_name(a);
        const auto bn  = renderer_map_name(b);
        const auto cmp = QString::compare(an, bn, Qt::CaseInsensitive);
        return cmp == 0 ? QString::compare(an, bn, Qt::CaseSensitive) < 0 : cmp < 0;
    });
}

// ---------------------------------------------------------------------------
// RendererListQuery
// ---------------------------------------------------------------------------

RendererListQuery::RendererListQuery(QObject* parent): Query(parent) {}

auto RendererListQuery::renderers() const -> const QStringList& { return m_renderers; }
auto RendererListQuery::instances() const -> const QVariantList& { return m_instances; }

void RendererListQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setRendererList(proto::RendererListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

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
                Renderer    renderer { inst };
                QVariantMap m;
                m[u"id"_s]                 = inst.rendererId();
                m[u"fps"_s]                = inst.fps();
                m[u"state"_s]              = static_cast<int>(renderer.state());
                m[u"status"_s]             = renderer.status();
                m[u"keep"_s]               = renderer.keep();
                m[u"process_generation"_s] = QVariant::fromValue(renderer.processGeneration());
                m[u"last_exit_reason"_s]   = renderer.lastExitReason();
                m[u"name"_s]               = inst.name();
                m[u"pid"_s]                = inst.pid();
                m[u"texture_width"_s]      = inst.textureWidth();
                m[u"texture_height"_s]     = inst.textureHeight();
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
auto RendererPluginListQuery::typeLabels() const -> const QVariantMap& { return m_type_labels; }

void RendererPluginListQuery::reload() {
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setRendererPluginList(proto::RendererPluginListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

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

                // Flatten SettingSchema entries to QVariantMaps so QML can
                // build a typed form without touching protobuf objects. The
                // `type` enum is exposed as an integer (matches the proto
                // `SettingValueType` numeric values: U32=1, F32=2, STRING=3,
                // BOOL=4, I32=5) so QML compares with plain integer literals.
                QVariantList settings;
                for (const auto& s : r.settings()) {
                    QVariantMap sm;
                    sm[u"key"_s]             = s.key();
                    sm[u"type"_s]            = static_cast<int>(s.type());
                    sm[u"default_value"_s]   = s.defaultValue();
                    sm[u"identity"_s]        = s.identity();
                    sm[u"label_key"_s]       = s.labelKey();
                    sm[u"description_key"_s] = s.descriptionKey();
                    sm[u"label"_s]           = pluginMessageFromPb(s.label(), s.labelKey());
                    sm[u"description"_s] = pluginMessageFromPb(s.description(), s.descriptionKey());
                    sm[u"group_label"_s] = pluginMessageFromPb(s.groupLabel(), s.group());
                    sm[u"min"_s]         = s.min();
                    sm[u"max"_s]         = s.max();
                    sm[u"step"_s]        = s.step();
                    QStringList choices;
                    for (const auto& c : s.choices()) {
                        choices.append(c);
                    }
                    sm[u"choices"_s] = choices;
                    sm[u"group"_s]   = s.group();
                    sm[u"order"_s]   = static_cast<int>(s.order());
                    settings.append(sm);
                }
                m[u"settings"_s] = settings;
                items.append(m);
            }
            sort_renderer_maps(items);
            self->m_renderers = std::move(items);
            Q_EMIT self->renderersChanged();

            QStringList types;
            for (const auto& t : list_rsp.supportedTypes()) {
                types.append(t);
            }
            self->m_supported_types = std::move(types);
            Q_EMIT self->supportedTypesChanged();

            QVariantMap type_labels;
            for (const auto& info : list_rsp.typeLabels()) {
                if (info.type().isEmpty()) continue;
                type_labels[info.type()] = pluginMessageFromPb(info.labelText(), info.label());
            }
            self->m_type_labels = std::move(type_labels);
            Q_EMIT self->typeLabelsChanged();
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
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [](const proto::Response&) {
        });
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/renderer_query.moc.cpp"
