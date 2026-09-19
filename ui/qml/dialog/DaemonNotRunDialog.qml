pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T

import Qcm.Material as MD
import waywallen.ui as W

MD.Popup {
    id: root

    // Dialog is the single anchor for "daemon is not usable yet". It
    // wins on either of two orthogonal conditions:
    //   - DBus says the daemon process is missing / version-mismatched
    //   - DBus is connected but the daemon's `phase` is still Starting
    //     (WS not bound yet, or core services still booting)
    readonly property bool dbusConnected: W.DaemonDBusClient.status === W.DaemonDBusClient.Connected
    readonly property bool daemonStarting: dbusConnected && W.Notify.daemonPhase !== W.Notify.DaemonPhase.Ready
    readonly property bool daemonUnusable: !W.DaemonDBusClient.daemonShutdownExpected &&
        (!dbusConnected || daemonStarting)

    // The phase is `Starting` until the daemon's first StatusSync, and the
    // window is on screen before the WebSocket has finished connecting, so
    // on a healthy launch this dialog is briefly correct — long enough to
    // flash a dimmed overlay, too short to read. Not having heard from the
    // daemon yet is not the same as something being wrong, and only a
    // condition that outlives the grace period is worth interrupting the
    // window for: a daemon that is genuinely missing or still booting stays
    // that way for seconds, while a healthy handshake clears in tens of
    // milliseconds. Qt carries the same idea in `QProgressDialog`, whose
    // `minimumDuration` exists so a dialog does not "pop up for tasks that
    // are quickly over". Nothing else waits on this — the pages gate their
    // queries on `daemonPhase` directly.
    visible: daemonUnusable && m_grace.elapsed
    closePolicy: T.Popup.NoAutoClose
    dim: true
    modal: true
    parent: T.Overlay.overlay
    x: Math.round((parent.width - width) / 2)
    y: Math.round((parent.height - height) / 2)
    bottomPadding: 24

    // Started and stopped by hand rather than bound to `running`: a
    // one-shot Timer stops itself when it fires, which would break the
    // binding and clear the flag the moment it was set.
    Timer {
        id: m_grace
        interval: 1000
        property bool elapsed: false
        onTriggered: elapsed = true
    }

    onDaemonUnusableChanged: {
        if (daemonUnusable) {
            m_grace.restart();
        } else {
            m_grace.stop();
            m_grace.elapsed = false;
        }
    }

    Component.onCompleted: if (daemonUnusable)
        m_grace.restart()

    function refreshProcs() {
        m_proc_model.clear();
        const list = W.DaemonDBusClient.listWaywallenProcesses();
        for (let i = 0; i < list.length; ++i) {
            m_proc_model.append(list[i]);
        }
    }

    onVisibleChanged: if (visible && !daemonStarting)
        refreshProcs()

    Connections {
        target: W.DaemonDBusClient
        function onStatusChanged() {
            if (root.visible && !root.daemonStarting)
                root.refreshProcs();
        }
    }

    contentItem: ColumnLayout {
        spacing: 16

        MD.DialogHeader {
            Layout.fillWidth: true
            title: {
                if (root.daemonStarting)
                    return qsTr("Starting…");
                switch (W.DaemonDBusClient.status) {
                case W.DaemonDBusClient.Disconnected:
                    return qsTr("Daemon not running");
                case W.DaemonDBusClient.VersionMissing:
                    return qsTr("Daemon too old");
                case W.DaemonDBusClient.VersionMismatch:
                    return qsTr("Daemon version mismatch");
                }
                // Connected with the phase ready is this dialog on its way
                // out: the exit transition is still running while the
                // bindings have already turned good. An empty string here
                // collapses the popup to a blank 200x48 background for the
                // length of that transition, so keep the last copy instead.
                return qsTr("Starting…");
            }
        }

        MD.Label {
            Layout.fillWidth: true
            Layout.leftMargin: 24
            Layout.rightMargin: 24
            wrapMode: Text.WordWrap
            text: {
                if (root.daemonStarting)
                    return qsTr("waywallen is initializing core services. This usually takes a few seconds.");
                switch (W.DaemonDBusClient.status) {
                case W.DaemonDBusClient.Disconnected:
                    return qsTr("The waywallen daemon is not on the session bus.");
                case W.DaemonDBusClient.VersionMissing:
                    return qsTr("Daemon is online but does not advertise a version.");
                case W.DaemonDBusClient.VersionMismatch:
                    return qsTr("Daemon version %1 + is incompatible.").arg(W.DaemonDBusClient.daemonVersion);
                }
                // See the title: this is the closing frame, not a state.
                return qsTr("waywallen is initializing core services. This usually takes a few seconds.");
            }
        }

        MD.LinearIndicator {
            Layout.fillWidth: true
            Layout.leftMargin: 24
            Layout.rightMargin: 24
            visible: root.daemonStarting
        }

        MD.VerticalListView {
            id: m_proc_list
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            Layout.preferredWidth: 300
            implicitHeight: Math.min(contentHeight, 200)
            visible: !root.daemonStarting && m_proc_model.count > 0
            clip: true
            spacing: 4
            model: ListModel {
                id: m_proc_model
            }

            delegate: MD.ListItem {
                id: m_item
                width: ListView.view ? ListView.view.contentWidth : 0
                spacing: 8
                required property int pid
                required property string cmdline

                text: cmdline
                elide: Text.ElideLeft
                background: MD.Rectangle {
                    color: root.MD.MProp.color.surface
                    corners: MD.Util.listCorners(index, count, 16)
                }

                leader: MD.Text {
                    text: m_item.pid
                }

                trailing: MD.BusyButton {
                    text: qsTr("Kill")
                    mdState.type: MD.Enum.BtText
                    busy: m_t.running
                    onClicked: {
                        W.DaemonDBusClient.killProcess(parent.pid);
                        m_t.start();
                    }
                }

                Timer {
                    id: m_t
                    interval: 2000
                    onTriggered: root.refreshProcs()
                }
            }
        }

        MD.DialogButtonBox {
            Layout.fillWidth: true

            MD.Button {
                text: qsTr("Exit")
                mdState.type: MD.Enum.BtText
                T.DialogButtonBox.buttonRole: T.DialogButtonBox.RejectRole
                onClicked: Qt.quit()
            }
            MD.Button {
                text: qsTr("Restart")
                mdState.type: MD.Enum.BtText
                T.DialogButtonBox.buttonRole: T.DialogButtonBox.AcceptRole
                visible: !root.daemonStarting
                onClicked: W.DaemonDBusClient.launchDaemon()
            }
        }
    }
}
