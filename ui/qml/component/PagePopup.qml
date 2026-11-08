import QtQuick
import QtQuick.Window

import Qcm.Material as MD
import waywallen.ui as W

MD.Popup {
    id: root
    property bool fillHeight: false
    property bool fillWidth: false
    property var props: ({})
    property var initialRequest: null
    property var pendingRequest: null
    required property string source

    MD.Presentation.ready: false
    readyForOpen: MD.Presentation.ready
    positioningItem: overlayItem
    x: Math.round((overlayWidth - width) / 2)
    y: Math.round((overlayHeight - height) / 2)
    width: fillWidth ? overlayWidth : m_size_transition.size.width
    height: fillHeight ? overlayHeight : m_size_transition.size.height

    mdState.textColor: MD.MProp.color.on_surface
    mdState.backgroundColor: MD.MProp.color.surface
    MD.MProp.backgroundColor: MD.MProp.color.surface

    radius: MD.MProp.size.isCompact ? 0 : MD.Token.shape.corner.large
    modal: !MD.MProp.size.isCompact

    Binding {
        when: root.MD.MProp.size.isCompact
        root.fillHeight: true
        root.fillWidth: true
        root.padding: 0
        root.verticalPadding: 0
    }

    QtObject {
        id: d
        property var leases: ({})
        property var accepting: null
        property bool changing: false
        property bool destroying: false
    }

    function acceptPage(request, mode) {
        const item = request.object as Item;
        if (!item)
            return null;
        d.accepting = request;
        try {
            return m_stack.pushItem(item, mode) && m_stack.currentItem === item ? item : null;
        } finally {
            d.accepting = null;
        }
    }

    function rejectInitialRequest(request, error) {
        root.initialRequest = null;
        request.release();
        const pending = root.pendingRequest;
        root.pendingRequest = null;
        if (pending)
            pending.cancel();
        root.MD.Presentation.fail(error);
        root.rejectOpen(error);
    }

    function handleInitialRequest() {
        if (d.destroying)
            return;
        if (d.changing) {
            Qt.callLater(root.handleInitialRequest);
            return;
        }
        const request = root.initialRequest;
        if (!request)
            return;
        d.changing = true;
        try {
            if (request.status === MD.PoolRequest.Ready) {
                root.initialRequest = null;
                const item = root.acceptPage(request, MD.PageStack.Immediate);
                if (!item) {
                    root.rejectInitialRequest(request, qsTr("Failed to open page"));
                    return;
                }

                Qt.callLater(function () {
                    if (!d.destroying && m_stack.depth > 0)
                        root.MD.Presentation.ready = true;
                });
            } else if (request.status === MD.PoolRequest.Error) {
                root.rejectInitialRequest(request, request.errorString || qsTr("Failed to load page"));
            }
        } finally {
            d.changing = false;
        }
        root.handlePendingRequest();
    }

    function handlePendingRequest() {
        if (d.destroying)
            return;
        if (d.changing) {
            Qt.callLater(root.handlePendingRequest);
            return;
        }
        if (root.initialRequest)
            return;
        const request = root.pendingRequest;
        if (!request)
            return;

        d.changing = true;
        try {
            if (request.status === MD.PoolRequest.Ready) {
                root.pendingRequest = null;
                if (!root.acceptPage(request, MD.PageStack.Animated)) {
                    W.Global.toastError(qsTr("Failed to open page"));
                    request.release();
                }
            } else if (request.status === MD.PoolRequest.Error) {
                root.pendingRequest = null;
                W.Global.toastError(request.errorString || qsTr("Failed to load page"));
                request.release();
            } else if (request.status === MD.PoolRequest.Cancelled) {
                root.pendingRequest = null;
            }
        } finally {
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
            const previous = root.pendingRequest;
            root.pendingRequest = null;
            if (previous)
                previous.cancel();
            root.pendingRequest = m_pool.request(source, properties, null, MD.Pool.AsynchronousIfNested);
        } finally {
            d.changing = false;
        }
        root.handlePendingRequest();
    }

    function popPage() {
        if (d.destroying)
            return;
        if (d.changing) {
            Qt.callLater(root.popPage);
            return;
        }
        d.changing = true;
        try {
            const previous = root.pendingRequest;
            root.pendingRequest = null;
            if (previous)
                previous.cancel();
            m_stack.popCurrentItem();
        } finally {
            d.changing = false;
        }
    }

    MD.PageContext {
        id: m_page_context
        showHeader: true
        backgroundRadius: root.radius
        radius: root.radius
        leadingAction: MD.Action {
            icon.name: MD.Token.icon.arrow_back
            onTriggered: {
                const cur = m_stack.currentItem;
                if (cur?.canBack) {
                    cur.back();
                } else if (m_stack.depth > 1) {
                    root.popPage();
                } else {
                    root.close();
                }
            }
        }
    }

    MD.Pool {
        id: m_pool
    }

    Connections {
        target: root.initialRequest

        function onStatusChanged() {
            root.handleInitialRequest();
        }
    }

    Connections {
        target: root.pendingRequest

        function onStatusChanged() {
            root.handlePendingRequest();
        }
    }

    Component.onCompleted: {
        d.changing = true;
        try {
            root.initialRequest = m_pool.request(source, props, null, MD.Pool.Asynchronous);
        } finally {
            d.changing = false;
        }
        root.handleInitialRequest();
    }

    Component.onDestruction: {
        d.destroying = true;
        if (root.initialRequest)
            root.initialRequest.cancel();
        if (root.pendingRequest)
            root.pendingRequest.cancel();
    }

    function preparePageClose(page) {
        if (page && typeof page.prepareClose === "function")
            page.prepareClose();
    }

    onAboutToHide: root.preparePageClose(m_stack.currentItem)

    contentItem: MD.PageStack {
        id: m_stack

        sizeTransition: MD.SizeTransition {
            id: m_size_transition

            enabled: root.visible && !(root.fillWidth && root.fillHeight)
            targetSize: Qt.size(root.fillWidth ? root.overlayWidth : Math.max(400, root.implicitWidth),
                                root.fillHeight ? root.overlayHeight : root.implicitHeight)
            maximumSize: Qt.size(root.overlayWidth, root.fillHeight ? root.overlayHeight : root.overlayHeight * 0.8)
        }
        onEntryAdded: (id, item) => {
            if (d.accepting && d.accepting.object === item)
                d.leases[id] = d.accepting;
        }
        onEntryRemoved: id => {
            const request = d.leases[id];
            delete d.leases[id];
            if (!request)
                return;
            const changing = d.changing;
            d.changing = true;
            try {
                request.release();
            } finally {
                d.changing = changing;
            }
        }

        MD.MProp.page: m_page_context
        Connections {
            target: m_page_context

            function onPushItem(comp, props) {
                root.pushPage(comp, props);
            }

            function onPop() {
                root.popPage();
            }
        }
    }
    closePolicy: MD.Popup.CloseOnEscape | MD.Popup.CloseOnPressOutside
}
