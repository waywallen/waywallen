pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    title: qsTr("Add Library")
    property string pathError: ""
    property string selectedPlugin: ""
    onSelectedPluginChanged: root.pathError = ""

    W.SourceListQuery {
        id: sourceQuery
    }

    // `daemonReady` is edge-triggered, so also level-check in
    // Component.onCompleted for the case where this page is pushed
    // after the daemon is already Ready. Without the ready gate an
    // early reload returns nothing and the source plugin list stays
    // empty with no retry.
    Connections {
        target: W.Notify
        function onDaemonReady() {
            sourceQuery.reload();
        }
        function onPluginChanged() {
            sourceQuery.reload();
        }
    }

    Component.onCompleted: {
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready)
            sourceQuery.reload();
    }

    W.LibraryAddQuery {
        id: addQuery
        forwardError: false
        onErrorOccurred: function (error) {
            root.pathError = error;
        }
        // Toast now fires globally from Window.qml on the daemon's
        // `LibrariesAdded` event (mirrored via Notify).
        onFinished: MD.Util.closePopup(root.MD.MProp.page)
    }

    readonly property var selectedSource: {
        const list = sourceQuery.sources || [];
        for (let i = 0; i < list.length; ++i) {
            if (list[i].name === root.selectedPlugin)
                return list[i];
        }
        return null;
    }

    MD.FolderDialog {
        id: folderDialog
        title: qsTr("Choose Library Folder")
        onAccepted: {
            pathInput.text = selectedFolder.toString().replace(/^file:\/\//, "");
        }
    }

    contentItem: MD.Flickable {
        MD.NestedScroll.smoothWheelEnabled: W.Global.smoothScrolling
        implicitHeight: contentHeight
        contentHeight: contentCol.implicitHeight
        leftMargin: 12
        rightMargin: 12

        ColumnLayout {
            id: contentCol
            width: parent.width
            spacing: 16
            Layout.margins: 16

            MD.Text {
                text: qsTr("Select Source Plugin")
                typescale: MD.Token.typescale.title_small
            }

            Flow {
                Layout.fillWidth: true
                spacing: 8
                enabled: !addQuery.querying

                Repeater {
                    model: sourceQuery.sources
                    delegate: MD.FilterChip {
                        required property var modelData
                        text: modelData.name
                        selected: root.selectedPlugin === modelData.name
                        onSelectionRequested: root.selectedPlugin = modelData.name
                    }
                }
            }

            MD.Divider {
                Layout.fillWidth: true
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                MD.TextField {
                    id: pathInput
                    Layout.fillWidth: true
                    mdState.size: MD.Enum.S
                    enabled: !addQuery.querying
                    error: root.pathError.length > 0
                    errorText: root.pathError
                    onTextChanged: root.pathError = ""
                    placeholderText: root.selectedSource ? (W.I18n.tr(root.selectedSource.libraryLabelText) || qsTr("Library Path")) : qsTr("Library Path")
                }

                MD.IconButton {
                    Layout.alignment: Qt.AlignVCenter
                    enabled: !addQuery.querying
                    icon.name: MD.Token.icon.folder
                    onClicked: folderDialog.open()
                }
            }

            MD.Text {
                Layout.fillWidth: true
                visible: text.length > 0
                text: root.selectedSource ? W.I18n.tr(root.selectedSource.libraryHintText) : ""
                wrapMode: Text.WordWrap
                typescale: MD.Token.typescale.body_small
                color: MD.Token.color.on_surface_variant
            }

            MD.TextField {
                id: nameInput
                Layout.fillWidth: true
                mdState.size: MD.Enum.S
                enabled: !addQuery.querying
                placeholderText: qsTr("Library name (optional)")
                supportingText: qsTr("Leave empty to use the folder name.")
            }

            Item {
                Layout.fillHeight: true
            }

            MD.BusyButton {
                Layout.fillWidth: true
                text: qsTr("Add Library")
                busy: addQuery.querying
                enabled: !addQuery.querying && root.selectedPlugin !== "" && pathInput.text !== ""
                mdState.type: MD.Enum.BtFilled
                onClicked: {
                    root.pathError = "";
                    addQuery.pluginName = root.selectedPlugin;
                    addQuery.path = pathInput.text;
                    addQuery.name = nameInput.text;
                    addQuery.reload();
                }
            }

            Item {}
        }
    }
}
