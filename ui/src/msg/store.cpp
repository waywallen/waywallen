module;
#include "waywallen/msg/store.moc.h"

module waywallen;
import qextra;
import :msg.store;

namespace waywallen
{

namespace
{
auto store_instance() -> AppStore* {
    static AppStore* instance { new AppStore(App::instance()) };
    return instance;
}
} // namespace

AppStore::AppStore(QObject* parent): QObject(parent), wallpapers(), remotes() {}

AppStore::~AppStore() {}

auto AppStore::instance() -> AppStore* { return store_instance(); }

AppStore* AppStore::create(QQmlEngine*, QJSEngine*) {
    auto self = store_instance();
    QJSEngine::setObjectOwnership(self, QJSEngine::ObjectOwnership::CppOwnership);
    return self;
}

void AppStore::setRemoteAcquisitionState(const QString& sourceId, const QString& itemId, int state,
                                         bool transferActive, quint64 receivedBytes,
                                         quint64 totalBytes) {
    auto key = model::remoteKey(sourceId, itemId);
    auto row = remotes.store_query(key);
    if (! row ||
        (row->acquisitionState == state && row->downloadTransferActive == transferActive &&
         row->downloadReceivedBytes == receivedBytes && row->downloadTotalBytes == totalBytes))
        return;
    row->acquisitionState       = state;
    row->downloadTransferActive = transferActive;
    row->downloadReceivedBytes  = receivedBytes;
    row->downloadTotalBytes     = totalBytes;
    remotes.store_changed_callback(std::span { &key, 1 });
}

auto AppStore::mergeRemoteRow(model::RemoteRow row) const -> model::RemoteRow {
    const auto previous = remotes.store_query(model::remoteKey(row.sourceId, row.itemId));
    if (row.acquisitionState != 3 && previous &&
        (previous->acquisitionState == 1 || previous->acquisitionState == 2)) {
        row.acquisitionState       = previous->acquisitionState;
        row.downloadTransferActive = previous->downloadTransferActive;
        row.downloadReceivedBytes  = previous->downloadReceivedBytes;
        row.downloadTotalBytes     = previous->downloadTotalBytes;
    }
    return row;
}

} // namespace waywallen

#include "waywallen/msg/store.moc.cpp"
