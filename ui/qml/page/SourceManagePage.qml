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

    contentItem: Item {
        implicitHeight: children[0].implicitHeight + m_fab.implicitHeight + 16 * 2
        implicitWidth: children[0].implicitWidth

        MD.VerticalListView {
            id: m_view
            width: parent.width
            model: W.App.libraryManager.libraries
            expand: true
            spacing: 8

            leftMargin: 12
            rightMargin: 12

            delegate: MD.ListItem {
                required property var modelData
                width: m_view.contentWidth
                radius: 8

                mdState.backgroundColor: MD.Token.color.surface_container

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
        }

        MD.FAB {
            id: m_fab
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: 16
            icon.name: MD.Token.icon.add
            onClicked: root.MD.MProp.page.pushItem('waywallen.ui/AddLibraryPage')
        }
    }
}
