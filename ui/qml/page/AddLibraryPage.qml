pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Dialogs
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    title: "Add Library"

    W.SourceListQuery {
        id: sourceQuery
        Component.onCompleted: reload()
    }

    W.LibraryAddQuery {
        id: addQuery
        onStatusChanged: {
            if (status === 3) {
                root.parent.parent.close();
            }
        }
    }

    MD.ButtonGroup {
        id: pluginGroup
        exclusive: true
        property string selectedPlugin: ""
    }

    FolderDialog {
        id: folderDialog
        title: "Choose Library Folder"
        onAccepted: {
            pathInput.text = selectedFolder.toString().replace(/^file:\/\//, "");
        }
    }

    contentItem: MD.Flickable {
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
                text: "Select Source Plugin"
                typescale: MD.Token.typescale.title_small
            }

            Flow {
                Layout.fillWidth: true
                spacing: 8

                Repeater {
                    model: sourceQuery.sources
                    delegate: MD.FilterChip {
                        required property var modelData
                        MD.ButtonGroup.group: pluginGroup
                        text: modelData.name
                        onClicked: pluginGroup.selectedPlugin = checked ? modelData.name : ""
                    }
                }
            }

            MD.Divider { Layout.fillWidth: true }

            MD.Text {
                text: "Library Path"
                typescale: MD.Token.typescale.title_small
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                MD.TextField {
                    id: pathInput
                    Layout.fillWidth: true
                    placeholderText: "e.g. /home/user/Pictures/Wallpapers"
                }

                MD.IconButton {
                    Layout.alignment: Qt.AlignVCenter
                    icon.name: MD.Token.icon.folder
                    onClicked: folderDialog.open()
                }
            }

            Item { Layout.fillHeight: true }

            MD.BusyButton {
                Layout.fillWidth: true
                text: "Add Library"
                busy: addQuery.querying
                enabled: pluginGroup.selectedPlugin !== "" && pathInput.text !== ""
                mdState.type: MD.Enum.BtFilled
                onClicked: {
                    addQuery.pluginName = pluginGroup.selectedPlugin;
                    addQuery.path = pathInput.text;
                    addQuery.reload();
                }
            }
        }
    }
}
