#pragma once

#include "kstore/qt/gadget_model.hpp"

namespace waywallen::model
{

class GridListModel : public kstore::QGadgetListModel {
public:
    static constexpr int KeyRole  = Qt::UserRole + 0x10000;
    static constexpr int ItemRole = KeyRole + 1;

    template<typename T>
    explicit GridListModel(T* oper, QObject* parent = nullptr)
        : kstore::QGadgetListModel(oper, parent) {}

    auto roleNames() const -> QHash<int, QByteArray> override {
        auto roles = kstore::QGadgetListModel::roleNames();
        roles.insert(KeyRole, "itemKey");
        roles.insert(ItemRole, "item");
        return roles;
    }

    auto data(const QModelIndex& index, int role = Qt::DisplayRole) const -> QVariant override {
        if (! index.isValid() || index.parent().isValid() || index.column() != 0 ||
            index.row() < 0 || index.row() >= rowCount())
            return {};
        if (role == KeyRole) return m_oper->rawKeyAt(index.row());
        if (role == ItemRole) return item(index.row());
        return kstore::QGadgetListModel::data(index, role);
    }
};

} // namespace waywallen::model
