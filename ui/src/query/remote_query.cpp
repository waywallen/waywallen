module;
#include "waywallen/query/remote_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import :query.remote;
import :app;
import :msg.store;

using namespace Qt::Literals::StringLiterals;

namespace proto = waywallen::control::v1;
using namespace qextra::prelude;

namespace waywallen
{

RemoteAvailabilityQuery::RemoteAvailabilityQuery(QObject* parent): Query(parent) {}

auto RemoteAvailabilityQuery::sources() const -> const QVariantList& { return m_sources; }
auto RemoteAvailabilityQuery::defaultSourceId() const -> const QString& {
    return m_default_source_id;
}

void RemoteAvailabilityQuery::reload() {
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setRemoteAvailability(proto::RemoteAvailabilityRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto&  av = rsp.remoteAvailability();
            QVariantList sources;
            sources.reserve(av.sources().size());
            for (const auto& src : av.sources()) {
                QVariantList sorts;
                sorts.reserve(src.sorts().size());
                for (const auto& sort : src.sorts()) {
                    QVariantMap sm;
                    sm[u"key"_s]   = sort.key();
                    sm[u"label"_s] = sort.label();
                    sorts.push_back(sm);
                }
                QStringList tags;
                for (const auto& tag : src.tags()) tags.push_back(tag);
                QVariantList filters;
                filters.reserve(src.filters().size());
                for (const auto& filter : src.filters()) {
                    QVariantMap fm;
                    QStringList values;
                    for (const auto& value : filter.values()) values.push_back(value);
                    QStringList valueLabels;
                    for (const auto& label : filter.valueLabels()) valueLabels.push_back(label);
                    fm[u"id"_s]           = filter.id_proto();
                    fm[u"title"_s]        = filter.title();
                    fm[u"type"_s]         = static_cast<int>(filter.type());
                    fm[u"values"_s]       = values;
                    fm[u"valueLabels"_s]  = valueLabels;
                    fm[u"description"_s]  = filter.description();
                    fm[u"confirmation"_s] = filter.confirmation();
                    filters.push_back(fm);
                }
                QVariantMap m;
                m[u"id"_s]               = src.id_proto();
                m[u"name"_s]             = src.name();
                m[u"supportsSearch"_s]   = src.supportsSearch();
                m[u"sorts"_s]            = sorts;
                m[u"tags"_s]             = tags;
                m[u"filters"_s]          = filters;
                m[u"contentDir"_s]       = src.contentDir();
                m[u"ownerPluginId"_s]    = src.ownerPluginId();
                m[u"displayName"_s]      = src.displayName();
                m[u"remoteCapability"_s] = static_cast<int>(src.remoteCapability());
                m[u"remoteHint"_s]       = src.remoteHint();
                m[u"avatarUrl"_s]        = src.avatarUrl();

                QVariantList settings;
                for (const auto& ss : src.settings()) {
                    QVariantMap sm;
                    sm[u"key"_s]             = ss.key();
                    sm[u"type"_s]            = static_cast<int>(ss.type());
                    sm[u"default_value"_s]   = ss.defaultValue();
                    sm[u"identity"_s]        = ss.identity();
                    sm[u"label_key"_s]       = ss.labelKey();
                    sm[u"description_key"_s] = ss.descriptionKey();
                    sm[u"min"_s]             = ss.min();
                    sm[u"max"_s]             = ss.max();
                    sm[u"step"_s]            = ss.step();
                    QStringList choices;
                    for (const auto& c : ss.choices()) {
                        choices.append(c);
                    }
                    sm[u"choices"_s] = choices;
                    sm[u"group"_s]   = ss.group();
                    sm[u"order"_s]   = static_cast<int>(ss.order());
                    settings.append(sm);
                }
                m[u"settings"_s] = settings;

                QVariantList actions;
                for (const auto& a : src.actions()) {
                    QVariantMap am;
                    am[u"id"_s]                = a.id_proto();
                    am[u"label"_s]             = a.label();
                    am[u"description"_s]       = a.description();
                    am[u"browseDescription"_s] = a.browseDescription();
                    am[u"browseButtonLabel"_s] = a.browseButtonLabel();
                    am[u"group"_s]             = a.group();
                    am[u"order"_s]             = static_cast<int>(a.order());
                    am[u"kind"_s]              = static_cast<int>(a.kind());
                    am[u"visible"_s]           = a.visible();
                    am[u"enabled"_s]           = a.enabled();
                    QVariantList fields;
                    for (const auto& field : a.fields()) {
                        QVariantMap fm;
                        fm[u"key"_s]         = field.key();
                        fm[u"label"_s]       = field.label();
                        fm[u"description"_s] = field.description();
                        fm[u"placeholder"_s] = field.placeholder();
                        fm[u"secret"_s]      = field.secret();
                        fm[u"required"_s]    = field.required();
                        fields.append(fm);
                    }
                    am[u"fields"_s]              = fields;
                    am[u"requiredForBrowsing"_s] = a.requiredForBrowsing();
                    actions.append(am);
                }
                m[u"actions"_s] = actions;

                QVariantList statusRows;
                for (const auto& st : src.status()) {
                    QVariantMap sm;
                    sm[u"id"_s]    = st.id_proto();
                    sm[u"label"_s] = st.label();
                    sm[u"group"_s] = st.group();
                    sm[u"order"_s] = static_cast<int>(st.order());
                    sm[u"value"_s] = st.value();
                    statusRows.append(sm);
                }
                m[u"status"_s] = statusRows;
                sources.push_back(m);
            }
            self->m_sources           = std::move(sources);
            self->m_default_source_id = av.defaultSourceId();
            Q_EMIT self->sourcesChanged();
        });
        co_return;
    });
}

