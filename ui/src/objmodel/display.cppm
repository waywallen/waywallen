module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/objmodel/display.moc"
#endif

export module waywallen:display;
export import :proto;
export import :backend;
import rstd;
import rstd.cppstd;
import qextra;

using rstd::boxed::Box;

namespace proto = waywallen::control::v1;

export namespace waywallen
{

/// One display, mirroring `proto::DisplayInfo` as a QObject so QML can
/// bind directly to its fields. Identity is `id()`; mutate via
/// `updateFrom(info)` which diff-emits per changed property.
class Display : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Display instances are owned by DisplayManager")

    Q_PROPERTY(quint64 id READ id CONSTANT FINAL)
    Q_PROPERTY(QString name READ name NOTIFY nameChanged FINAL)
    Q_PROPERTY(QString alias READ alias NOTIFY aliasChanged FINAL)
    Q_PROPERTY(QString displayLabel READ displayLabel NOTIFY displayLabelChanged FINAL)
    Q_PROPERTY(quint32 width READ width NOTIFY sizeChanged FINAL)
    Q_PROPERTY(quint32 height READ height NOTIFY sizeChanged FINAL)
    Q_PROPERTY(quint32 refreshMhz READ refreshMhz NOTIFY refreshMhzChanged FINAL)
    Q_PROPERTY(QVariantList links READ links NOTIFY linksChanged FINAL)
    /// Resolved layout currently in use for this display
    /// (per-display override on top of global defaults). Map keys:
    /// `fillmode` (int), `locationX` / `locationY` (0..100).
    Q_PROPERTY(QVariantMap effectiveLayout READ effectiveLayout NOTIFY layoutChanged FINAL)
    Q_PROPERTY(QVariantMap displayLayout READ displayLayout NOTIFY layoutChanged FINAL)
    /// Sparse per-display override. Same key set as effectiveLayout
    /// plus `fillmodeSet` / `locationSet` booleans
    /// indicating whether each field is explicitly overridden vs. inherited.
    Q_PROPERTY(QVariantMap layoutOverride READ layoutOverride NOTIFY layoutChanged FINAL)
    // DRM render-node id of the GPU this display's consumer is on.
    // Set once at register_display time; never changes for a live display.
    Q_PROPERTY(quint32 drmRenderMajor READ drmRenderMajor CONSTANT FINAL)
    Q_PROPERTY(quint32 drmRenderMinor READ drmRenderMinor CONSTANT FINAL)
    Q_PROPERTY(qint64 activePlaylistId READ activePlaylistId NOTIFY playlistStatusChanged FINAL)
    Q_PROPERTY(QVariantMap playlistStatus READ playlistStatus NOTIFY playlistStatusChanged FINAL)
    Q_PROPERTY(QString lockWallpaper READ lockWallpaper NOTIFY lockWallpaperChanged FINAL)
    Q_PROPERTY(bool hasLockScreen READ hasLockScreen NOTIFY hasLockScreenChanged FINAL)
    /// Resolved lock-screen layout; the Displays-tab lock section binds here.
    Q_PROPERTY(QVariantMap lockDisplayLayout READ lockDisplayLayout NOTIFY lockLayoutChanged FINAL)
    /// Sparse per-display lock-screen layout override.
    Q_PROPERTY(QVariantMap lockLayoutOverride READ lockLayoutOverride NOTIFY lockLayoutChanged FINAL)

public:
    explicit Display(const proto::DisplayInfo& info, QObject* parent = nullptr);

    auto id() const -> quint64 { return m_id; }
    auto name() const -> const QString& { return m_name; }
    auto alias() const -> const QString& { return m_alias; }
    auto displayLabel() const -> QString { return m_alias.isEmpty() ? m_name : m_alias; }
    auto width() const -> quint32 { return m_width; }
    auto height() const -> quint32 { return m_height; }
    auto refreshMhz() const -> quint32 { return m_refresh_mhz; }
    auto links() const -> const QVariantList& { return m_links; }
    auto effectiveLayout() const -> const QVariantMap& { return m_effective_layout; }
    auto displayLayout() const -> const QVariantMap& { return m_display_layout; }
    auto layoutOverride() const -> const QVariantMap& { return m_layout_override; }
    auto drmRenderMajor() const -> quint32 { return m_drm_render_major; }
    auto drmRenderMinor() const -> quint32 { return m_drm_render_minor; }
    auto activePlaylistId() const -> qint64 { return m_active_playlist_id; }
    auto playlistStatus() const -> const QVariantMap& { return m_playlist_status; }
    auto lockWallpaper() const -> const QString& { return m_lock_wallpaper; }
    auto hasLockScreen() const -> bool { return m_has_lock_screen; }
    auto lockDisplayLayout() const -> const QVariantMap& { return m_lock_display_layout; }
    auto lockLayoutOverride() const -> const QVariantMap& { return m_lock_layout_override; }

