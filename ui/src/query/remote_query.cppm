module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/remote_query.moc"
#endif

export module waywallen:query.remote;
export import :query.query;
export import :model.remote;

namespace waywallen
{

export class RemoteAvailabilityQuery
    : public Query,
      public QueryExtra<control::v1::Response, RemoteAvailabilityQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantList sources READ sources NOTIFY sourcesChanged FINAL)
    Q_PROPERTY(QString defaultSourceId READ defaultSourceId NOTIFY sourcesChanged FINAL)

public:
    RemoteAvailabilityQuery(QObject* parent = nullptr);

    auto sources() const -> const QVariantList&;
    auto defaultSourceId() const -> const QString&;

    void reload() override;

    Q_SIGNAL void sourcesChanged();

private:
    QVariantList m_sources;
    QString      m_default_source_id;
};

export class RemoteSearchQuery : public QueryList,
                                 public QueryExtra<model::RemoteListModel, RemoteSearchQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString sourceId READ sourceId WRITE setSourceId NOTIFY sourceIdChanged FINAL)
    Q_PROPERTY(QString query READ query WRITE setQuery NOTIFY queryChanged FINAL)
    Q_PROPERTY(QString sortKey READ sortKey WRITE setSortKey NOTIFY sortKeyChanged FINAL)
    Q_PROPERTY(QStringList tags READ tags WRITE setTags NOTIFY tagsChanged FINAL)
    Q_PROPERTY(bool browsingEnabled READ browsingEnabled WRITE setBrowsingEnabled NOTIFY
                   browsingEnabledChanged FINAL)
    Q_PROPERTY(waywallen::model::RemoteListModel* model READ model CONSTANT FINAL)
    Q_PROPERTY(bool hasMore READ hasMore NOTIFY stateChanged FINAL)
    Q_PROPERTY(QString errorText READ errorText NOTIFY stateChanged FINAL)

public:
    RemoteSearchQuery(QObject* parent = nullptr);

    auto sourceId() const -> const QString&;
    void setSourceId(const QString&);

    auto query() const -> const QString&;
    void setQuery(const QString&);

    auto sortKey() const -> const QString&;
    void setSortKey(const QString&);

    auto tags() const -> const QStringList&;
    void setTags(const QStringList&);

    auto browsingEnabled() const -> bool;
    void setBrowsingEnabled(bool);

    auto model() const -> model::RemoteListModel*;
    auto hasMore() const -> bool;
    auto errorText() const -> const QString&;

    void             reload() override;
    Q_INVOKABLE void loadMore();
    Q_SLOT void      fetchMore(qint32) override;

    Q_SIGNAL void sourceIdChanged();
    Q_SIGNAL void queryChanged();
    Q_SIGNAL void sortKeyChanged();
    Q_SIGNAL void tagsChanged();
    Q_SIGNAL void browsingEnabledChanged();
    Q_SIGNAL void stateChanged();

private:
    void clearResults();
    void fetchPage(quint32 page, bool append);

    QString     m_source_id;
    QString     m_query;
    QString     m_sort_key;
    QStringList m_tags;
    QString     m_error;
    bool        m_browsing_enabled { false };
};

export class RemoteDetailsQuery : public Query,
                                  public QueryExtra<control::v1::Response, RemoteDetailsQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString sourceId READ sourceId WRITE setSourceId NOTIFY sourceIdChanged FINAL)
    Q_PROPERTY(QString itemId READ itemId WRITE setItemId NOTIFY itemIdChanged FINAL)
    Q_PROPERTY(QString author READ author NOTIFY loaded FINAL)
    Q_PROPERTY(QString authorAvatarUrl READ authorAvatarUrl NOTIFY loaded FINAL)
    Q_PROPERTY(QString description READ description NOTIFY loaded FINAL)
    Q_PROPERTY(QString size READ size NOTIFY loaded FINAL)
    Q_PROPERTY(int width READ width NOTIFY loaded FINAL)
    Q_PROPERTY(int height READ height NOTIFY loaded FINAL)
    Q_PROPERTY(QStringList tags READ tags NOTIFY loaded FINAL)
    Q_PROPERTY(QString webUrl READ webUrl NOTIFY loaded FINAL)

public:
    RemoteDetailsQuery(QObject* parent = nullptr);

    auto sourceId() const -> const QString&;
    void setSourceId(const QString&);

    auto itemId() const -> const QString&;
    void setItemId(const QString&);
    auto author() const -> const QString&;
    auto authorAvatarUrl() const -> const QString&;
    auto description() const -> const QString&;
    auto size() const -> const QString&;
    auto width() const -> int;
    auto height() const -> int;
    auto tags() const -> const QStringList&;
    auto webUrl() const -> const QString&;

    void reload() override;

    Q_SIGNAL void sourceIdChanged();
    Q_SIGNAL void itemIdChanged();
    Q_SIGNAL void loaded();

private:
    QString     m_source_id;
    QString     m_item_id;
    QString     m_author;
    QString     m_author_avatar_url;
    QString     m_description;
    QString     m_size;
    int         m_width { 0 };
    int         m_height { 0 };
    QStringList m_tags;
    QString     m_web_url;
};

export class RemoteDownloadQuery : public Query,
                                   public QueryExtra<control::v1::Response, RemoteDownloadQuery> {
    Q_OBJECT
    QML_ELEMENT

public:
    RemoteDownloadQuery(QObject* parent = nullptr);

    void             reload() override;
    Q_INVOKABLE void start(const QString& sourceId, const QString& id);
    Q_INVOKABLE void remove(const QString& sourceId, const QString& id);

    Q_SIGNAL void accepted(const QString& sourceId, const QString& id);
    Q_SIGNAL void rejected(const QString& sourceId, const QString& id, const QString& error);
    Q_SIGNAL void removed(const QString& sourceId, const QString& id);
    Q_SIGNAL void removeFailed(const QString& sourceId, const QString& id, const QString& error);
};

export class RemoteSubscriptionQuery
    : public Query,
      public QueryExtra<control::v1::Response, RemoteSubscriptionQuery> {
    Q_OBJECT
    QML_ELEMENT

public:
    RemoteSubscriptionQuery(QObject* parent = nullptr);

    void             reload() override;
    Q_INVOKABLE void refresh(const QString& sourceId, const QString& id);
    Q_INVOKABLE void setSubscribed(const QString& sourceId, const QString& id, bool subscribed);

    Q_SIGNAL void stateLoaded(const QString& sourceId, const QString& id, int state,
                              const QVariant& error);
    Q_SIGNAL void setFinished(const QString& sourceId, const QString& id, bool subscribed,
                              bool accepted, const QVariant& error);
};

export class RemoteSettingsPatchQuery
    : public Query,
      public QueryExtra<control::v1::Response, RemoteSettingsPatchQuery> {
    Q_OBJECT
    QML_ELEMENT

public:
    RemoteSettingsPatchQuery(QObject* parent = nullptr);

    void             reload() override;
    Q_INVOKABLE void patch(const QString& sourceId, const QVariantMap& values);

    Q_SIGNAL void completed(const QString& sourceId, const QVariantMap& values, bool accepted,
                            const QString& error);
};

} // namespace waywallen
