module;
#include "waywallen/query/remote_query.moc.h"
#undef assert
#include <rstd/macro.hpp>

module waywallen;
import qextra;
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
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req = proto::Request {};
    req.setRemoteAvailability(proto::RemoteAvailabilityRequest {});

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        self->inspect_set(result, [self](const proto::Response& rsp) {
            const auto&  av = rsp.remoteAvailability();
            QVariantList sources;
            sources.reserve(av.sources().size());
            for (const auto& src : av.sources()) {
                QVariantList sorts;
                sorts.reserve(src.sorts().size());
                for (const auto& sort : src.sorts()) {
                    QVariantMap sm;
                    sm[u"key"_s]       = sort.key();
                    sm[u"label"_s]     = sort.label();
                    sm[u"labelText"_s] = pluginMessageFromPb(sort.labelText(), sort.label());
                    sorts.push_back(sm);
                }
                QStringList tags;
                for (const auto& tag : src.tags()) tags.push_back(tag);
                QVariantList filters;
                filters.reserve(src.filters().size());
                for (const auto& filter : src.filters()) {
                    QVariantMap  fm;
                    QVariantList options;
                    options.reserve(filter.options().size());
                    for (const auto& option : filter.options()) {
                        QVariantMap om;
                        const auto  fallback =
                            option.label().isEmpty() ? option.value() : option.label();
                        om[u"value"_s]     = option.value();
                        om[u"label"_s]     = fallback;
                        om[u"labelText"_s] = pluginMessageFromPb(option.labelText(), fallback);
                        options.push_back(om);
                    }
                    fm[u"id"_s]           = filter.id_proto();
                    fm[u"title"_s]        = filter.title();
                    fm[u"type"_s]         = static_cast<int>(filter.type());
                    fm[u"options"_s]      = options;
                    fm[u"description"_s]  = filter.description();
                    fm[u"confirmation"_s] = filter.confirmation();
                    fm[u"matchModes"_s] = filter.matchModes();
                    fm[u"defaultMatchMode"_s] = filter.defaultMatchMode();
                    fm[u"supportsExclusion"_s] = filter.supportsExclusion();
                    fm[u"titleText"_s]    = pluginMessageFromPb(filter.titleText(), filter.title());
                    fm[u"descriptionText"_s] =
                        pluginMessageFromPb(filter.descriptionText(), filter.description());
                    fm[u"confirmationText"_s] =
                        pluginMessageFromPb(filter.confirmationText(), filter.confirmation());
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
                m[u"pagination"_s]       = static_cast<int>(src.pagination());
                m[u"remoteHint"_s]       = src.remoteHint();
                m[u"avatarUrl"_s]        = src.avatarUrl();
                m[u"displayNameText"_s] =
                    pluginMessageFromPb(src.displayNameText(), src.displayName());
                m[u"remoteHintText"_s] =
                    pluginMessageFromPb(src.remoteHintText(), src.remoteHint());
                const auto& download = src.downloadAction();
                QVariantMap download_action;
                download_action[u"enabled"_s] = ! src.hasDownloadAction() || download.enabled();
                download_action[u"labelText"_s] =
                    pluginMessageFromPb(download.labelText(), download.label());
                download_action[u"tooltipText"_s] =
                    pluginMessageFromPb(download.tooltipText(), download.tooltip());
                m[u"downloadAction"_s] = download_action;

                QVariantList settings;
                for (const auto& ss : src.settings()) {
                    QVariantMap sm;
                    sm[u"key"_s]             = ss.key();
                    sm[u"type"_s]            = static_cast<int>(ss.type());
                    sm[u"default_value"_s]   = ss.defaultValue();
                    sm[u"identity"_s]        = ss.identity();
                    sm[u"label_key"_s]       = ss.labelKey();
                    sm[u"description_key"_s] = ss.descriptionKey();
                    sm[u"label"_s]           = pluginMessageFromPb(ss.label(), ss.labelKey());
                    sm[u"description"_s] =
                        pluginMessageFromPb(ss.description(), ss.descriptionKey());
                    sm[u"group_label"_s] = pluginMessageFromPb(ss.groupLabel(), ss.group());
                    sm[u"min"_s]         = ss.min();
                    sm[u"max"_s]         = ss.max();
                    sm[u"step"_s]        = ss.step();
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
                    am[u"labelText"_s]         = pluginMessageFromPb(a.labelText(), a.label());
                    am[u"descriptionText"_s] =
                        pluginMessageFromPb(a.descriptionText(), a.description());
                    am[u"browseDescriptionText"_s] =
                        pluginMessageFromPb(a.browseDescriptionText(), a.browseDescription());
                    am[u"browseButtonLabelText"_s] =
                        pluginMessageFromPb(a.browseButtonLabelText(), a.browseButtonLabel());
                    am[u"groupLabel"_s] = pluginMessageFromPb(a.groupLabel(), a.group());
                    am[u"order"_s]      = static_cast<int>(a.order());
                    am[u"kind"_s]       = static_cast<int>(a.kind());
                    am[u"visible"_s]    = a.visible();
                    am[u"enabled"_s]    = a.enabled();
                    QVariantList fields;
                    for (const auto& field : a.fields()) {
                        QVariantMap fm;
                        fm[u"key"_s]         = field.key();
                        fm[u"label"_s]       = field.label();
                        fm[u"description"_s] = field.description();
                        fm[u"placeholder"_s] = field.placeholder();
                        fm[u"labelText"_s] = pluginMessageFromPb(field.labelText(), field.label());
                        fm[u"descriptionText"_s] =
                            pluginMessageFromPb(field.descriptionText(), field.description());
                        fm[u"placeholderText"_s] =
                            pluginMessageFromPb(field.placeholderText(), field.placeholder());
                        fm[u"secret"_s]   = field.secret();
                        fm[u"required"_s] = field.required();
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
                    sm[u"id"_s]         = st.id_proto();
                    sm[u"label"_s]      = st.label();
                    sm[u"labelText"_s]  = pluginMessageFromPb(st.labelText(), st.label());
                    sm[u"groupLabel"_s] = pluginMessageFromPb(st.groupLabel(), st.group());
                    sm[u"group"_s]      = st.group();
                    sm[u"order"_s]      = static_cast<int>(st.order());
                    sm[u"value"_s]      = st.value();
                    sm[u"valueText"_s]  = pluginMessageFromPb(st.valueText(), st.value());
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
    connect(this, &RemoteSearchQuery::statusChanged, this, &RemoteSearchQuery::stateChanged);
    connect(this, &RemoteSearchQuery::sourceIdChanged, this, &RemoteSearchQuery::invalidateSearch);
    connect(this, &RemoteSearchQuery::queryChanged, this, &RemoteSearchQuery::invalidateSearch);
    connect(this, &RemoteSearchQuery::sortKeyChanged, this, &RemoteSearchQuery::invalidateSearch);
    connect(this, &RemoteSearchQuery::tagsChanged, this, &RemoteSearchQuery::invalidateSearch);
    connect(this, &RemoteSearchQuery::excludedTagsChanged, this, &RemoteSearchQuery::invalidateSearch);
    connect(this, &RemoteSearchQuery::filterMatchModesChanged, this, &RemoteSearchQuery::invalidateSearch);
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
auto RemoteSearchQuery::excludedTags() const -> const QStringList& { return m_excluded_tags; }
void RemoteSearchQuery::setExcludedTags(const QStringList& v) {
    if (m_excluded_tags == v) return;
    m_excluded_tags = v;
    Q_EMIT excludedTagsChanged();
}
auto RemoteSearchQuery::filterMatchModes() const -> const QVariantMap& { return m_filter_match_modes; }
void RemoteSearchQuery::setFilterMatchModes(const QVariantMap& v) {
    if (m_filter_match_modes == v) return;
    m_filter_match_modes = v;
    Q_EMIT filterMatchModesChanged();
}
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
        invalidateSearch();
    } else {
        invalidateRequests();
        clearResults();
    }
}

auto RemoteSearchQuery::model() const -> model::RemoteListModel* { return tdata(); }
auto RemoteSearchQuery::hasMore() const -> bool {
    const auto t = model();
    return t && t->hasMore();
}
auto RemoteSearchQuery::errorText() const -> const QString& { return error(); }

void RemoteSearchQuery::reload() {
    if (! m_browsing_enabled || m_source_id.isEmpty()) {
        clearResults();
        return;
    }
    setOffset(0);
    setNoMore(false);
    m_next_cursor.clear();
    m_seen_cursors.clear();
    fetchPage(1, false);
}

void RemoteSearchQuery::loadMore() {
    if (! m_browsing_enabled || m_reload_pending || noMore() || querying()) return;
    fetchPage(static_cast<quint32>(offset() + 2), true);
}

void RemoteSearchQuery::fetchMore(qint32) {
    if (! m_browsing_enabled || m_reload_pending || noMore() || querying()) return;
    fetchPage(static_cast<quint32>(offset() + 2), true);
}

void RemoteSearchQuery::retry() {
    if (status() != Status::Error) return;
    if (m_failed_append)
        loadMore();
    else
        reload();
}

void RemoteSearchQuery::invalidateSearch() {
    m_reload_pending = true;
    m_failed_append  = false;
    m_next_cursor.clear();
    m_seen_cursors.clear();
    if (auto t = model()) t->setHasMore(false);
    delayReload();
}

void RemoteSearchQuery::clearResults() {
    m_reload_pending = false;
    m_failed_append  = false;
    setOffset(0);
    setNoMore(true);
    m_next_cursor.clear();
    m_seen_cursors.clear();
    setError({});
    if (auto t = model()) t->reset({}, false);
    setStatus(Status::Finished);
    Q_EMIT stateChanged();
}

void RemoteSearchQuery::fetchPage(quint32 page, bool append) {
    auto t = model();
    if (! t) return;
    m_reload_pending = false;
    t->setHasMore(false);
    m_failed_append = append;
    setError({});
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::RemoteSearchRequest {};
    inner.setSourceId(m_source_id);
    inner.setQuery(m_query);
    inner.setSortKey(m_sort_key);
    inner.setPage(page);
    inner.setRequiredTags(m_tags);
    inner.setExcludedTags(m_excluded_tags);
    QHash<QString, QString> modes;
    for (auto it = m_filter_match_modes.cbegin(); it != m_filter_match_modes.cend(); ++it)
        modes.insert(it.key(), it.value().toString());
    inner.setFilterMatchModes(modes);
    if (append) inner.setCursor(m_next_cursor);
    req.setRemoteSearch(std::move(inner));

    const auto revision = beginRequest();
    auto       self     = QWatcher { this };
    spawn([self, backend, req = std::move(req), page, append, revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        if (! self->isCurrentRequest(revision)) co_return;

        if (! result) {
            self->setError(result.unwrap_err_unchecked());
            self->setStatus(Status::Error);
            co_return;
        }
        const auto& sr = result->remoteSearch();
        if (! sr.error().isEmpty()) {
            self->setError(sr.error());
            self->setStatus(Status::Error);
            co_return;
        }
        if (sr.hasMore() && ! sr.nextCursor().isEmpty() &&
            self->m_seen_cursors.contains(sr.nextCursor())) {
            self->setError(tr("The source returned a repeated continuation token."));
            self->setStatus(Status::Error);
            co_return;
        }
        {
            QList<model::RemoteRow> rows;
            rows.reserve(sr.items().size());
            for (const auto& it : sr.items()) {
                rows.push_back(AppStore::instance()->mergeRemoteRow(model::RemoteRow {
                    it.sourceId(),
                    it.id_proto(),
                    it.title(),
                    it.previewUrl(),
                    it.author(),
                    it.authorAvatarUrl(),
                    it.wpType(),
                    it.downloaded() ? 3 : 0,
                    it.hasWidth() ? it.width() : 0,
                    it.hasHeight() ? it.height() : 0,
                }));
            }
            auto t = self->model();
            if (! t) co_return;
            const bool more = sr.hasMore() && (! rows.isEmpty() || ! sr.nextCursor().isEmpty());
            self->setOffset(static_cast<qint32>(page - 1));
            self->setNoMore(! more);
            self->m_next_cursor = sr.nextCursor();
            if (! sr.nextCursor().isEmpty()) {
                if (self->m_seen_cursors.size() == 128) self->m_seen_cursors.removeFirst();
                self->m_seen_cursors.push_back(sr.nextCursor());
            }
            self->m_failed_append = false;
            if (append)
                t->append(rows, more);
            else
                t->reset(std::move(rows), more);
            Q_EMIT self->stateChanged();
        }
        self->setStatus(Status::Finished);
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
auto RemoteDetailsQuery::authorAvatarUrl() const -> const QString& { return m_author_avatar_url; }
auto RemoteDetailsQuery::authorUrl() const -> const QString& { return m_author_url; }
auto RemoteDetailsQuery::description() const -> const QString& { return m_description; }
auto RemoteDetailsQuery::size() const -> const QString& { return m_size; }
auto RemoteDetailsQuery::width() const -> int { return m_width; }
auto RemoteDetailsQuery::height() const -> int { return m_height; }
auto RemoteDetailsQuery::tags() const -> const QStringList& { return m_tags; }
auto RemoteDetailsQuery::webUrl() const -> const QString& { return m_web_url; }
auto RemoteDetailsQuery::fields() const -> const QVariantList& { return m_fields; }

void RemoteDetailsQuery::reload() {
    const auto revision = beginRequest();
    m_author.clear();
    m_author_avatar_url.clear();
    m_author_url.clear();
    m_description.clear();
    m_size.clear();
    m_width  = 0;
    m_height = 0;
    m_tags.clear();
    m_web_url.clear();
    m_fields.clear();
    setError({});
    Q_EMIT loaded();
    if (m_item_id.isEmpty()) {
        setStatus(Status::Finished);
        return;
    }
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::RemoteDetailsRequest {};
    inner.setSourceId(m_source_id);
    inner.setId_proto(m_item_id);
    req.setRemoteDetails(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

        if (! result) {
            self->setError(result.unwrap_err_unchecked());
            self->setStatus(Status::Error);
            co_return;
        }
        const auto& dr = result->remoteDetails();
        if (! dr.error().isEmpty()) {
            self->setError(dr.error());
            self->setStatus(Status::Error);
            co_return;
        }
        {
            self->m_author            = dr.author();
            self->m_author_avatar_url = dr.authorAvatarUrl();
            self->m_author_url        = dr.authorUrl();
            self->m_description       = dr.description();
            self->m_size              = dr.size();
            self->m_width             = static_cast<int>(dr.width());
            self->m_height            = static_cast<int>(dr.height());
            self->m_web_url           = dr.webUrl();
            self->m_tags.clear();
            for (const auto& t : dr.tags()) self->m_tags.push_back(t);
            for (const auto& field : dr.fields()) {
                QVariantMap row;
                row[u"key"_s]       = field.key();
                row[u"label"_s]     = field.label();
                row[u"labelText"_s] = pluginMessageFromPb(field.labelText(), field.label());
                row[u"value"_s]     = field.value();
                const QUrl url(field.url());
                if (url.isValid() && ! url.host().isEmpty() &&
                    (url.scheme() == u"https"_s || url.scheme() == u"http"_s)) {
                    row[u"url"_s] = url;
                }
                self->m_fields.push_back(row);
            }
            Q_EMIT self->loaded();
        }
        self->setStatus(Status::Finished);
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
    const auto revision = beginRequest();
    setStatus(Status::Querying);
    auto backend = App::instance()->backend();

    auto req   = proto::Request {};
    auto inner = proto::SubscriptionStatusRequest {};
    inner.setSourceId(sourceId);
    inner.setItemIds({ id });
    req.setSubscriptionStatus(std::move(inner));

    auto self = QWatcher { this };
    spawn([self, backend, req = std::move(req), sourceId, id, revision]() mutable -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self) co_return;
        if (! self->isCurrentRequest(revision)) co_return;

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
            Q_EMIT self->stateLoaded(
                sourceId, id, state, pluginMessageFromPb(status.errorText(), status.error()));
        });
        co_return;
    });
}

void RemoteSubscriptionQuery::setSubscribed(const QString& sourceId, const QString& id,
                                            bool subscribed) {
    const auto revision = beginRequest();
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
    spawn([self, backend, req = std::move(req), sourceId, id, subscribed, revision]() mutable
              -> task<void> {
        auto result = co_await backend->send(std::move(req));
        if (! co_await QAsyncResult::qexecutor()) co_return;
        if (! self || ! self->isCurrentRequest(revision)) co_return;

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
            Q_EMIT self->setFinished(sourceId,
                                     id,
                                     subscribed,
                                     update.accepted(),
                                     pluginMessageFromPb(update.errorText(), update.error()));
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
