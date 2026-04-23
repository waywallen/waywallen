module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/library.moc"
#endif

export module waywallen:library;
export import :proto;
export import :backend;
import rstd;
import rstd.cppstd;
import qextra;

using rstd::boxed::Box;

namespace proto = waywallen::control::v1;

export namespace waywallen
{

/// One library, mirroring `proto::LibraryInstance` as a QObject so QML can
/// bind directly to its fields. Identity is `id()`; mutate via
/// `updateFrom(info)` which diff-emits per changed property.
class Library : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Library instances are owned by LibraryManager")

    Q_PROPERTY(qint64 id READ id CONSTANT FINAL)
    Q_PROPERTY(QString path READ path NOTIFY pathChanged FINAL)
    Q_PROPERTY(QString pluginName READ pluginName NOTIFY pluginNameChanged FINAL)

public:
    explicit Library(const proto::LibraryInstance& info, QObject* parent = nullptr);

    auto id() const -> qint64 { return m_id; }
    auto path() const -> const QString& { return m_path; }
    auto pluginName() const -> const QString& { return m_plugin_name; }

    /// Diff-update from a freshly-received `LibraryInstance`. Only emits
    /// the signals for properties that actually changed.
    void updateFrom(const proto::LibraryInstance& info);

    Q_SIGNAL void pathChanged();
    Q_SIGNAL void pluginNameChanged();

private:
    qint64  m_id;
    QString m_path;
    QString m_plugin_name;
};

/// Singleton model for all currently-registered libraries. Fed by:
///   1. the snapshot that arrives on ws connect (via `Backend::eventReceived`),
///   2. subsequent `LibraryChanged` / `LibraryRemoved` events,
///   3. `LibraryListQuery::reload` as a fallback refresh path.
///
/// Consumers should prefer reading from `LibraryManager` over issuing
/// a fresh `LibraryListRequest` — the manager is push-updated.
class LibraryManager : public QObject {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantList libraries READ libraries NOTIFY librariesChanged FINAL)
    Q_PROPERTY(int count READ count NOTIFY librariesChanged FINAL)

public:
    LibraryManager(QObject* parent = nullptr);
    ~LibraryManager() override;

    static auto instance() -> LibraryManager*;

    /// Snapshot of all libraries (ordered by ascending id) as a list of
    /// `Library*`, suitable for QML `Repeater { model: LibraryManager.libraries }`.
    auto libraries() const -> QVariantList;
    auto count() const -> int { return (int)m_ordered.size(); }

    Q_INVOKABLE waywallen::Library* get(qint64 id) const;

    /// Full replace. Removes any id not present in `list`, upserts the rest.
    /// Exactly-once `librariesChanged` after the batch.
    void replaceAll(const QList<proto::LibraryInstance>& list);

    /// Upsert a single library; emits `librariesChanged` only if this
    /// was an add (removal/add changes the ordered list). Property
    /// changes on an existing library emit per-property signals.
    void upsert(const proto::LibraryInstance& info);

    /// Remove by id. Emits `librariesChanged` if the id existed.
    void remove(qint64 id);

    /// Wire up to a backend's `eventReceived` signal. Call once from
    /// `App::init` after the backend is constructed.
    void attachTo(Backend* backend);

    Q_SIGNAL void librariesChanged();

private:
    void handleEvent(const proto::Event& evt);

    QList<Library*>               m_ordered;  // sorted by id
    cppstd::map<qint64, Library*> m_by_id;
};

} // namespace waywallen
