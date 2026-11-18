pragma ComponentBehavior: Bound
import QtQuick
import Qcm.Material as MD

MD.PageStack {
    id: root

    required property MD.PageContext pageContext
    readonly property bool canBack: depth > 1 || pendingRequest !== null
    property var pendingRequest: null
    signal pageLoadFailed(string error)

    MD.MProp.page: pageContext
    backGestureEnabled: true

    QtObject {
        id: d
        property var accepting: null
        property var leases: ({})
        property bool changing: false
        property bool destroying: false
    }

    MD.Pool {
        id: pool
    }

    function acceptPending() {
        if (d.destroying || d.changing)
            return;
        const request = pendingRequest;
        if (!request || request.status === MD.PoolRequest.Loading || request.status === MD.PoolRequest.Null)
            return;
        d.changing = true;
        try {
            pendingRequest = null;
            if (request.status === MD.PoolRequest.Ready) {
                const item = request.object as Item;
                d.accepting = request;
                if (!item || !root.pushItem(item)) {
                    request.release();
                    root.pageLoadFailed(qsTr("Failed to open page"));
                }
            } else if (request.status === MD.PoolRequest.Error) {
                root.pageLoadFailed(request.errorString);
                request.release();
            }
        } finally {
            d.accepting = null;
            d.changing = false;
        }
    }

    function pushPage(source, properties) {
        if (d.destroying)
            return;
        if (d.changing) {
            Qt.callLater(root.pushPage, source, properties);
            return;
        }
        d.changing = true;
        try {
            cancelPending();
            pendingRequest = pool.request(source, properties, null, MD.Pool.Asynchronous);
        } finally {
            d.changing = false;
        }
        acceptPending();
    }

    function cancelPending() {
        const request = pendingRequest;
        pendingRequest = null;
        request?.cancel();
    }

    function back() {
        if (d.changing) {
            Qt.callLater(root.back);
            return;
        }
        if (pendingRequest)
            cancelPending();
        else if (depth > 1)
            popCurrentItem();
    }

    function reset() {
        if (d.changing) {
            Qt.callLater(root.reset);
            return;
        }
        cancelPending();
        completeTransition();
        while (depth > 1)
            popCurrentItem(MD.PageStack.Immediate);
    }

    onEntryAdded: (id, item) => {
        if (d.accepting?.object === item)
            d.leases[id] = d.accepting;
    }
    onEntryRemoved: id => {
        const request = d.leases[id];
        delete d.leases[id];
        if (!request || d.destroying)
            return;
        const changing = d.changing;
        d.changing = true;
        try {
            request.release();
        } finally {
            d.changing = changing;
        }
    }

    Connections {
        target: root.pendingRequest
        function onStatusChanged() {
            root.acceptPending();
        }
    }
    Connections {
        target: root.pageContext
        function onPushItem(source, properties) {
            root.pushPage(source, properties);
        }
        function onPop() {
            root.back();
        }
    }

    Component.onDestruction: {
        d.destroying = true;
        cancelPending();
    }
}
