pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as T
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
                // Success: close popup
                root.parent.parent.close();
            }
        }
    }

    contentItem: MD.Flickable {
        contentHeight: contentCol.implicitHeight

        ColumnLayout {
            id: contentCol
            width: parent.width
            spacing: 16
            Layout.margins: 16

            MD.Text {
                text: "Select Source Plugin"
                typescale: MD.Token.typescale.title_small
            }

            ListView {
                id: pluginList
                Layout.fillWidth: true
                implicitHeight: contentHeight
                model: sourceQuery.sources
                interactive: false
                spacing: 4

                property string selectedPlugin: ""

                delegate: MD.ListItem {
                    required property var modelData
                    width: pluginList.width
                    radius: 8
                    text: modelData.name
                    supportText: modelData.types.join(", ")
                    checked: pluginList.selectedPlugin === modelData.name
                    onClicked: pluginList.selectedPlugin = modelData.name
                }
            }

            MD.Divider { Layout.fillWidth: true }

            MD.Text {
                text: "Library Path"
                typescale: MD.Token.typescale.title_small
            }

            // Using a simple Rectangle + TextInput as a fallback for TextField
            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: 48
                radius: 8
                color: MD.Token.color.surface_container_highest
                border.color: pathInput.activeFocus ? MD.Token.color.primary : "transparent"
                border.width: 2

                TextInput {
                    id: pathInput
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 12
                    verticalAlignment: TextInput.AlignVCenter
                    color: MD.Token.color.on_surface
                    font.pixelSize: 16
                    clip: true

                    property string placeholder: "e.g. /home/user/Pictures/Wallpapers"
                    Text {
                        text: pathInput.placeholder
                        color: MD.Token.color.on_surface_variant
                        visible: !pathInput.text && !pathInput.activeFocus
                        anchors.fill: parent
                        verticalAlignment: Text.AlignVCenter
                    }
                }
            }

            Item { Layout.fillHeight: true }

            MD.BusyButton {
                Layout.fillWidth: true
                text: "Add Library"
                busy: addQuery.querying
                enabled: pluginList.selectedPlugin !== "" && pathInput.text !== ""
                mdState.type: MD.Enum.BtFilled
                onClicked: {
                    addQuery.pluginName = pluginList.selectedPlugin;
                    addQuery.path = pathInput.text;
                    addQuery.reload();
                }
            }
        }
    }
}