RemoteSearchQuery::RemoteSearchQuery(QObject* parent): QueryList(parent) {
    tdata()->set_store(tdata(), AppStore::instance()->remotes);
    connect_requet_reload(&RemoteSearchQuery::sourceIdChanged, this);
    connect_requet_reload(&RemoteSearchQuery::queryChanged, this);
    connect_requet_reload(&RemoteSearchQuery::sortKeyChanged, this);
    connect_requet_reload(&RemoteSearchQuery::tagsChanged, this);
}

auto RemoteSearchQuery::sourceId() const -> const QString& { return m_source_id; }
void RemoteSearchQuery::setSourceId(const QString& v) {
    if (m_source_id != v) {
        m_source_id = v;
        Q_EMIT sourceIdChanged();
    }
}

auto RemoteSearchQuery::query() const -> const QString& { return m_query; }
void RemoteSearchQuery::setQuery(const QString& v) {
    if (m_query != v) {
        m_query = v;
        Q_EMIT queryChanged();
    }
}

auto RemoteSearchQuery::sortKey() const -> const QString& { return m_sort_key; }
void RemoteSearchQuery::setSortKey(const QString& v) {
    if (m_sort_key != v) {
        m_sort_key = v;
        Q_EMIT sortKeyChanged();
    }
}

auto RemoteSearchQuery::tags() const -> const QStringList& { return m_tags; }
void RemoteSearchQuery::setTags(const QStringList& v) {
    if (m_tags != v) {
        m_tags = v;
        Q_EMIT tagsChanged();
    }
}

auto RemoteSearchQuery::browsingEnabled() const -> bool { return m_browsing_enabled; }
void RemoteSearchQuery::setBrowsingEnabled(bool v) {
    if (m_browsing_enabled == v) return;
    m_browsing_enabled = v;
    Q_EMIT browsingEnabledChanged();
    if (v) {
        delayReload();
    } else {
        ++m_generation;
        cancel();
        clearResults();
    }
}

auto RemoteSearchQuery::model() const -> model::RemoteListModel* { return tdata(); }
auto RemoteSearchQuery::hasMore() const -> bool {
    const auto t = model();
    return t && t->hasMore();
}
auto RemoteSearchQuery::errorText() const -> const QString& { return m_error; }

void RemoteSearchQuery::reload() {
    if (! m_browsing_enabled || m_source_id.isEmpty()) {
        clearResults();
        return;
    }
    setOffset(0);
    setNoMore(false);
    fetchPage(1, false);
}

void RemoteSearchQuery::loadMore() {
    if (! m_browsing_enabled || noMore() || querying()) return;
    fetchPage(static_cast<quint32>(offset() + 2), true);
}

void RemoteSearchQuery::fetchMore(qint32) {
    if (! m_browsing_enabled || noMore()) return;
    fetchPage(static_cast<quint32>(offset() + 2), true);
}

void RemoteSearchQuery::clearResults() {
    setOffset(0);
    setNoMore(true);
    m_error.clear();
    setError({});
    if (auto t = model()) t->reset({}, false);
    setStatus(Status::Finished);
    Q_EMIT stateChanged();
}

