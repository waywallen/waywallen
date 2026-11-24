module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/model/user_property_model.moc"
#endif

export module waywallen:model.user_property;
import qextra;
import rstd.cppstd;
import :model.user_property_condition;

export namespace waywallen::model
{

// Wallpaper detail property list. Built-in rows are derived from
// daemon defaults and the source schema; plugin-published user
// properties are appended from the schema.
//
// `schemaJson` is the plugin-published map<string,WPProperty>;
// `overridesJson` is the DB column verbatim (object keyed by property
// name with raw wire-side string values).
class UserPropertyListModel : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT

    Q_PROPERTY(QString schemaJson READ schemaJson WRITE setSchemaJson NOTIFY schemaJsonChanged)
    Q_PROPERTY(QString wallpaperId READ wallpaperId WRITE setWallpaperId NOTIFY wallpaperIdChanged)
    Q_PROPERTY(
        QString overridesJson READ overridesJson WRITE setOverridesJson NOTIFY overridesJsonChanged)
    Q_PROPERTY(int count READ rowCount NOTIFY countChanged)
    Q_PROPERTY(bool hasPredefinedPropertyOverrides READ hasPredefinedPropertyOverrides NOTIFY
                   overrideStateChanged)
    Q_PROPERTY(
        bool hasUserPropertyOverrides READ hasUserPropertyOverrides NOTIFY overrideStateChanged)

public:
    enum Roles
    {
        KeyRole = Qt::UserRole + 1,
        LabelRole,
        TypeRole,
        SupportedRole,
        MinValRole,
        MaxValRole,
        StepValRole,
        ValueSuffixRole,
        CurrentValueRole,
        HasAlphaRole,
        OptionLabelsRole,
        OptionValuesRole,
        SectionRole,
        KindRole,
        IsGroupRole,
        GroupKeyRole,
        ExpandedRole,
        PrecisionRole,
    };
    Q_ENUM(Roles)

    explicit UserPropertyListModel(QObject* parent = nullptr);
    ~UserPropertyListModel() override;

    int                    rowCount(const QModelIndex& parent = {}) const override;
    QVariant               data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    auto          schemaJson() const -> const QString& { return m_schema_json; }
    void          setSchemaJson(const QString& v);
    Q_SIGNAL void schemaJsonChanged();

    auto          wallpaperId() const -> const QString& { return m_wallpaper_id; }
    void          setWallpaperId(const QString& v);
    Q_SIGNAL void wallpaperIdChanged();

    auto          overridesJson() const -> const QString& { return m_overrides_json; }
    void          setOverridesJson(const QString& v);
    Q_SIGNAL void overridesJsonChanged();

    Q_SIGNAL void countChanged();
    auto          hasPredefinedPropertyOverrides() const -> bool;
    auto          hasUserPropertyOverrides() const -> bool;
    Q_SIGNAL void overrideStateChanged();

    // Mutate the local value for a single key. Internal state +
    // `dataChanged` + `valueChanged` all fire synchronously. UI
    // controls bind to roles and react to `dataChanged`; the wire
    // push path (daemon RPC) is driven by `valueChanged` so it
    // can debounce without seeing the noise from external schema /
    // overrides rebuilds.
    Q_INVOKABLE void setValue(const QString& key, const QString& value);
    Q_INVOKABLE void setGroupExpanded(const QString& key, bool expanded);

    // Clear each matching override. One `resetRequested` per key, in order.
    Q_INVOKABLE void resetAll();
    Q_INVOKABLE void resetPredefinedProperties();
    Q_INVOKABLE void resetUserProperties();

    // Emitted exclusively from user intent, never from external
    // schema/overrides updates. Drives the QML-side debounced query
    // flush.
    Q_SIGNAL void valueChanged(const QString& key, const QString& value);
    Q_SIGNAL void resetRequested(const QString& key);

private:
    struct Entry {
        QString               key;
        QString               label;
        QVariant              localized_label;
        QString               type;
        QString               section;
        QString               kind;
        QString               group_key;
        UserPropertyCondition condition;
        QJsonValue            default_value;
        QJsonArray            options;
        int                   precision { 0 };
        bool                  supported { false };
        double                min_val { 0.0 };
        double                max_val { 1.0 };
        double                step_val { 0.0 };
        QString               value_suffix;
        QString               default_wire;
        QStringList           option_labels;
        QVariantList          localized_option_labels;
        QStringList           option_values;
        double                order { 0.0 };
    };

    void    rebuildEntries_();
    void    rebuildVisibleRows_();
    auto    visibleRows_() const -> QList<int>;
    void    updateVisibleRows_();
    auto    conditionValueFor_(const Entry& entry) const -> QJsonValue;
    void    resetKind_(const QString& kind);
    void    appendPredefinedEntries_(const QJsonObject& schema);
    QString currentValueFor_(qsizetype entry_index) const;
    void    notifyCurrentChanged_(const QString& key);
    auto    hasOverridesForKind_(const QString& kind) const -> bool;

    QString                 m_schema_json;
    QString                 m_wallpaper_id;
    QString                 m_overrides_json;
    QHash<QString, QString> m_overrides; // parsed view of m_overrides_json
    QList<Entry>            m_entries;
    // Hidden entries retain their values and participate in resets.
    QList<int>    m_visible_rows;
    QSet<QString> m_collapsed_groups;
};

} // namespace waywallen::model
