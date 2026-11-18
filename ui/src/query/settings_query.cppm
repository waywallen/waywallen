module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/query/settings_query.moc"
#endif

export module waywallen:query.settings;
export import :query.query;

namespace waywallen
{

/// Fetch the daemon's persisted settings. `global` is a flat
/// QVariantMap (`layoutDefaults`, `wallpaperFilters`, …).
/// `plugins` is keyed by runtime component name with each value a
/// `{key: stringValue}` QVariantMap. Plugin values are wire-string
/// typed — the QML form coerces per the matching `SettingSchema.type`.
export class SettingsGetQuery : public Query,
                                public QueryExtra<control::v1::Response, SettingsGetQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantMap global READ global NOTIFY globalChanged FINAL)
    Q_PROPERTY(QVariantMap plugins READ plugins NOTIFY pluginsChanged FINAL)
    Q_PROPERTY(QString logDir READ logDir NOTIFY logDirChanged FINAL)
    Q_PROPERTY(bool wwLogActive READ wwLogActive NOTIFY wwLogActiveChanged FINAL)

public:
    SettingsGetQuery(QObject* parent = nullptr);

    auto global() const -> const QVariantMap&;
    auto plugins() const -> const QVariantMap&;
    auto logDir() const -> const QString&;
    auto wwLogActive() const -> bool;

    void reload() override;

    Q_SIGNAL void globalChanged();
    Q_SIGNAL void pluginsChanged();
    Q_SIGNAL void logDirChanged();
    Q_SIGNAL void wwLogActiveChanged();

private:
    QVariantMap m_global;
    QVariantMap m_plugins;
    QString     m_log_dir;
    bool        m_ww_log_active = false;
};

/// Apply a full-replace settings write. Caller must populate both
/// `global` (QVariantMap; today only `layoutDefaults` is meaningful) and `plugins`
/// (`{plugin: {key: stringValue}}`). The
/// daemon validates against the manifest schema (range, enum) and
/// returns INVALID_ARGUMENT on rejection — surfaced via the standard
/// Query `error` property.
export class SettingsSetQuery : public Query,
                                public QueryExtra<control::v1::Response, SettingsSetQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QVariantMap global READ global WRITE setGlobal NOTIFY globalChanged FINAL)
    Q_PROPERTY(QVariantMap plugins READ plugins WRITE setPlugins NOTIFY pluginsChanged FINAL)

public:
    SettingsSetQuery(QObject* parent = nullptr);

    auto global() const -> const QVariantMap&;
    void setGlobal(const QVariantMap& v);

    auto plugins() const -> const QVariantMap&;
    void setPlugins(const QVariantMap& v);

    void reload() override;

    Q_SIGNAL void globalChanged();
    Q_SIGNAL void pluginsChanged();

private:
    QVariantMap m_global;
    QVariantMap m_plugins;
};

export class SystemWallpaperSnapshotsQuery : public Query,
    public QueryExtra<control::v1::Response, SystemWallpaperSnapshotsQuery> {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(bool clear READ clear WRITE setClear NOTIFY clearChanged FINAL)
    Q_PROPERTY(bool retake READ retake WRITE setRetake NOTIFY retakeChanged FINAL)

public:
    SystemWallpaperSnapshotsQuery(QObject* parent = nullptr);
    auto clear() const -> bool;
    void setClear(bool value);
    auto retake() const -> bool;
    void setRetake(bool value);
    void reload() override;

    Q_SIGNAL void clearChanged();
    Q_SIGNAL void retakeChanged();

private:
    bool m_clear = false;
    bool m_retake = false;
};

} // namespace waywallen
