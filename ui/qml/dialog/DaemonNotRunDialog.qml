pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts

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

    visible: !W.DaemonDBusClient.daemonShutdownExpected && (!dbusConnected || daemonStarting)
    closePolicy: MD.Popup.NoAutoClose
    dim: true
    modal: true
    parent: MD.Overlay.overlay
    x: Math.round((parent.width - width) / 2)
    y: Math.round((parent.height - height) / 2)
    bottomPadding: 24

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
                return "";
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
                return "";
            }
        }

        MD.LinearIndicator {
            Layout.fillWidth: true
            Layout.leftMargin: 24
            Layout.rightMargin: 24
            visible: root.daemonStarting
        }

        MD.VerticalLazyList {
            id: m_proc_list
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            Layout.preferredWidth: 300
            implicitHeight: Math.min(contentHeight, 200)
            visible: !root.daemonStarting && m_proc_model.count > 0
            clip: true
            spacing: 4
            source: MD.ItemModelSource {
                keyMode: MD.ItemModelSource.PersistentIndexKey
                model: ListModel {
                    id: m_proc_model
                }
            }

            delegate: MD.ListItem {
                id: m_item
                required property MD.LazyRow row
                itemRow: row
                spacing: 8
                readonly property int pid: model.pid
                readonly property string cmdline: model.cmdline

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
                        W.DaemonDBusClient.killProcess(m_item.pid);
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
                MD.DialogButtonBox.buttonRole: MD.DialogButtonBox.RejectRole
                onClicked: Qt.quit()
            }
        }
    }
}
