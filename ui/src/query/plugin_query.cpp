module;
#include "waywallen/query/plugin_query.moc.h"
#undef assert
#include <algorithm>
#include <rstd/macro.hpp>

module waywallen;
import qextra;
import :query.plugin;
import :app;

using namespace Qt::Literals::StringLiterals;
using namespace qextra::prelude;

namespace proto = waywallen::control::v1;

namespace waywallen
{

// Flatten one renderer component (+ its settings schema) into a QVariantMap,
// matching RendererPluginListQuery so PluginSettingsPopup can consume it.
static auto renderer_to_map(const proto::RendererPluginInfo& r) -> QVariantMap {
    QVariantMap m;
    m[u"name"_s]    = r.name();
    m[u"version"_s] = r.version();
    QStringList types;
    for (const auto& t : r.types()) {
        types.append(t);
    }
    m[u"types"_s] = types;

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
        sm[u"description"_s]     = pluginMessageFromPb(s.description(), s.descriptionKey());
        sm[u"group_label"_s]     = pluginMessageFromPb(s.groupLabel(), s.group());
        sm[u"min"_s]             = s.min();
        sm[u"max"_s]             = s.max();
        sm[u"step"_s]            = s.step();
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
    return m;
}

static auto plugin_update_to_map(const proto::PluginUpdateInfo& info) -> QVariantMap {
    QVariantMap m;
    m[u"pluginId"_s]      = info.pluginId();
    m[u"state"_s]         = static_cast<int>(info.state());
    m[u"latestVersion"_s] = info.latestVersion();
    m[u"zipUrl"_s]        = info.zipUrl();
    m[u"sha256"_s]        = info.sha256();
    m[u"error"_s]         = info.error();
    m[u"checkedAtMs"_s]   = static_cast<qlonglong>(info.checkedAtMs());
    return m;
}

static auto plugin_compat_to_map(const proto::PluginCompat& compat) -> QVariantMap {
    QVariantMap m;
    m[u"compatible"_s] = compat.compatible();
    m[u"reason"_s]     = compat.reason();
    return m;
}

// --- PluginListQuery --------------------------------------------------------

PluginListQuery::PluginListQuery(QObject* parent): Query(parent) {}

auto PluginListQuery::plugins() const -> const QVariantList& { return m_plugins; }
auto PluginListQuery::inactiveSystem() const -> const QStringList& { return m_inactive_system; }
auto PluginListQuery::inactiveUser() const -> const QStringList& { return m_inactive_user; }

void PluginListQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setPluginList(proto::PluginListRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            QVariantList items;
            for (const auto& p : rsp.pluginList().plugins()) {
                QVariantMap m;
                m[u"id"_s]         = p.id_proto();
                m[u"name"_s]       = p.name();
                m[u"version"_s]    = p.version();
                m[u"update"_s]     = p.update();
                m[u"hasSource"_s]  = p.hasSource();
                m[u"system"_s]     = p.system();
                m[u"section"_s]    = p.system() ? u"system"_s : u"user"_s;
                m[u"updateInfo"_s] = plugin_update_to_map(p.updateInfo());
                m[u"compat"_s]     = plugin_compat_to_map(p.compat());
                QVariantList renderers;
                for (const auto& r : p.renderers()) {
                    renderers.append(renderer_to_map(r));
                }
                m[u"renderers"_s] = renderers;
                items.append(m);
            }
            std::sort(items.begin(), items.end(), [](const QVariant& a, const QVariant& b) {
                const auto am      = a.toMap();
                const auto bm      = b.toMap();
                const bool aSystem = am.value(u"system"_s).toBool();
                const bool bSystem = bm.value(u"system"_s).toBool();
                if (aSystem != bSystem) {
                    return ! aSystem;
                }
                auto an = am.value(u"name"_s).toString();
                auto bn = bm.value(u"name"_s).toString();
                if (an.isEmpty()) an = am.value(u"id"_s).toString();
                if (bn.isEmpty()) bn = bm.value(u"id"_s).toString();
                return QString::localeAwareCompare(an, bn) < 0;
            });
            QStringList inactive_system;
            for (const auto& id : rsp.pluginList().inactiveSystem()) {
                inactive_system.append(id);
            }
            QStringList inactive_user;
            for (const auto& id : rsp.pluginList().inactiveUser()) {
                inactive_user.append(id);
            }
            self->m_plugins         = std::move(items);
            self->m_inactive_system = std::move(inactive_system);
            self->m_inactive_user   = std::move(inactive_user);
            Q_EMIT self->pluginsChanged();
        });
        co_return;
    });
}

PluginInstallQuery::PluginInstallQuery(QObject* parent): Query(parent) {}

auto PluginInstallQuery::zipPath() const -> const QString& { return m_zip_path; }
void PluginInstallQuery::setZipPath(const QString& v) {
    if (m_zip_path == v) return;
    m_zip_path = v;
    Q_EMIT zipPathChanged();
}
auto PluginInstallQuery::pluginId() const -> const QString& { return m_plugin_id; }
auto PluginInstallQuery::needsRestart() const -> bool { return m_needs_restart; }

void PluginInstallQuery::reload() {
    if (m_zip_path.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::PluginInstallRequest {};
    inner.setZipPath(m_zip_path);
    req.setPluginInstall(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto& r         = rsp.pluginInstall();
            self->m_plugin_id     = r.pluginId();
            self->m_needs_restart = r.needsRestart();
            Q_EMIT self->resultChanged();
            Q_EMIT self->installed(self->m_plugin_id, self->m_needs_restart);
        });
        co_return;
    });
}