void RemoteSearchQuery::fetchPage(quint32 page, bool append) {
    auto t = model();
    if (! t) return;
    t->setHasMore(false);
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::RemoteSearchRequest {};
    inner.setSourceId(m_source_id);
    inner.setQuery(m_query);
    inner.setSortKey(m_sort_key);
    inner.setPage(page);
    inner.setRequiredTags(m_tags);
    req.setRemoteSearch(std::move(inner));

    const auto generation = ++m_generation;
    auto       self       = QWatcher { this };
    spawn([self, backend, req = std::move(req), page, append, generation]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        if (self->m_generation != generation) co_return;

        self->inspect_set(result, [self, page, append](const proto::Response& rsp) {
            const auto&             sr = rsp.remoteSearch();
            QList<model::RemoteRow> rows;
            rows.reserve(sr.items().size());
            for (const auto& it : sr.items()) {
                rows.push_back(model::RemoteRow {
                    it.sourceId(),
                    it.id_proto(),
                    it.title(),
                    it.previewUrl(),
                    it.author(),
                    it.wpType(),
                    it.downloaded() ? 3 : 0,
                });
            }
            auto t = self->model();
            if (! t) return;
            const bool more = sr.hasMore() && ! rows.isEmpty();
            self->setOffset(static_cast<qint32>(page - 1));
            self->setNoMore(! more);
            self->m_error = sr.error();
            if (append)
                t->append(rows, more);
            else
                t->reset(std::move(rows), more);
            Q_EMIT self->stateChanged();
        });
        co_return;
    });
}

RemoteDetailsQuery::RemoteDetailsQuery(QObject* parent): Query(parent) {
    connect_requet_reload(&RemoteDetailsQuery::sourceIdChanged, this);
    connect_requet_reload(&RemoteDetailsQuery::itemIdChanged, this);
}

auto RemoteDetailsQuery::sourceId() const -> const QString& { return m_source_id; }
void RemoteDetailsQuery::setSourceId(const QString& v) {
    if (m_source_id != v) {
        m_source_id = v;
        Q_EMIT sourceIdChanged();
    }
}

auto RemoteDetailsQuery::itemId() const -> const QString& { return m_item_id; }
void RemoteDetailsQuery::setItemId(const QString& v) {
    if (m_item_id != v) {
        m_item_id = v;
        Q_EMIT itemIdChanged();
    }
}
auto RemoteDetailsQuery::author() const -> const QString& { return m_author; }
auto RemoteDetailsQuery::description() const -> const QString& { return m_description; }
auto RemoteDetailsQuery::size() const -> const QString& { return m_size; }
auto RemoteDetailsQuery::width() const -> int { return m_width; }
auto RemoteDetailsQuery::height() const -> int { return m_height; }
auto RemoteDetailsQuery::tags() const -> const QStringList& { return m_tags; }
auto RemoteDetailsQuery::webUrl() const -> const QString& { return m_web_url; }

void RemoteDetailsQuery::reload() {
    m_author.clear();
    m_description.clear();
    m_size.clear();
    m_width  = 0;
    m_height = 0;
    m_tags.clear();
    m_web_url.clear();
    Q_EMIT loaded();
    if (m_item_id.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::RemoteDetailsRequest {};
    inner.setSourceId(m_source_id);
    inner.setId_proto(m_item_id);
    req.setRemoteDetails(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req)]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto& dr      = rsp.remoteDetails();
            self->m_author      = dr.author();
            self->m_description = dr.description();
            self->m_size        = dr.size();
            self->m_width       = static_cast<int>(dr.width());
            self->m_height      = static_cast<int>(dr.height());
            self->m_web_url     = dr.webUrl();
            self->m_tags.clear();
            for (const auto& t : dr.tags()) self->m_tags.push_back(t);
            Q_EMIT self->loaded();
        });
        co_return;
    });
}

RemoteDownloadQuery::RemoteDownloadQuery(QObject* parent): Query(parent) {}

void RemoteDownloadQuery::reload() {}

void RemoteDownloadQuery::start(const QString& sourceId, const QString& id) {
    if (sourceId.isEmpty() || id.isEmpty()) return;
    AppStore::instance()->setRemoteAcquisitionState(sourceId, id, 1);
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::RemoteDownloadRequest {};
    inner.setSourceId(sourceId);
    inner.setId_proto(id);
    req.setRemoteDownload(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), sourceId, id]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        if (! result) {
            self->inspect_set(result, [](const proto::Response&) {
            });
            AppStore::instance()->setRemoteAcquisitionState(sourceId, id, 0);
            Q_EMIT self->rejected(sourceId, id, self->error());
            co_return;
        }

        self->inspect_set(result, [self, sourceId, id](const proto::Response& rsp) {
            const auto& dr = rsp.remoteDownload();
            if (dr.accepted()) {
                Q_EMIT self->accepted(sourceId, id);
            } else {
                AppStore::instance()->setRemoteAcquisitionState(sourceId, id, 0);
                Q_EMIT self->rejected(sourceId, id, dr.error());
            }
        });
        co_return;
    });
}

