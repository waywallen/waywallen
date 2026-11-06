module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/playlist_query.moc"
#    include <QtQml/QQmlParserStatus>
#endif

export module waywallen:query.playlist;
export import :query.query;
export import :model.list_models;

namespace waywallen
{

export class PlaylistListQuery : public Query,
                                 public QueryExtra<control::v1::Response, PlaylistListQuery> {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(QVariantList playlists READ playlists NOTIFY playlistsChanged FINAL)
public:
    PlaylistListQuery(QObject* parent = nullptr);
    auto          playlists() const -> const QVariantList&;
    void          reload() override;
    Q_SIGNAL void playlistsChanged();

private:
    QVariantList m_playlists;
};

export class PlaylistDetailQuery
    : public QueryList,
      public QueryExtra<model::WallpaperListModel, PlaylistDetailQuery>,
      public QQmlParserStatus {
    Q_OBJECT
    Q_INTERFACES(QQmlParserStatus)
    QML_ELEMENT
    Q_PROPERTY(qint64 playlistId READ playlistId WRITE setPlaylistId NOTIFY playlistIdChanged FINAL)
    Q_PROPERTY(QVariantMap playlist READ playlist NOTIFY playlistChanged FINAL)
    Q_PROPERTY(qint64 revision READ revision NOTIFY playlistChanged FINAL)
    Q_PROPERTY(QStringList entryIds READ entryIds NOTIFY playlistChanged FINAL)
public:
    PlaylistDetailQuery(QObject* parent = nullptr);

    auto playlistId() const -> qint64;
    void setPlaylistId(qint64 id);
    auto playlist() const -> const QVariantMap&;
    auto revision() const -> qint64;
    auto entryIds() const -> const QStringList&;

    void classBegin() override;
    void componentComplete() override;
    void reload() override;

    Q_SIGNAL void playlistIdChanged();
    Q_SIGNAL void playlistChanged();

private:
    qint64      m_playlist_id { 0 };
    QVariantMap m_playlist;
    qint64      m_revision { 0 };
    QStringList m_entry_ids;
};

export class PlaylistMutationQuery
    : public Query,
      public QueryExtra<control::v1::Response, PlaylistMutationQuery> {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(qint64 createdId READ createdId NOTIFY createdIdChanged FINAL)
public:
    PlaylistMutationQuery(QObject* parent = nullptr);
    qint64 createdId() const { return m_createdId; }

    Q_INVOKABLE void create(const QString& name, int mode, int intervalSecs,
                            bool synchronizedSelection, const QVariantList& itemIds);
    Q_INVOKABLE void remove(qint64 id);
    Q_INVOKABLE void saveSmart(qint64 id, const QString& name, int mode, int intervalSecs,
                               bool synchronizedSelection, const QVariant& selection,
                               qint64 expectedRevision);
    Q_INVOKABLE void update(qint64 id, const QString& name, int mode, int intervalSecs,
                            bool synchronizedSelection, const QVariantList& itemIds,
                            qint64 expectedRevision);
    Q_INVOKABLE void rename(qint64 id, const QString& name);
    Q_INVOKABLE void setItems(qint64 id, const QVariantList& itemIds, qint64 expectedRevision);
    Q_INVOKABLE void setMode(qint64 id, int mode);
    Q_INVOKABLE void setInterval(qint64 id, int intervalSecs);
    Q_INVOKABLE void activate(qint64 id, const QVariantList& targets, bool autoAttach);
    Q_INVOKABLE void deactivate(const QVariantList& targets, qint64 clearAutoAttach);
    Q_INVOKABLE void jumpTo(qint64 id, const QString& entryId);

    void reload() override {}

    Q_SIGNAL void createdIdChanged();
    Q_SIGNAL void done();

private:
    void   send(proto::Request req, bool captureCreate);
    qint64 m_createdId = 0;
};

} // namespace waywallen
