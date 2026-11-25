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

bool isValueProperty(const QString& type) {
    return type != QLatin1String("group") && type != QLatin1String("text");
}

struct SliderPresentation {
    double step;
    int    precision;
};

SliderPresentation sliderPresentation(const QJsonObject& schema, double maximum) {
    const auto fraction  = schema.value(QStringLiteral("fraction"));
    const bool integer   = fraction.isBool() && ! fraction.toBool();
    double     step      = schema.value(QStringLiteral("step")).toDouble();
    int        precision = fraction.toBool() || maximum <= 10 ? 3 : 0;
    if (integer) {
        precision = 0;
    } else if (schema.value(QStringLiteral("precision")).isDouble()) {
        precision = qBound(0, schema.value(QStringLiteral("precision")).toInt(), 15);
    } else if (step > 0) {
        auto decimal = QString::number(step, 'f', 15);
        while (decimal.endsWith(u'0')) decimal.chop(1);
        const auto point = decimal.indexOf(u'.');
        precision        = point < 0 ? 0 : static_cast<int>(decimal.size() - point - 1);
        if (step < 1 && precision == 0) precision = 15;
    }
    if (step <= 0) step = integer || (precision == 0 && ! fraction.toBool()) ? 1.0 : 0.0;
    return { step, precision };
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
    case QJsonValue::Double: return QString::number(v.toDouble(), 'g', 17);
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
    if (type == QLatin1String("slider")) return QString::number(def.toDouble(), 'g', 17);
    if (type == QLatin1String("combo")) return jsonValueToWireString(def);
    return jsonValueToWireString(def);
}

} // namespace

UserPropertyListModel::UserPropertyListModel(QObject* parent): QAbstractListModel(parent) {
    connect(
        PluginTranslationStore::instance(), &PluginTranslationStore::revisionChanged, this, [this] {
            if (m_visible_rows.isEmpty()) return;
            Q_EMIT dataChanged(index(0), index(rowCount() - 1), { LabelRole, OptionLabelsRole });
        });
}

UserPropertyListModel::~UserPropertyListModel() = default;

int UserPropertyListModel::rowCount(const QModelIndex& parent) const {
    if (parent.isValid()) return 0;
    return static_cast<int>(m_visible_rows.size());
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
        { IsGroupRole, "isGroup" },
        { GroupKeyRole, "groupKey" },
        { ExpandedRole, "expanded" },
        { PrecisionRole, "precision" },
    };
}

QVariant UserPropertyListModel::data(const QModelIndex& index, int role) const {
    if (! index.isValid()) return {};
    const auto row = index.row();
    if (row < 0 || row >= m_visible_rows.size()) return {};
    const auto  entry_index = m_visible_rows.at(row);
    const auto& e           = m_entries.at(entry_index);
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
    case PrecisionRole: return e.precision;
    case ValueSuffixRole: return e.value_suffix;
    case CurrentValueRole: return currentValueFor_(entry_index);
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
        const QString                   cv = currentValueFor_(entry_index);
        static const QRegularExpression reSpaces(QStringLiteral("\\s+"));
        return cv.trimmed().split(reSpaces, Qt::SkipEmptyParts).size() >= 4;
    }
    case SectionRole: return e.section;
    case KindRole: return e.kind;
    case IsGroupRole: return e.type == QLatin1String("group");
    case GroupKeyRole: return e.group_key;
    case ExpandedRole:
        return e.type == QLatin1String("group") && ! m_collapsed_groups.contains(e.key);
    default: return {};
    }
}

QString UserPropertyListModel::currentValueFor_(qsizetype entry_index) const {
    const auto& e = m_entries.at(entry_index);
    if (e.type == QLatin1String("group")) return {};
    const auto it = m_overrides.constFind(e.key);
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
        if (isValueProperty(e.type) && e.kind == kind && m_overrides.contains(e.key)) return true;
    }
    return false;
}

void UserPropertyListModel::setSchemaJson(const QString& v) {
    if (v == m_schema_json) return;
    m_schema_json = v;
    Q_EMIT schemaJsonChanged();
    rebuildEntries_();
}

void UserPropertyListModel::setWallpaperId(const QString& v) {
    if (v == m_wallpaper_id) return;
    m_wallpaper_id = v;
    if (! m_collapsed_groups.isEmpty()) {
        beginResetModel();
        m_collapsed_groups.clear();
        rebuildVisibleRows_();
        endResetModel();
        Q_EMIT countChanged();
    }
    Q_EMIT wallpaperIdChanged();
}

