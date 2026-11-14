module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/library_query.moc"
#endif

export module waywallen:query.library;
export import :query.query;

namespace waywallen
{

export class LibraryListQuery : public Query,
                                public QueryExtra<control::v1::Response, LibraryListQuery> {
    Q_OBJECT
    QML_ELEMENT

public:
    LibraryListQuery(QObject* parent = nullptr);

    void reload() override;
};

export class LibraryAddQuery : public Query,
                               public QueryExtra<control::v1::Response, LibraryAddQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantMap settings MEMBER m_settings NOTIFY settingsChanged)
    Q_PROPERTY(QString path READ path WRITE setPath NOTIFY pathChanged)
    Q_PROPERTY(QString name READ name WRITE setName NOTIFY nameChanged)
    Q_PROPERTY(QString pluginName READ pluginName WRITE setPluginName NOTIFY pluginNameChanged)

public:
    LibraryAddQuery(QObject* parent = nullptr);

    auto path() const -> const QString&;
    void setPath(const QString& v);

    auto name() const -> const QString&;
    void setName(const QString& v);

    auto pluginName() const -> const QString&;
    void setPluginName(const QString& v);

    void reload() override;

    Q_SIGNAL void settingsChanged();
    Q_SIGNAL void pathChanged();
    Q_SIGNAL void nameChanged();
    Q_SIGNAL void pluginNameChanged();

private:
    QVariantMap m_settings;
    QString     m_path;
    QString     m_name;
    QString     m_plugin_name;
};

export class LibraryEditQuery : public Query,
                                public QueryExtra<control::v1::Response, LibraryEditQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(qint64 libraryId READ libraryId WRITE setLibraryId NOTIFY libraryIdChanged)
    Q_PROPERTY(QString name READ name WRITE setName NOTIFY nameChanged)
    Q_PROPERTY(QVariantMap settings MEMBER m_settings NOTIFY settingsChanged)

public:
    LibraryEditQuery(QObject* parent = nullptr);

    auto libraryId() const -> qint64;
    void setLibraryId(qint64 v);
    auto name() const -> const QString&;
    void setName(const QString& v);
    void reload() override;

    Q_SIGNAL void libraryIdChanged();
    Q_SIGNAL void nameChanged();
    Q_SIGNAL void settingsChanged();

private:
    qint64      m_library_id = 0;
    QString     m_name;
    QVariantMap m_settings;
};

export class LibraryRelocateQuery : public Query,
                                    public QueryExtra<control::v1::Response, LibraryRelocateQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(qint64 libraryId READ libraryId WRITE setLibraryId NOTIFY libraryIdChanged)
    Q_PROPERTY(QString path READ path WRITE setPath NOTIFY pathChanged)

public:
    LibraryRelocateQuery(QObject* parent = nullptr);

    auto libraryId() const -> qint64;
    void setLibraryId(qint64 v);
    auto path() const -> const QString&;
    void setPath(const QString& v);
    void reload() override;

    Q_SIGNAL void libraryIdChanged();
    Q_SIGNAL void pathChanged();

private:
    qint64  m_library_id = 0;
    QString m_path;
};

export class LibraryAutoDetectQuery
    : public Query,
      public QueryExtra<control::v1::Response, LibraryAutoDetectQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(qint32 addedCount READ addedCount NOTIFY addedCountChanged)

public:
    LibraryAutoDetectQuery(QObject* parent = nullptr);

    auto addedCount() const -> qint32;

    void reload() override;

    Q_SIGNAL void addedCountChanged();

private:
    qint32 m_added_count = 0;
};

export class LibraryRemoveQuery : public Query,
                                  public QueryExtra<control::v1::Response, LibraryRemoveQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(qint64 libraryId READ libraryId WRITE setLibraryId NOTIFY libraryIdChanged)

public:
    LibraryRemoveQuery(QObject* parent = nullptr);

    auto libraryId() const -> qint64;
    void setLibraryId(qint64 v);

    void reload() override;

    Q_SIGNAL void libraryIdChanged();

private:
    qint64 m_library_id = 0;
};

} // namespace waywallen
