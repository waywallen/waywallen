module;
#include "waywallen/model/filter_rule_model.moc.h"
module waywallen;
import :model.filter_rule;
import qextra;

namespace waywallen
{

FilterRuleModel::FilterRuleModel(kstore::QListInterface* list, QObject* parent)
    : kstore::QGadgetListModel(list, parent) {
    connect(this, &QAbstractItemModel::dataChanged, this, &FilterRuleModel::markDirty);
    connect(this, &QAbstractItemModel::rowsInserted, this, &FilterRuleModel::markDirty);
    connect(this, &QAbstractItemModel::rowsRemoved, this, &FilterRuleModel::markDirty);
    connect(this, &FilterRuleModel::filterLogicsChanged, this, &FilterRuleModel::markDirty);

    connect(this, &FilterRuleModel::apply, this, [this]() {
        setDirty(false);
    });
    connect(this, &FilterRuleModel::reset, this, [this]() {
        setDirty(false);
    });
}

FilterRuleModel::~FilterRuleModel() = default;

void FilterRuleModel::setFilterLogics(const QList<control::v1::FilterLogic>& v) {
    m_filter_logics = v;
    Q_EMIT filterLogicsChanged();
}

int FilterRuleModel::roleForName_(const QByteArray& name) const {
    const auto names = roleNames();
    for (auto it = names.cbegin(); it != names.cend(); ++it) {
        if (it.value() == name) return it.key();
    }
    return -1;
}

int FilterRuleModel::groupOf_(const QVariant& v) const {
    const auto metaType   = v.metaType();
    const auto metaObject = metaType.metaObject();
    if (! metaObject) return 0;
    const int index = metaObject->indexOfProperty("group");
    if (index < 0) return 0;
    return metaObject->property(index).readOnGadget(v.constData()).toInt();
}

auto FilterRuleModel::orderedGroups_() const -> QList<int> {
    QList<int> groups;
    const int  role = roleForName_("group");
    if (role < 0) return groups;
    for (int i = 0; i < rowCount(); ++i) {
        const int group = data(index(i, 0), role).toInt();
        if (groups.isEmpty() || groups.back() != group) groups.append(group);
    }
    return groups;
}

int FilterRuleModel::newGroupId() const {
    const int role = roleForName_("group");
    if (role < 0) return 0;
    int max = -1;
    for (int i = 0; i < rowCount(); ++i) {
        max = std::max(max, data(index(i, 0), role).toInt());
    }
    return max + 1;
}

int FilterRuleModel::rowIndexInGroup(int row) const {
    const int role = roleForName_("group");
    if (role < 0 || row < 0 || row >= rowCount()) return 0;
    const int group = data(index(row, 0), role).toInt();
    int       count = 0;
    for (int i = 0; i < row; ++i) {
        if (data(index(i, 0), role).toInt() == group) ++count;
    }
    return count;
}

int FilterRuleModel::rowCountInGroupOf(int row) const {
    const int role = roleForName_("group");
    if (role < 0 || row < 0 || row >= rowCount()) return 0;
    return countInGroup(data(index(row, 0), role).toInt());
}

int FilterRuleModel::countInGroup(int group) const {
    const int role = roleForName_("group");
    if (role < 0) return 0;
    int count = 0;
    for (int i = 0; i < rowCount(); ++i) {
        if (data(index(i, 0), role).toInt() == group) ++count;
    }
    return count;
}

int FilterRuleModel::findInsertPosition(int group) const {
    const int role = roleForName_("group");
    if (role < 0) return rowCount();
    int  pos  = 0;
    bool seen = false;
    for (int i = 0; i < rowCount(); ++i) {
        const int current = data(index(i, 0), role).toInt();
        if (current <= group) {
            pos  = i + 1;
            seen = true;
        } else if (seen) {
            break;
        }
    }
    return pos;
}

int FilterRuleModel::sectionIndexForGroup(int group) const {
    return orderedGroups_().indexOf(group);
}

int FilterRuleModel::findLogicAt(int sectionIndex) const {
    if (sectionIndex <= 0) return -1;
    const auto groups = orderedGroups_();
    if (sectionIndex >= groups.size()) return -1;
    const int previous = groups[sectionIndex - 1];
    const int current  = groups[sectionIndex];
    for (int i = 0; i < m_filter_logics.size(); ++i) {
        const auto& logic = m_filter_logics[i];
        if (logic.groupA() == previous && logic.groupB() == current) return i;
    }
    return -1;
}

int FilterRuleModel::logicOpAt(int sectionIndex) const {
    const int idx = findLogicAt(sectionIndex);
    if (idx < 0) return -1;
    return static_cast<int>(m_filter_logics[idx].op());
}

void FilterRuleModel::setLogicOpAt(int sectionIndex, int op) {
    int idx = findLogicAt(sectionIndex);
    if (idx < 0) {
        const auto groups = orderedGroups_();
        if (sectionIndex <= 0 || sectionIndex >= groups.size()) return;
        control::v1::FilterLogic logic;
        logic.setOp(static_cast<control::v1::LogicOp>(op));
        logic.setGroupA(groups[sectionIndex - 1]);
        logic.setGroupB(groups[sectionIndex]);
        m_filter_logics.append(logic);
    } else {
        if (static_cast<int>(m_filter_logics[idx].op()) == op) return;
        m_filter_logics[idx].setOp(static_cast<control::v1::LogicOp>(op));
    }
    Q_EMIT filterLogicsChanged();
}

void FilterRuleModel::appendRuleInGroup(int group) {
    const int role = roleForName_("group");
    const int row  = findInsertPosition(group);
    if (! insertRows(row, 1)) return;
    if (role < 0) return;
    QVariant   v          = item(row);
    const auto metaType   = v.metaType();
    const auto metaObject = metaType.metaObject();
    if (! metaObject) return;
    const int index = metaObject->indexOfProperty("group");
    if (index < 0) return;
    metaObject->property(index).writeOnGadget(v.data(), group);
    setItem(row, v);
}

void FilterRuleModel::appendNewGroup() {
    const int newId       = newGroupId();
    int       previousMax = -1;
    if (rowCount() > 0) {
        const int role = roleForName_("group");
        if (role >= 0) previousMax = data(index(rowCount() - 1, 0), role).toInt();
    }
    appendRuleInGroup(newId);
    if (previousMax >= 0) {
        control::v1::FilterLogic logic;
        logic.setOp(control::v1::LogicOp::LOGIC_OP_AND);
        logic.setGroupA(previousMax);
        logic.setGroupB(newId);
        m_filter_logics.append(logic);
        Q_EMIT filterLogicsChanged();
    }
}

void FilterRuleModel::deleteGroup(int group) {
    const int role = roleForName_("group");
    if (role < 0) return;
    for (int i = rowCount() - 1; i >= 0; --i) {
        if (data(index(i, 0), role).toInt() == group) removeRow(i);
    }
    bool changed = false;
    for (int i = m_filter_logics.size() - 1; i >= 0; --i) {
        const auto& logic = m_filter_logics[i];
        if (logic.groupA() == group || logic.groupB() == group) {
            m_filter_logics.removeAt(i);
            changed = true;
        }
    }
    if (changed) Q_EMIT filterLogicsChanged();
}

void FilterRuleModel::sortByGroup() {
    auto values = items();
    std::stable_sort(values.begin(), values.end(), [this](const QVariant& a, const QVariant& b) {
        return groupOf_(a) < groupOf_(b);
    });
    fromVariantlist(values);
}

void FilterRuleModel::replaceState(const QList<control::v1::WallpaperFilterRule>& filters,
                                   const QList<control::v1::FilterLogic>&         filterLogics) {
    QVariantList values;
    values.reserve(filters.size());
    for (const auto& filter : filters) {
        values.push_back(QVariant::fromValue(filter));
    }
    fromVariantlist(values);
    setFilterLogics(filterLogics);
    sortByGroup();
    setDirty(false);
}

void FilterRuleModel::setDirty(bool v) {
    if (m_dirty == v) return;
    m_dirty = v;
    Q_EMIT dirtyChanged();
}

void FilterRuleModel::markDirty() { setDirty(true); }

WallpaperFilterRuleModel::WallpaperFilterRuleModel(QObject* parent): FilterRuleModel(this, parent) {
    updateRoleNames(control::v1::WallpaperFilterRule::staticMetaObject, this, {});
}

WallpaperFilterRuleModel::~WallpaperFilterRuleModel() = default;

auto WallpaperFilterRuleModel::roleNames() const -> QHash<int, QByteArray> {
    auto roles = FilterRuleModel::roleNames();
    for (auto it = roles.begin(); it != roles.end();) {
        const auto property = propertyOfRole(it.key());
        if (property && ! property->isScriptable())
            it = roles.erase(it);
        else
            ++it;
    }
    return roles;
}

auto WallpaperFilterRuleModel::data(const QModelIndex& index, int role) const -> QVariant {
    if (! index.isValid() || index.row() < 0 || index.row() >= rowCount()) return {};
    const auto property = propertyOfRole(role);
    if (! property) return FilterRuleModel::data(index, role);
    if (! property->isScriptable()) return {};

    // Reading an inactive Qt Protobuf oneof getter can replace the stored payload.
    const QByteArray name = property->name();
    if (name == "stringFilter" || name == "intFilter" || name == "tagFilter") {
        const auto rule = item(index.row()).value<control::v1::WallpaperFilterRule>();
        if (name == "stringFilter") {
            return QVariant::fromValue(rule.hasStringFilter()
                                           ? rule.stringFilter()
                                           : control::v1::WallpaperStringFilter {});
        }
        if (name == "intFilter") {
            return QVariant::fromValue(rule.hasIntFilter() ? rule.intFilter()
                                                           : control::v1::WallpaperIntFilter {});
        }
        return QVariant::fromValue(rule.hasTagFilter() ? rule.tagFilter()
                                                       : control::v1::WallpaperTagFilter {});
    }
    return FilterRuleModel::data(index, role);
}

void WallpaperFilterRuleModel::fromVariantlist(const QVariantList& v) {
    auto view = std::views::transform(v, [](const QVariant& value) {
        return value.value<control::v1::WallpaperFilterRule>();
    });
    resetModel(view);
}

} // namespace waywallen

#include "waywallen/model/filter_rule_model.moc.cpp"
