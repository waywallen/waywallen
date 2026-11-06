module;
#include "waywallen/model/remote_model.moc.h"
#include <cstddef>

module waywallen;
import qextra;
import :model.remote;

namespace waywallen::model
{

RemoteListModel::RemoteListModel(QObject* parent): GridListModel(this, parent), list_crtp_t() {
    connect(this, &QAbstractItemModel::modelReset, this, &RemoteListModel::countChanged);
    connect(this, &QAbstractItemModel::rowsInserted, this, &RemoteListModel::countChanged);
    connect(this, &QAbstractItemModel::rowsRemoved, this, &RemoteListModel::countChanged);
}

void RemoteListModel::reset(QList<RemoteRow> rows, bool hasMore) {
    setHasMore(hasMore);
    resetModel(rows);
}

void RemoteListModel::append(const QList<RemoteRow>& rows, bool hasMore) {
    setHasMore(hasMore);
    if (! rows.isEmpty()) insert(static_cast<int>(size()), rows);
}

QStringList RemoteListModel::itemIds() const {
    QStringList ids;
    ids.reserve(static_cast<qsizetype>(size()));
    for (std::size_t i = 0; i < size(); ++i) ids.push_back(at(i).itemId);
    return ids;
}

} // namespace waywallen::model

#include "waywallen/model/remote_model.moc.cpp"