    /// Diff-update from a freshly-received `DisplayInfo`. Only emits
    /// the signals for properties that actually changed.
    void updateFrom(const proto::DisplayInfo& info);
    void updatePlaylistStatus(const proto::PlaylistDisplayStatus* status);

    Q_SIGNAL void nameChanged();
    Q_SIGNAL void aliasChanged();
    Q_SIGNAL void displayLabelChanged();
    Q_SIGNAL void sizeChanged();
    Q_SIGNAL void refreshMhzChanged();
    Q_SIGNAL void linksChanged();
    Q_SIGNAL void layoutChanged();
    Q_SIGNAL void playlistStatusChanged();
    Q_SIGNAL void lockWallpaperChanged();
    Q_SIGNAL void hasLockScreenChanged();
    Q_SIGNAL void lockLayoutChanged();

private:
    static auto linksFromPb(const proto::DisplayInfo& info) -> QVariantList;
    static auto effectiveLayoutFromPb(const proto::DisplayInfo& info) -> QVariantMap;
    static auto displayLayoutFromPb(const proto::DisplayInfo& info) -> QVariantMap;
    static auto layoutOverrideFromPb(const proto::DisplayInfo& info) -> QVariantMap;
    static auto lockDisplayLayoutFromPb(const proto::DisplayInfo& info) -> QVariantMap;
    static auto lockLayoutOverrideFromPb(const proto::DisplayInfo& info) -> QVariantMap;
    static auto playlistStatusFromPb(const proto::PlaylistDisplayStatus* status) -> QVariantMap;

    quint64      m_id;
    QString      m_name;
    QString      m_alias;
    quint32      m_width;
    quint32      m_height;
    quint32      m_refresh_mhz;
    QVariantList m_links;
    QVariantMap  m_effective_layout;
    QVariantMap  m_display_layout;
    QVariantMap  m_layout_override;
    quint32      m_drm_render_major;
    quint32      m_drm_render_minor;
    qint64       m_active_playlist_id { 0 };
    QVariantMap  m_playlist_status;
    QString      m_lock_wallpaper;
    bool         m_has_lock_screen { false };
    QVariantMap  m_lock_display_layout;
    QVariantMap  m_lock_layout_override;
};

/// Singleton model for all currently-registered displays. Fed by:
///   1. the snapshot that arrives on ws connect (via `Backend::eventReceived`),
///   2. subsequent `DisplayChanged` / `DisplayRemoved` events,
///   3. `DisplayListQuery::reload` as a fallback refresh path.
///
/// Consumers should prefer reading from `DisplayManager` over issuing
/// a fresh `DisplayListRequest` — the manager is push-updated.
class DisplayManager : public QObject {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantList displays READ displays NOTIFY displaysChanged FINAL)
    Q_PROPERTY(int count READ count NOTIFY displaysChanged FINAL)
    Q_PROPERTY(bool hasActivePlaylistDisplays READ hasActivePlaylistDisplays NOTIFY
                   playlistStatusChanged FINAL)

public:
    DisplayManager(QObject* parent = nullptr);
    ~DisplayManager() override;

    static auto instance() -> DisplayManager*;

    /// Snapshot of all displays (ordered by ascending id) as a list of
    /// `Display*`, suitable for QML `Repeater { model: DisplayManager.displays }`.
    auto displays() const -> QVariantList;
    auto count() const -> int { return (int)m_ordered.size(); }
    auto hasActivePlaylistDisplays() const -> bool;

    Q_INVOKABLE waywallen::Display* get(quint64 id) const;

    /// Full replace. Removes any id not present in `list`, upserts the rest.
    /// Exactly-once `displaysChanged` after the batch.
    void replaceAll(const QList<proto::DisplayInfo>& list);

    /// Upsert a single display; emits `displaysChanged` only if this
    /// was an add (removal/add changes the ordered list). Property
    /// changes on an existing display emit per-property signals.
    void upsert(const proto::DisplayInfo& info);

    /// Remove by id. Emits `displaysChanged` if the id existed.
    void remove(quint64 id);

    /// Full replacement of current playlist runtime state by display id.
    /// Missing displays are treated as inactive.
    void replacePlaylistStatuses(const QList<proto::PlaylistDisplayStatus>& list);

    /// Wire up to a backend's `eventReceived` signal. Call once from
    /// `App::init` after the backend is constructed.
    void attachTo(Backend* backend);

    Q_SIGNAL void displaysChanged();
    Q_SIGNAL void playlistStatusChanged();

private:
    void handleEvent(const proto::Event& evt);

    QList<Display*>             m_ordered; // sorted by id
    std::map<quint64, Display*> m_by_id;
};

} // namespace waywallen
