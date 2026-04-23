pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    title: 'Source Manage'

    W.LibraryRemoveQuery {
        id: removeQuery
    }

    contentItem: ColumnLayout {
        spacing: 0

        ListView {
            id: m_view
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: W.App.libraryManager.libraries

            delegate: MD.ListItem {
                required property var modelData
                width: m_view.width
                radius: 8

                text: modelData.path
                supportText: "Plugin: " + modelData.pluginName

                leader: MD.Icon {
                    name: MD.Token.icon.folder
                    color: MD.Token.color.on_surface_variant
                }

                trailing: MD.IconButton {
                    icon.name: MD.Token.icon.delete
                    onClicked: {
                        removeQuery.libraryId = modelData.id;
                        removeQuery.reload();
                    }
                }
            }

            footer: Item {
                width: parent.width
                height: 80
            }
        }
    }

    // MD.FAB {
    //     anchors.right: parent.right
    //     anchors.bottom: parent.bottom
    //     anchors.margins: 16
    //     icon.name: MD.Token.icon.add
    //     onClicked: MD.Util.showPopup('waywallen.ui/PagePopup', {
    //         source: 'waywallen.ui/AddLibraryPage'
    //     }, root)
    // }
}
