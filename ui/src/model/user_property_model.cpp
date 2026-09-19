module;
#include "waywallen/model/user_property_model.moc.h"

module waywallen;
import qextra;
import :model.user_property;
import :plugin_translation;

namespace waywallen::model
{

namespace
{

bool isSupported(const QString& type, bool has_options) {
    return type == QLatin1String("color") || type == QLatin1String("slider") ||
           type == QLatin1String("bool") || type == QLatin1String("textinput") ||
           type == QLatin1String("text") || (type == QLatin1String("combo") && has_options);
}

QString builtinKind() { return QStringLiteral("property"); }
QString userKind() { return QStringLiteral("user"); }
QString schemeColorKey() { return QStringLiteral("waywallen.scheme_color"); }
QString enableAudioKey() { return QStringLiteral("waywallen.enable_audio"); }
QString playbackSpeedKey() { return QStringLiteral("waywallen.playback_speed"); }
QString mouseParallaxKey() { return QStringLiteral("waywallen.mouse_parallax"); }

bool isPredefinedKey(const QString& key) {
    return key == schemeColorKey() || key == enableAudioKey() || key == playbackSpeedKey() ||
           key == mouseParallaxKey();
}

QString jsonValueToWireString(const QJsonValue& v) {
    switch (v.type()) {
    case QJsonValue::Bool: return v.toBool() ? QStringLiteral("true") : QStringLiteral("false");
    case QJsonValue::Double: return QString::number(v.toDouble());
    case QJsonValue::String: return v.toString();
    case QJsonValue::Array: {
        QStringList parts;
        const auto  a = v.toArray();
        parts.reserve(a.size());
        for (const auto& e : a) parts << QString::number(e.toDouble(), 'f', 4);
        return parts.join(QLatin1Char(' '));
    }
    default: return {};
    }
}

QString coerceDefaultWireString(const QJsonValue& def, const QString& type) {
    if (def.isUndefined() || def.isNull()) return {};
    // For colors WE may emit the default either as `"r g b"` string or as
    // a JSON array; normalise to space-separated floats either way.
    if (type == QLatin1String("color")) {
        if (def.isArray()) {
            QStringList parts;
            const auto  a = def.toArray();
            parts.reserve(a.size());
            for (const auto& e : a) parts << QString::number(e.toDouble(), 'f', 4);
            return parts.join(QLatin1Char(' '));
        }
        if (def.isString()) return def.toString();
    }
    if (type == QLatin1String("bool"))
        return def.toBool() ? QStringLiteral("true") : QStringLiteral("false");
    if (type == QLatin1String("slider")) return QString::number(def.toDouble());
    if (type == QLatin1String("combo")) return jsonValueToWireString(def);
    return jsonValueToWireString(def);
}

} // namespace

UserPropertyListModel::UserPropertyListModel(QObject* parent): QAbstractListModel(parent) {
    connect(
        PluginTranslationStore::instance(), &PluginTranslationStore::revisionChanged, this, [this] {
            if (m_entries.isEmpty()) return;
            Q_EMIT dataChanged(index(0),
                               index(static_cast<int>(m_entries.size()) - 1),
                               { LabelRole, OptionLabelsRole });
        });
}

UserPropertyListModel::~UserPropertyListModel() = default;

int UserPropertyListModel::rowCount(const QModelIndex& parent) const {
    if (parent.isValid()) return 0;
    return static_cast<int>(m_entries.size());
}

QHash<int, QByteArray> UserPropertyListModel::roleNames() const {
    return {
        { KeyRole, "key" },
        { LabelRole, "label" },
        { TypeRole, "type" },
        { SupportedRole, "supported" },
        { MinValRole, "minVal" },
        { MaxValRole, "maxVal" },
        { StepValRole, "stepVal" },
        { ValueSuffixRole, "valueSuffix" },
        { CurrentValueRole, "currentValue" },
        { HasAlphaRole, "hasAlpha" },
        { OptionLabelsRole, "optionLabels" },
        { OptionValuesRole, "optionValues" },
        { SectionRole, "section" },
        { KindRole, "kind" },
    };
}

QVariant UserPropertyListModel::data(const QModelIndex& index, int role) const {
    if (! index.isValid()) return {};
    const auto row = index.row();
    if (row < 0 || row >= m_entries.size()) return {};
    const auto& e = m_entries.at(row);
    switch (role) {
    case KeyRole: return e.key;
    case LabelRole: {
        const auto translated = PluginTranslationStore::instance()->translate(e.localized_label);
        return translated.isEmpty() ? e.label : translated;
    }
    case TypeRole: return e.type;
    case SupportedRole: return e.supported;
    case MinValRole: return e.min_val;
    case MaxValRole: return e.max_val;
    case StepValRole: return e.step_val;
    case ValueSuffixRole: return e.value_suffix;
    case CurrentValueRole: return currentValueFor_(row);
    case OptionLabelsRole: {
        auto labels = e.option_labels;
        for (qsizetype i = 0; i < labels.size() && i < e.localized_option_labels.size(); ++i) {
            const auto translated =
                PluginTranslationStore::instance()->translate(e.localized_option_labels.at(i));
            if (! translated.isEmpty()) labels[i] = translated;
        }
        return labels;
    }
    case OptionValuesRole: return e.option_values;
    case HasAlphaRole: {
        const QString                   cv = currentValueFor_(row);
        static const QRegularExpression reSpaces(QStringLiteral("\\s+"));
        return cv.trimmed().split(reSpaces, Qt::SkipEmptyParts).size() >= 4;
    }
    case SectionRole: return e.section;
    case KindRole: return e.kind;
    default: return {};
    }
}

QString UserPropertyListModel::currentValueFor_(qsizetype row) const {
    const auto& e  = m_entries.at(row);
    const auto  it = m_overrides.constFind(e.key);
    if (it != m_overrides.constEnd()) return it.value();
    return e.default_wire;
}

auto UserPropertyListModel::hasPredefinedPropertyOverrides() const -> bool {
    return hasOverridesForKind_(builtinKind());
}

auto UserPropertyListModel::hasUserPropertyOverrides() const -> bool {
    return hasOverridesForKind_(userKind());
}

auto UserPropertyListModel::hasOverridesForKind_(const QString& kind) const -> bool {
    for (const auto& e : m_entries) {
        if (e.kind == kind && m_overrides.contains(e.key)) return true;
    }
    return false;
}

void UserPropertyListModel::setSchemaJson(const QString& v) {
    if (v == m_schema_json) return;
    m_schema_json = v;
    Q_EMIT schemaJsonChanged();
    rebuildEntries_();
}

void UserPropertyListModel::setOverridesJson(const QString& v) {
    if (v == m_overrides_json) return;
    m_overrides_json = v;
    Q_EMIT overridesJsonChanged();

    m_overrides.clear();
    if (! m_overrides_json.isEmpty()) {
        QJsonParseError err {};
        const auto      doc = QJsonDocument::fromJson(m_overrides_json.toUtf8(), &err);
        if (err.error == QJsonParseError::NoError && doc.isObject()) {
            const auto obj = doc.object();
            for (auto it = obj.constBegin(); it != obj.constEnd(); ++it) {
                if (it.value().isString()) m_overrides.insert(it.key(), it.value().toString());
            }
        }
    }
    // Every row's CurrentValue derivation depends on m_overrides.
    if (! m_entries.isEmpty()) {
        Q_EMIT dataChanged(index(0),
                           index(static_cast<int>(m_entries.size()) - 1),
                           { CurrentValueRole, HasAlphaRole });
    }
    Q_EMIT overrideStateChanged();
}

void UserPropertyListModel::rebuildEntries_() {
    beginResetModel();
    m_entries.clear();
    QJsonObject schema_obj;
    if (! m_schema_json.isEmpty()) {
        QJsonParseError err {};
        const auto      doc = QJsonDocument::fromJson(m_schema_json.toUtf8(), &err);
        if (err.error == QJsonParseError::NoError && doc.isObject()) {
            schema_obj = doc.object();
        }
    }
    appendPredefinedEntries_(schema_obj);
    if (! schema_obj.isEmpty()) {
        QList<Entry> user_entries;
        user_entries.reserve(schema_obj.size());
        for (auto it = schema_obj.constBegin(); it != schema_obj.constEnd(); ++it) {
            if (isPredefinedKey(it.key())) continue;
            const auto v = it.value().toObject();
            Entry      e;
            e.key             = it.key();
            e.label           = v.value(QStringLiteral("text")).toString();
            e.localized_label = v.value(QStringLiteral("localized_text")).toObject().toVariantMap();
            e.section         = userKind();
            e.kind            = userKind();
            if (e.label.isEmpty()) e.label = e.key;
            e.type = v.value(QStringLiteral("type")).toString().toLower();
            if (v.value(QStringLiteral("options")).isArray()) {
                const auto opts = v.value(QStringLiteral("options")).toArray();
                e.option_labels.reserve(opts.size());
                e.option_values.reserve(opts.size());
                for (const auto& opt_value : opts) {
                    const auto opt   = opt_value.toObject();
                    QString    value = jsonValueToWireString(opt.value(QStringLiteral("value")));
                    QString    label = opt.value(QStringLiteral("label")).toString();
                    const auto localized_label =
                        opt.value(QStringLiteral("localized_label")).toObject().toVariantMap();
                    if (label.isEmpty()) label = value;
                    e.option_values.append(std::move(value));
                    e.option_labels.append(std::move(label));
                    e.localized_option_labels.append(localized_label);
                }
            }
            e.supported    = isSupported(e.type, ! e.option_values.isEmpty());
            e.min_val      = v.value(QStringLiteral("min")).toDouble(0.0);
            e.max_val      = v.value(QStringLiteral("max")).toDouble(1.0);
            e.step_val     = v.value(QStringLiteral("step")).toDouble(0.0);
            e.value_suffix = v.value(QStringLiteral("suffix")).toString();
            e.default_wire = coerceDefaultWireString(v.value(QStringLiteral("value")), e.type);
            e.order        = v.value(QStringLiteral("order")).toDouble(0.0);
            user_entries.append(std::move(e));
        }
        std::sort(user_entries.begin(), user_entries.end(), [](const Entry& a, const Entry& b) {
            return a.order < b.order;
        });
        m_entries.append(user_entries);
    }
    endResetModel();
    Q_EMIT countChanged();
    Q_EMIT overrideStateChanged();
}

void UserPropertyListModel::appendPredefinedEntries_(const QJsonObject& schema) {
    auto make = [](QString            key,
                   const QJsonObject& value,
                   QString            label,
                   QString            type,
                   QString            default_wire) {
        Entry e;
        e.key             = std::move(key);
        e.label           = value.value(QStringLiteral("text")).toString();
        e.localized_label = value.value(QStringLiteral("localized_text")).toObject().toVariantMap();
        e.type            = value.value(QStringLiteral("type")).toString().toLower();
        e.section         = builtinKind();
        e.kind            = builtinKind();
        if (e.label.isEmpty()) e.label = std::move(label);
        if (e.type.isEmpty()) e.type = std::move(type);
        e.supported    = isSupported(e.type, false);
        e.min_val      = value.value(QStringLiteral("min")).toDouble(0.0);
        e.max_val      = value.value(QStringLiteral("max")).toDouble(1.0);
        e.step_val     = value.value(QStringLiteral("step")).toDouble(0.0);
        e.value_suffix = value.value(QStringLiteral("suffix")).toString();
        e.default_wire = coerceDefaultWireString(value.value(QStringLiteral("value")), e.type);
        if (e.default_wire.isEmpty()) e.default_wire = std::move(default_wire);
        return e;
    };

    const auto scheme_schema = schema.value(schemeColorKey()).toObject();
    auto       scheme        = make(schemeColorKey(),
                                    scheme_schema,
                                    UserPropertyListModel::tr("Scheme color"),
                                    QStringLiteral("color"),
                                    QStringLiteral("0.0000 0.0000 0.0000 1.0000"));
    m_entries.append(std::move(scheme));

    if (schema.contains(enableAudioKey())) {
        const auto audio_schema = schema.value(enableAudioKey()).toObject();
        auto       audio        = make(enableAudioKey(),
                                       audio_schema,
                                       UserPropertyListModel::tr("Enable audio"),
                                       QStringLiteral("bool"),
                                       QStringLiteral("true"));
        m_entries.append(std::move(audio));
    }

    if (schema.contains(playbackSpeedKey())) {
        const auto speed_schema = schema.value(playbackSpeedKey()).toObject();
        auto       speed        = make(playbackSpeedKey(),
                                       speed_schema,
                                       UserPropertyListModel::tr("Playback speed"),
                                       QStringLiteral("slider"),
                                       QStringLiteral("100"));
        m_entries.append(std::move(speed));
    }

    if (schema.contains(mouseParallaxKey())) {
        const auto parallax_schema = schema.value(mouseParallaxKey()).toObject();
        auto       parallax        = make(mouseParallaxKey(),
                                          parallax_schema,
                                          UserPropertyListModel::tr("Mouse parallax"),
                                          QStringLiteral("bool"),
                                          QStringLiteral("true"));
        m_entries.append(std::move(parallax));
    }
}

void UserPropertyListModel::setValue(const QString& key, const QString& value) {
    m_overrides.insert(key, value);
    notifyCurrentChanged_(key);
    Q_EMIT overrideStateChanged();
    Q_EMIT valueChanged(key, value);
}

void UserPropertyListModel::resetAll() {
    for (const auto& e : m_entries) {
        if (! m_overrides.contains(e.key)) continue;
        m_overrides.remove(e.key);
        notifyCurrentChanged_(e.key);
        Q_EMIT resetRequested(e.key);
    }
    Q_EMIT overrideStateChanged();
}

void UserPropertyListModel::resetPredefinedProperties() {
    for (const auto& e : m_entries) {
        if (e.kind != builtinKind()) continue;
        if (! m_overrides.contains(e.key)) continue;
        m_overrides.remove(e.key);
        notifyCurrentChanged_(e.key);
        Q_EMIT resetRequested(e.key);
    }
    Q_EMIT overrideStateChanged();
}

void UserPropertyListModel::resetUserProperties() {
    for (const auto& e : m_entries) {
        if (e.kind != userKind()) continue;
        if (! m_overrides.contains(e.key)) continue;
        m_overrides.remove(e.key);
        notifyCurrentChanged_(e.key);
        Q_EMIT resetRequested(e.key);
    }
    Q_EMIT overrideStateChanged();
}

void UserPropertyListModel::notifyCurrentChanged_(const QString& key) {
    for (qsizetype i = 0; i < m_entries.size(); ++i) {
        if (m_entries.at(i).key == key) {
            const auto idx = index(static_cast<int>(i));
            Q_EMIT dataChanged(idx, idx, { CurrentValueRole, HasAlphaRole });
            return;
        }
    }
}

} // namespace waywallen::model

#include "waywallen/model/user_property_model.moc.cpp"