PluginInspectQuery::PluginInspectQuery(QObject* parent): Query(parent) {}

auto PluginInspectQuery::zipPath() const -> const QString& { return m_zip_path; }
void PluginInspectQuery::setZipPath(const QString& v) {
    if (m_zip_path == v) return;
    m_zip_path = v;
    Q_EMIT zipPathChanged();
}
auto PluginInspectQuery::pluginId() const -> const QString& { return m_plugin_id; }
auto PluginInspectQuery::name() const -> const QString& { return m_name; }
auto PluginInspectQuery::version() const -> const QString& { return m_version; }
auto PluginInspectQuery::update() const -> const QString& { return m_update; }
auto PluginInspectQuery::hasSource() const -> bool { return m_has_source; }
auto PluginInspectQuery::renderers() const -> const QStringList& { return m_renderers; }
auto PluginInspectQuery::overwrite() const -> bool { return m_overwrite; }
auto PluginInspectQuery::existingVersion() const -> const QString& { return m_existing_version; }
auto PluginInspectQuery::existingName() const -> const QString& { return m_existing_name; }
auto PluginInspectQuery::existingSystem() const -> bool { return m_existing_system; }

void PluginInspectQuery::reload() {
    if (m_zip_path.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::PluginInspectRequest {};
    inner.setZipPath(m_zip_path);
    req.setPluginInspect(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto& r            = rsp.pluginInspect();
            self->m_plugin_id        = r.pluginId();
            self->m_name             = r.name();
            self->m_version          = r.version();
            self->m_update           = r.update();
            self->m_has_source       = r.hasSource();
            self->m_overwrite        = r.overwrite();
            self->m_existing_version = r.existingVersion();
            self->m_existing_name    = r.existingName();
            self->m_existing_system  = r.existingSystem();
            QStringList renderers;
            for (const auto& name : r.renderers()) {
                renderers.append(name);
            }
            self->m_renderers = std::move(renderers);
            Q_EMIT self->resultChanged();
            Q_EMIT self->inspected();
        });
        co_return;
    });
}

PluginDeleteQuery::PluginDeleteQuery(QObject* parent): Query(parent) {}

auto PluginDeleteQuery::pluginId() const -> const QString& { return m_plugin_id; }
auto PluginDeleteQuery::needsRestart() const -> bool { return m_needs_restart; }

void PluginDeleteQuery::reload() { remove(m_plugin_id); }

void PluginDeleteQuery::remove(const QString& pluginId) {
    if (pluginId.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::PluginDeleteRequest {};
    inner.setPluginId(pluginId);
    req.setPluginDelete(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto& r         = rsp.pluginDelete();
            self->m_plugin_id     = r.pluginId();
            self->m_needs_restart = r.needsRestart();
            Q_EMIT self->resultChanged();
            Q_EMIT self->deleted(self->m_plugin_id, self->m_needs_restart);
        });
        co_return;
    });
}

PluginUpdateCheckQuery::PluginUpdateCheckQuery(QObject* parent): ProgressQuery(parent) {
    connect(this, &ProgressQuery::progressEnded, this, [this](bool error, const QString&) {
        if (! error) {
            Q_EMIT checked();
        }
    });
}

auto PluginUpdateCheckQuery::pluginId() const -> const QString& { return m_plugin_id; }
void PluginUpdateCheckQuery::setPluginId(const QString& v) {
    if (m_plugin_id == v) return;
    m_plugin_id = v;
    Q_EMIT pluginIdChanged();
}
auto PluginUpdateCheckQuery::updates() const -> const QVariantList& { return m_updates; }

void PluginUpdateCheckQuery::reload() { check(m_plugin_id); }

void PluginUpdateCheckQuery::check(const QString& pluginId) {
    if (querying() || progressing()) return;
    setPluginId(pluginId);
    beginProgressQuery();
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::PluginUpdateCheckRequest {};
    inner.setPluginId(m_plugin_id);
    req.setPluginUpdateCheck(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        if (! result) {
            self->failProgressQuery(result.unwrap_err_unchecked());
            co_return;
        }

        const auto   rsp = result.unwrap_unchecked();
        QVariantList updates;
        for (const auto& info : rsp.pluginUpdateCheck().updates()) {
            updates.append(plugin_update_to_map(info));
        }
        self->m_updates = std::move(updates);
        Q_EMIT self->updatesChanged();
        self->acceptProgressQuery(rsp.pluginUpdateCheck().queryId());
        co_return;
    });
}

PluginUpdateInstallQuery::PluginUpdateInstallQuery(QObject* parent): ProgressQuery(parent) {
    connect(this, &ProgressQuery::progressEnded, this, [this](bool error, const QString&) {
        if (! error) {
            Q_EMIT installed(m_plugin_id);
        }
    });
}

auto PluginUpdateInstallQuery::pluginId() const -> const QString& { return m_plugin_id; }

void PluginUpdateInstallQuery::reload() { install(m_plugin_id); }

void PluginUpdateInstallQuery::install(const QString& pluginId) {
    if (pluginId.isEmpty() || querying() || progressing()) return;
    if (m_plugin_id != pluginId) {
        m_plugin_id = pluginId;
        Q_EMIT pluginIdChanged();
    }
    beginProgressQuery();
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::PluginUpdateInstallRequest {};
    inner.setPluginId(m_plugin_id);
    req.setPluginUpdateInstall(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        if (! result) {
            self->failProgressQuery(result.unwrap_err_unchecked());
            co_return;
        }

        const auto rsp = result.unwrap_unchecked();
        self->acceptProgressQuery(rsp.pluginUpdateInstall().queryId());
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/plugin_query.moc.cpp"
