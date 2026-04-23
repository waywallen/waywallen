module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/library_query.moc"
#endif

export module waywallen:query.library;
export import :query.query;

namespace waywallen
{

export class LibraryListQuery : public Query {
    Q_OBJECT
    QML_ELEMENT

public:
    LibraryListQuery(QObject* parent = nullptr);

    void reload() override;
};

export class LibraryAddQuery : public Query {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString path READ path WRITE setPath NOTIFY pathChanged)
    Q_PROPERTY(QString pluginName READ pluginName WRITE setPluginName NOTIFY pluginNameChanged)

public:
    LibraryAddQuery(QObject* parent = nullptr);

    auto path() const -> const QString&;
    void setPath(const QString& v);

    auto pluginName() const -> const QString&;
    void setPluginName(const QString& v);

    void reload() override;

    Q_SIGNAL void pathChanged();
    Q_SIGNAL void pluginNameChanged();

private:
    QString m_path;
    QString m_plugin_name;
};

export class LibraryRemoveQuery : public Query {
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