void RemoteDownloadQuery::remove(const QString& sourceId, const QString& id) {
    if (sourceId.isEmpty() || id.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::RemoteUninstallRequest {};
    inner.setSourceId(sourceId);
    inner.setId_proto(id);
    req.setRemoteUninstall(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), sourceId, id]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        if (! result) {
            self->inspect_set(result, [](const proto::Response&) {
            });
            Q_EMIT self->removeFailed(sourceId, id, self->error());
            co_return;
        }

        self->inspect_set(result, [self, sourceId, id](const proto::Response& rsp) {
            const auto& ur = rsp.remoteUninstall();
            if (ur.removed()) {
                AppStore::instance()->setRemoteAcquisitionState(sourceId, id, 0);
                Q_EMIT self->removed(sourceId, id);
            } else {
                Q_EMIT self->removeFailed(sourceId, id, ur.error());
            }
        });
        co_return;
    });
}

RemoteSubscriptionQuery::RemoteSubscriptionQuery(QObject* parent): Query(parent) {}

void RemoteSubscriptionQuery::reload() {}

void RemoteSubscriptionQuery::refresh(const QString& sourceId, const QString& id) {
    if (sourceId.isEmpty() || id.isEmpty()) return;
    AppStore::instance()->setRemoteAcquisitionState(sourceId, id, 3);
    const auto generation = ++m_refresh_generation;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::SubscriptionStatusRequest {};
    inner.setSourceId(sourceId);
    inner.setItemIds({ id });
    req.setSubscriptionStatus(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), sourceId, id, generation]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        if (self->m_refresh_generation != generation) co_return;

        if (! result) {
            self->inspect_set(result, [](const proto::Response&) {
            });
            AppStore::instance()->setRemoteAcquisitionState(sourceId, id, 0);
            Q_EMIT self->stateLoaded(sourceId, id, 0, self->error());
            co_return;
        }

        self->inspect_set(result, [self, sourceId, id](const proto::Response& rsp) {
            const auto& status = rsp.subscriptionStatus();
            auto        state  = 0;
            for (const auto& item : status.items()) {
                if (item.id_proto() == id) {
                    state = static_cast<int>(item.state());
                    break;
                }
            }
            AppStore::instance()->setRemoteAcquisitionState(sourceId, id, state);
            Q_EMIT self->stateLoaded(sourceId, id, state, status.error());
        });
        co_return;
    });
}

void RemoteSubscriptionQuery::setSubscribed(const QString& sourceId, const QString& id,
                                            bool subscribed) {
    if (sourceId.isEmpty() || id.isEmpty()) return;
    AppStore::instance()->setRemoteAcquisitionState(sourceId, id, 3);
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::SubscriptionSetRequest {};
    inner.setSourceId(sourceId);
    inner.setItemId(id);
    inner.setSubscribed(subscribed);
    req.setSubscriptionSet(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), sourceId, id, subscribed]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;

        if (! result) {
            self->inspect_set(result, [](const proto::Response&) {
            });
            AppStore::instance()->setRemoteAcquisitionState(sourceId, id, subscribed ? 1 : 2);
            Q_EMIT self->setFinished(sourceId, id, subscribed, false, self->error());
            co_return;
        }

        self->inspect_set(result, [self, sourceId, id, subscribed](const proto::Response& rsp) {
            const auto& update = rsp.subscriptionSet();
            const auto  state  = update.accepted() ? (subscribed ? 2 : 1) : (subscribed ? 1 : 2);
            AppStore::instance()->setRemoteAcquisitionState(sourceId, id, state);
            Q_EMIT self->setFinished(sourceId, id, subscribed, update.accepted(), update.error());
        });
        co_return;
    });
}

RemoteSettingsPatchQuery::RemoteSettingsPatchQuery(QObject* parent): Query(parent) {}

void RemoteSettingsPatchQuery::reload() {}

void RemoteSettingsPatchQuery::patch(const QString& sourceId, const QVariantMap& values) {
    if (sourceId.isEmpty() || values.isEmpty()) return;
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    QHash<QString, QString> wire_values;
    for (auto it = values.cbegin(); it != values.cend(); ++it) {
        wire_values.insert(it.key(), it.value().toString());
    }
    auto req   = proto::Request {};
    auto inner = proto::RemoteSettingsPatchRequest {};
    inner.setSourceId(sourceId);
    inner.setValues(wire_values);
    req.setRemoteSettingsPatch(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), sourceId, values]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        if (! result) {
            self->inspect_set(result, [](const proto::Response&) {
            });
            Q_EMIT self->completed(sourceId, values, false, self->error());
            co_return;
        }

        self->inspect_set(result, [](const proto::Response&) {
        });
        Q_EMIT self->completed(sourceId, values, true, QString {});
        co_return;
    });
}

} // namespace waywallen

#include "waywallen/query/remote_query.moc.cpp"
