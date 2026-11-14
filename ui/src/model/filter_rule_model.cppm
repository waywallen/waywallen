module;
#include "QExtra/macro_qt.hpp"

#ifdef Q_MOC_RUN
#    include "waywallen/model/filter_rule_model.moc"
#endif

export module waywallen:model.filter_rule;
export import :proto;
import qextra;
import rstd.cppstd;

export namespace waywallen
{

class FilterRuleModel : public kstore::QGadgetListModel {
    Q_OBJECT
    QML_ANONYMOUS

    Q_PROPERTY(bool dirty READ dirty NOTIFY dirtyChanged FINAL)
    Q_PROPERTY(QList<waywallen::control::v1::FilterLogic> filterLogics READ filterLogics WRITE
                   setFilterLogics NOTIFY filterLogicsChanged FINAL)

public:
    FilterRuleModel(kstore::QListInterface* list, QObject* parent = nullptr);
    ~FilterRuleModel() override;

    Q_SIGNAL void apply();
    Q_SIGNAL void reset();

    Q_INVOKABLE void replaceState(const QList<waywallen::control::v1::WallpaperFilterRule>& filters,
                                  const QList<waywallen::control::v1::FilterLogic>& filterLogics);

    auto          dirty() const noexcept -> bool { return m_dirty; }
    void          setDirty(bool v);
    Q_SIGNAL void dirtyChanged();

    auto filterLogics() const noexcept -> const QList<control::v1::FilterLogic>& {
        return m_filter_logics;
    }
    void          setFilterLogics(const QList<control::v1::FilterLogic>&);
    Q_SIGNAL void filterLogicsChanged();

    Q_INVOKABLE int  newGroupId() const;
    Q_INVOKABLE int  countInGroup(int group) const;
    Q_INVOKABLE int  rowIndexInGroup(int row) const;
    Q_INVOKABLE int  rowCountInGroupOf(int row) const;
    Q_INVOKABLE int  findInsertPosition(int group) const;
    Q_INVOKABLE int  sectionIndexForGroup(int group) const;
    Q_INVOKABLE int  findLogicAt(int sectionIndex) const;
    Q_INVOKABLE int  logicOpAt(int sectionIndex) const;
    Q_INVOKABLE void setLogicOpAt(int sectionIndex, int op);
    Q_INVOKABLE void appendRuleInGroup(int group);
    Q_INVOKABLE void appendNewGroup();
    Q_INVOKABLE void deleteGroup(int group);
    Q_INVOKABLE void sortByGroup();

protected:
    virtual void fromVariantlist(const QVariantList& v) = 0;

private:
    int  roleForName_(const QByteArray& name) const;
    int  groupOf_(const QVariant& v) const;
    auto orderedGroups_() const -> QList<int>;

    Q_SLOT void markDirty();

    bool                            m_dirty { false };
    QList<control::v1::FilterLogic> m_filter_logics;
};

class WallpaperFilterRuleModel
    : public FilterRuleModel,
      public kstore::QMetaListModelCRTP<control::v1::WallpaperFilterRule, WallpaperFilterRuleModel,
                                        kstore::ListStoreType::Vector> {
    Q_OBJECT
    QML_ELEMENT

public:
    WallpaperFilterRuleModel(QObject* parent = nullptr);
    ~WallpaperFilterRuleModel() override;

    auto roleNames() const -> QHash<int, QByteArray> override;
    auto data(const QModelIndex& index, int role) const -> QVariant override;
    void fromVariantlist(const QVariantList& v) override;
};

} // namespace waywallen
