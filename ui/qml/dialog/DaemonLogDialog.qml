pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Dialog {
    id: root
    title: qsTr("Daemon log")
    implicitWidth: 720
    implicitHeight: 520
    width: Math.min(implicitWidth, parent ? parent.width - 48 : implicitWidth)
    height: Math.min(implicitHeight, parent ? parent.height - 48 : implicitHeight)
    horizontalPadding: 16
    standardButtons: MD.Dialog.Close

    function scrollToLatest() {
        Qt.callLater(function () {
            logFlick.contentY = Math.max(logFlick.originY,
                                         logFlick.contentHeight - logFlick.height + logFlick.bottomMargin);
        });
    }

    W.DaemonLogQuery {
        id: logQuery
        forwardError: false
        onContentChanged: root.scrollToLatest()
    }

    MD.Action {
        id: refreshAction
        icon.name: MD.Token.icon.refresh
        text: qsTr("Refresh")
        displayHint: MD.ToolBarLayout.KeepVisible
        busy: logQuery.querying ? MD.Enum.Busy : MD.Enum.Idle
        enabled: !logQuery.querying
        onTriggered: logQuery.reload()
    }

    MD.Action {
        id: copyAction
        icon.name: MD.Token.icon.content_copy
        text: qsTr("Copy")
        displayHint: MD.ToolBarLayout.KeepVisible
        enabled: logQuery.content.length > 0
        onTriggered: {
            W.Action.copyToClipboard(logQuery.content);
            W.Action.toast(qsTr("Copied to clipboard"), 2000);
        }
    }

    Component.onCompleted: logQuery.reload()

    contentItem: ColumnLayout {
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            MD.Text {
                Layout.fillWidth: true
                text: logQuery.path
                visible: text.length > 0
                typescale: MD.Token.typescale.body_small
                color: MD.Token.color.on_surface_variant
                elide: Text.ElideLeft
                maximumLineCount: 1
            }

            Item {
                readonly property real targetWidth: Math.ceil(logActions.maximumContentWidth) + 2

                implicitWidth: targetWidth
                implicitHeight: logActions.implicitHeight
                Layout.minimumWidth: targetWidth
                Layout.preferredWidth: targetWidth
                Layout.maximumWidth: targetWidth

                MD.ActionToolBar {
                    id: logActions
                    anchors.fill: parent
                    actions: [refreshAction, copyAction]
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            color: MD.Token.color.surface_container_low
            radius: 12

            MD.Flickable {
                id: logFlick
                MD.NestedScroll.smoothWheelEnabled: W.Global.smoothScrolling
                anchors.fill: parent
                leftMargin: 12
                rightMargin: 12
                topMargin: 12
                bottomMargin: 12
                contentWidth: Math.max(width, logText.implicitWidth + leftMargin + rightMargin)
                contentHeight: Math.max(height, logText.implicitHeight + topMargin + bottomMargin)

                MD.ScrollBarBase.vertical: MD.ScrollBar {}
                MD.ScrollBarBase.horizontal: MD.ScrollBar {}

                MD.TextEdit {
                    id: logText
                    x: logFlick.leftMargin
                    y: logFlick.topMargin
                    width: Math.max(logFlick.width - logFlick.leftMargin - logFlick.rightMargin,
                                    implicitWidth)
                    height: Math.max(logFlick.height - logFlick.topMargin - logFlick.bottomMargin,
                                     implicitHeight)
                    text: logQuery.content
                    readOnly: true
                    selectByMouse: true
                    persistentSelection: true
                    wrapMode: TextEdit.NoWrap
                    font.family: "monospace"
                    typescale: MD.Token.typescale.body_small
                    color: MD.Token.color.on_surface
                }
            }

            MD.Text {
                anchors.centerIn: parent
                width: parent.width - 32
                visible: logQuery.error.length > 0 || (!logQuery.querying && logQuery.content.length === 0)
                text: logQuery.error.length > 0 ? logQuery.error : qsTr("The log is empty.")
                typescale: MD.Token.typescale.body_medium
                color: logQuery.error.length > 0 ? MD.Token.color.error : MD.Token.color.on_surface_variant
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
            }
        }
    }
}