void UserPropertyListModel::setOverridesJson(const QString& v) {
    const bool json_changed = v != m_overrides_json;
    m_overrides_json        = v;

    // The same server snapshot can still replace optimistic edits.
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
    updateVisibleRows_();
    // Every row's CurrentValue derivation depends on m_overrides.
    if (! m_visible_rows.isEmpty()) {
        Q_EMIT dataChanged(index(0), index(rowCount() - 1), { CurrentValueRole, HasAlphaRole });
    }
    Q_EMIT overrideStateChanged();
    if (json_changed) Q_EMIT overridesJsonChanged();
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
            if (e.type.isEmpty()) e.type = QStringLiteral("text");
            e.condition = UserPropertyCondition(v.value(QStringLiteral("condition")).toString());
            if (! e.condition.isValid())
                qWarning().noquote() << "unsupported user property condition:" << e.key
                                     << "wallpaper:" << m_wallpaper_id;
            if (v.value(QStringLiteral("options")).isArray()) {
                const auto opts = v.value(QStringLiteral("options")).toArray();
                e.options       = opts;
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
            e.supported = e.type == QLatin1String("group") ||
                          isSupported(e.type, ! e.option_values.isEmpty());
            e.min_val   = v.value(QStringLiteral("min")).toDouble(0.0);
            e.max_val   = v.value(QStringLiteral("max")).toDouble(1.0);
            e.step_val  = v.value(QStringLiteral("step")).toDouble(0.0);
            if (e.type == QLatin1String("slider")) {
                const auto presentation = sliderPresentation(v, e.max_val);
                e.step_val              = presentation.step;
                e.precision             = presentation.precision;
            }
            e.value_suffix  = v.value(QStringLiteral("suffix")).toString();
            e.default_wire  = coerceDefaultWireString(v.value(QStringLiteral("value")), e.type);
            e.default_value = v.value(QStringLiteral("value"));
            e.order         = v.value(QStringLiteral("order"))
                                  .toDouble(v.value(QStringLiteral("index")).toDouble(0.0));
            user_entries.append(std::move(e));
        }
        std::stable_sort(
            user_entries.begin(), user_entries.end(), [](const Entry& a, const Entry& b) {
                return a.order < b.order;
            });
        QString       group_key;
        QSet<QString> groups;
        for (auto& entry : user_entries) {
            if (entry.type == QLatin1String("group")) {
                group_key = entry.key;
                groups.insert(group_key);
            }
            entry.group_key = group_key;
        }
        m_collapsed_groups.intersect(groups);
        m_entries.append(user_entries);
    }
    rebuildVisibleRows_();
    endResetModel();
    Q_EMIT countChanged();
    Q_EMIT overrideStateChanged();
}

void UserPropertyListModel::rebuildVisibleRows_() { m_visible_rows = visibleRows_(); }

auto UserPropertyListModel::conditionValueFor_(const Entry& entry) const -> QJsonValue {
    const auto override = m_overrides.constFind(entry.key);
    if (override == m_overrides.cend()) return entry.default_value;
    const auto& wire = override.value();
    if (entry.type == QLatin1String("combo")) {
        for (const auto& option : entry.options) {
            const auto value = option.toObject().value(QStringLiteral("value"));
            if (jsonValueToWireString(value) == wire) return value;
        }
    }
    if (entry.type == QLatin1String("bool") || entry.default_value.isBool()) {
        if (wire == QLatin1String("true")) return true;
        if (wire == QLatin1String("false")) return false;
        return QJsonValue::Undefined;
    }
    if (entry.type == QLatin1String("slider") || entry.default_value.isDouble()) {
        bool       valid  = false;
        const auto number = wire.toDouble(&valid);
        return valid && qIsFinite(number) ? QJsonValue(number) : QJsonValue::Undefined;
    }
    return wire;
}

auto UserPropertyListModel::visibleRows_() const -> QList<int> {
    QHash<QString, QJsonValue> values;
    for (const auto& entry : m_entries) values.insert(entry.key, conditionValueFor_(entry));
    QList<bool> included;
    included.reserve(m_entries.size());
    for (const auto& entry : m_entries) included.append(entry.condition.matches(values));
    QSet<QString> visible_groups;
    for (int i = 0; i < m_entries.size(); ++i) {
        const auto& entry = m_entries.at(i);
        if (entry.type != QLatin1String("group") || ! included.at(i)) continue;
        for (int child = i + 1; child < m_entries.size(); ++child) {
            if (m_entries.at(child).type == QLatin1String("group")) break;
            if (included.at(child)) {
                visible_groups.insert(entry.key);
                break;
            }
        }
    }
    QList<int> rows;
    for (int i = 0; i < m_entries.size(); ++i) {
        const auto& entry = m_entries.at(i);
        if (! included.at(i)) continue;
        if (entry.type == QLatin1String("group")) {
            if (! visible_groups.contains(entry.key)) continue;
        } else if (! entry.group_key.isEmpty() && (! visible_groups.contains(entry.group_key) ||
                                                   m_collapsed_groups.contains(entry.group_key))) {
            continue;
        }
        rows.append(i);
    }
    return rows;
}

