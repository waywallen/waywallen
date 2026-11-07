module;
#include "QExtra/macro_qt.hpp"
#ifdef Q_MOC_RUN
#    include "waywallen/thumb/service.moc"
#    include <QtQml/QQmlParserStatus>
#endif

export module waywallen:thumb.service;
import qextra.qt;

namespace waywallen
{
export class ThumbnailRequest;

class ThumbnailJob : public QObject, public QRunnable {
    Q_OBJECT
public:
    ThumbnailJob(quint64 id, QString key, QString path, QString cache_root, bool timeline,
                 int maximum_edge, QObject* parent);
    void          run() override;
    Q_SIGNAL void finished(quint64 id, const QString& key, const QString& cache_path,
                           const QString& error);

private:
    quint64 m_id;
    QString m_key;
    QString m_path;
    QString m_cache_root;
    bool    m_timeline;
    int     m_maximum_edge;
};

class ThumbnailService : public QObject {
    Q_OBJECT
public:
    static ThumbnailService* instance();
    ~ThumbnailService() override;
    void submit(ThumbnailRequest* request, quint64 revision, const QString& path, bool timeline,
                int maximum_edge, bool prefetch);
    void cancel(ThumbnailRequest* request);
    void reprioritize();

private:
    explicit ThumbnailService(QObject* parent);
    struct Subscription {
        QPointer<ThumbnailRequest> request;
        quint64                    revision;
    };
    struct Pending {
        quint64             id;
        ThumbnailJob*       job;
        int                 priority;
        QList<Subscription> subscribers;
    };
    void                    onFinished(quint64 id, const QString& key, const QString& cache_path,
                                       const QString& error);
    QThreadPool             m_pool;
    QHash<QString, Pending> m_pending;
    quint64                 m_next_id = 0;
};

export class ThumbnailRequest : public QObject, public QQmlParserStatus {
    Q_OBJECT
    Q_INTERFACES(QQmlParserStatus)
    QML_ELEMENT

    Q_PROPERTY(QString source READ source WRITE setSource NOTIFY sourceChanged FINAL)
    Q_PROPERTY(QString resource READ resource WRITE setResource NOTIFY resourceChanged FINAL)
    Q_PROPERTY(QString wpType READ wpType WRITE setWpType NOTIFY wpTypeChanged FINAL)
    Q_PROPERTY(bool timeline READ timeline WRITE setTimeline NOTIFY timelineChanged FINAL)
    Q_PROPERTY(bool enabled READ enabled WRITE setEnabled NOTIFY enabledChanged FINAL)
    Q_PROPERTY(bool prefetch READ prefetch WRITE setPrefetch NOTIFY prefetchChanged FINAL)
    Q_PROPERTY(bool retainWhileLoading READ retainWhileLoading WRITE setRetainWhileLoading NOTIFY
                   retainWhileLoadingChanged FINAL)
    Q_PROPERTY(
        int maximumEdge READ maximumEdge WRITE setMaximumEdge NOTIFY maximumEdgeChanged FINAL)
    Q_PROPERTY(State state READ state NOTIFY stateChanged FINAL)
    Q_PROPERTY(QUrl cachePath READ cachePath NOTIFY cachePathChanged FINAL)
    Q_PROPERTY(QString error READ error NOTIFY errorChanged FINAL)
public:
    enum State
    {
        Idle,
        Loading,
        Ready,
        Failed
    };
    Q_ENUM(State)
    explicit ThumbnailRequest(QObject* parent = nullptr);
    ~ThumbnailRequest() override;

    QString source() const { return m_source; }
    void    setSource(const QString& value);
    QString resource() const { return m_resource; }
    void    setResource(const QString& value);
    QString wpType() const { return m_wp_type; }
    void    setWpType(const QString& value);
    bool    timeline() const { return m_timeline; }
    void    setTimeline(bool value);
    bool    enabled() const { return m_enabled; }
    void    setEnabled(bool value);
    bool    prefetch() const { return m_prefetch; }
    void    setPrefetch(bool value);
    bool    retainWhileLoading() const { return m_retain; }
    void    setRetainWhileLoading(bool value);
    int     maximumEdge() const { return m_maximum_edge; }
    void    setMaximumEdge(int value);
    State   state() const { return m_state; }
    QUrl    cachePath() const { return m_cache_path; }
    QString error() const { return m_error; }

    void             classBegin() override;
    void             componentComplete() override;
    Q_INVOKABLE void reload();

    Q_SIGNAL void sourceChanged();
    Q_SIGNAL void resourceChanged();
    Q_SIGNAL void wpTypeChanged();
    Q_SIGNAL void timelineChanged();
    Q_SIGNAL void enabledChanged();
    Q_SIGNAL void prefetchChanged();
    Q_SIGNAL void retainWhileLoadingChanged();
    Q_SIGNAL void maximumEdgeChanged();
    Q_SIGNAL void stateChanged();
    Q_SIGNAL void cachePathChanged();
    Q_SIGNAL void errorChanged();

private:
    friend class ThumbnailService;
    void invalidate();
    void submit();
    void publish(quint64 revision, State state, QUrl path, QString error = {});

    QString m_source;
    QString m_resource;
    QString m_wp_type;
    bool    m_timeline     = false;
    bool    m_enabled      = true;
    bool    m_prefetch     = false;
    bool    m_retain       = false;
    bool    m_complete     = false;
    bool    m_scheduled    = false;
    int     m_maximum_edge = 512;
    quint64 m_revision     = 0;
    State   m_state        = Idle;
    QUrl    m_cache_path;
    QString m_error;
};
} // namespace waywallen
