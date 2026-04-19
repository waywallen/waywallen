module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/renderer.moc"
#endif

export module waywallen:renderer;
export import :proto;
export import :backend;
import rstd;
import rstd.cppstd;
import qextra;

using rstd::boxed::Box;

namespace proto = waywallen::control::v1;

export namespace waywallen
{

/// One renderer, mirroring `proto::RendererInstance` as a QObject so
/// QML can bind directly to its fields. Identity is `id()`; mutate via
/// `updateFrom(info)` which diff-emits per changed property.
class Renderer : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Renderer instances are owned by RendererManager")

    Q_PROPERTY(QString id READ id CONSTANT FINAL)
    Q_PROPERTY(quint32 fps READ fps NOTIFY fpsChanged FINAL)
    Q_PROPERTY(QString status READ status NOTIFY statusChanged FINAL)
    Q_PROPERTY(QString name READ name NOTIFY nameChanged FINAL)
    Q_PROPERTY(quint32 pid READ pid NOTIFY pidChanged FINAL)

public:
    explicit Renderer(const proto::RendererInstance& info, QObject* parent = nullptr);

    auto id() const -> const QString& { return m_id; }
    auto fps() const -> quint32 { return m_fps; }
    auto status() const -> const QString& { return m_status; }
    auto name() const -> const QString& { return m_name; }
    auto pid() const -> quint32 { return m_pid; }

    /// Diff-update from a freshly-received `RendererInstance`. Only emits
    /// the signals for properties that actually changed.
    void updateFrom(const proto::RendererInstance& info);

    Q_SIGNAL void fpsChanged();
    Q_SIGNAL void statusChanged();
    Q_SIGNAL void nameChanged();
    Q_SIGNAL void pidChanged();

private:
    QString m_id;
    quint32 m_fps;
    QString m_status;
    QString m_name;
    quint32 m_pid;
};

/// Singleton model for all currently-registered renderers. Fed by:
///   1. the snapshot that arrives on ws connect (via `Backend::eventReceived`),
///   2. subsequent `RendererChanged` / `RendererRemoved` events,
///   3. `RendererListQuery::reload` as a fallback refresh path.
///
/// Consumers should prefer reading from `RendererManager` over issuing
/// a fresh `RendererListRequest` — the manager is push-updated.
class RendererManager : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_SINGLETON

    Q_PROPERTY(QVariantList renderers READ renderers NOTIFY renderersChanged FINAL)
    Q_PROPERTY(int count READ count NOTIFY renderersChanged FINAL)

public:
    RendererManager(QObject* parent = nullptr);
    ~RendererManager() override;

    static auto instance() -> RendererManager*;
    static auto create(QQmlEngine*, QJSEngine*) -> RendererManager*;

    // make qml prefer create
    RendererManager(const RendererManager&) = delete;

    /// Snapshot of all renderers (ordered by ascending id) as a list of
    /// `Renderer*`, suitable for QML `Repeater { model: RendererManager.renderers }`.
    auto renderers() const -> QVariantList;
    auto count() const -> int { return (int)m_ordered.size(); }

    Q_INVOKABLE waywallen::Renderer* get(const QString& id) const;

    /// Full replace. Removes any id not present in `list`, upserts the rest.
    /// Exactly-once `renderersChanged` after the batch.
    void replaceAll(const QList<proto::RendererInstance>& list);

    /// Upsert a single renderer; emits `renderersChanged` only if this
    /// was an add (removal/add changes the ordered list). Property
    /// changes on an existing renderer emit per-property signals.
    void upsert(const proto::RendererInstance& info);

    /// Remove by id. Emits `renderersChanged` if the id existed.
    void remove(const QString& id);

    /// Wire up to a backend's `eventReceived` signal. Call once from
    /// `App::init` after the backend is constructed.
    void attachTo(Backend* backend);

    Q_SIGNAL void renderersChanged();

private:
    void handleEvent(const proto::Event& evt);

    QList<Renderer*>                m_ordered;  // sorted by id
    cppstd::map<QString, Renderer*> m_by_id;
};

} // namespace waywallen