void UserPropertyListModel::updateVisibleRows_() {
    const auto      next = visibleRows_();
    const QSet<int> keep(next.cbegin(), next.cend());
    const auto      previous_count = rowCount();
    for (int last = rowCount() - 1; last >= 0;) {
        if (keep.contains(m_visible_rows.at(last))) {
            --last;
            continue;
        }
        int first = last;
        while (first > 0 && ! keep.contains(m_visible_rows.at(first - 1))) --first;
        beginRemoveRows({}, first, last);
        m_visible_rows.remove(first, last - first + 1);
        endRemoveRows();
        last = first - 1;
    }
    for (int row = 0; row < next.size();) {
        if (row < m_visible_rows.size() && m_visible_rows.at(row) == next.at(row)) {
            ++row;
            continue;
        }
        const auto first = row;
        while (row < next.size() &&
               (first >= m_visible_rows.size() || next.at(row) != m_visible_rows.at(first)))
            ++row;
        beginInsertRows({}, first, row - 1);
        for (int i = first; i < row; ++i) m_visible_rows.insert(i, next.at(i));
        endInsertRows();
    }
    if (previous_count != rowCount()) Q_EMIT countChanged();
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
        e.supported = isSupported(e.type, false);
        e.min_val   = value.value(QStringLiteral("min")).toDouble(0.0);
        e.max_val   = value.value(QStringLiteral("max")).toDouble(1.0);
        e.step_val  = value.value(QStringLiteral("step")).toDouble(0.0);
        if (e.type == QLatin1String("slider")) {
            const auto presentation = sliderPresentation(value, e.max_val);
            e.step_val              = presentation.step;
            e.precision             = presentation.precision;
        }
        e.value_suffix = value.value(QStringLiteral("suffix")).toString();
        e.default_wire = coerceDefaultWireString(value.value(QStringLiteral("value")), e.type);
        if (e.default_wire.isEmpty()) e.default_wire = std::move(default_wire);
        e.default_value = value.value(QStringLiteral("value"));
        if (e.default_value.isUndefined()) {
            if (e.type == QLatin1String("bool"))
                e.default_value = e.default_wire == QLatin1String("true");
            else if (e.type == QLatin1String("slider"))
                e.default_value = e.default_wire.toDouble();
            else
                e.default_value = e.default_wire;
        }
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
    const auto entry = std::find_if(m_entries.cbegin(), m_entries.cend(), [&key](const Entry& e) {
        return e.key == key && isValueProperty(e.type);
    });
    if (entry == m_entries.cend()) return;
    m_overrides.insert(key, value);
    updateVisibleRows_();
    notifyCurrentChanged_(key);
    Q_EMIT overrideStateChanged();
    Q_EMIT valueChanged(key, value);
}

void UserPropertyListModel::setGroupExpanded(const QString& key, bool expanded) {
    if (expanded == ! m_collapsed_groups.contains(key)) return;
    const auto entry = std::find_if(m_entries.cbegin(), m_entries.cend(), [&key](const Entry& e) {
        return e.key == key && e.type == QLatin1String("group");
    });
    if (entry == m_entries.cend()) return;
    const auto entry_index = static_cast<int>(entry - m_entries.cbegin());
    if (expanded)
        m_collapsed_groups.remove(key);
    else
        m_collapsed_groups.insert(key);
    updateVisibleRows_();
    const auto row = m_visible_rows.indexOf(entry_index);
    if (row >= 0) {
        const auto header = index(static_cast<int>(row));
        Q_EMIT dataChanged(header, header, { ExpandedRole });
    }
}

void UserPropertyListModel::resetAll() { resetKind_({}); }

void UserPropertyListModel::resetPredefinedProperties() { resetKind_(builtinKind()); }

void UserPropertyListModel::resetUserProperties() { resetKind_(userKind()); }

void UserPropertyListModel::resetKind_(const QString& kind) {
    QStringList changed;
    for (const auto& e : m_entries) {
        if (! isValueProperty(e.type) || (! kind.isEmpty() && e.kind != kind)) continue;
        if (m_overrides.remove(e.key)) changed.append(e.key);
    }
    updateVisibleRows_();
    for (const auto& key : changed) {
        notifyCurrentChanged_(key);
        Q_EMIT resetRequested(key);
    }
    Q_EMIT overrideStateChanged();
}

void UserPropertyListModel::notifyCurrentChanged_(const QString& key) {
    for (qsizetype i = 0; i < m_visible_rows.size(); ++i) {
        if (m_entries.at(m_visible_rows.at(i)).key == key) {
            const auto idx = index(static_cast<int>(i));
            Q_EMIT dataChanged(idx, idx, { CurrentValueRole, HasAlphaRole });
            return;
        }
    }
}

} // namespace waywallen::model

#include "waywallen/model/user_property_model.moc.cpp"
