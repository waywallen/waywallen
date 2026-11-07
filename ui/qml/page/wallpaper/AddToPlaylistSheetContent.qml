pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD

ColumnLayout {
    id: control

    required property var sheetState

    width: parent ? parent.width : implicitWidth
    Layout.fillWidth: true
    spacing: 0

    ColumnLayout {
        Layout.fillWidth: true
        Layout.topMargin: 16
        Layout.leftMargin: 16
        Layout.rightMargin: 16
        Layout.bottomMargin: 16
        spacing: 8

        MD.Text {
            Layout.fillWidth: true
            text: qsTr("Add to playlist")
            typescale: MD.Token.typescale.title_medium
            color: MD.Token.color.on_surface
            maximumLineCount: 1
            elide: Text.ElideRight
        }

        MD.LinearIndicator {
            Layout.fillWidth: true
            visible: control.sheetState.playlistListLoading
            running: visible
        }

        MD.Text {
            Layout.fillWidth: true
            Layout.preferredHeight: 56
            visible: !control.sheetState.playlistListLoading && control.sheetState.playlists.length === 0
            text: qsTr("No manual playlists found")
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.on_surface_variant
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }

        MD.VerticalLazyList {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(260, Math.max(88, contentHeight + topMargin + bottomMargin))
            visible: control.sheetState.playlists.length > 0
            interactive: contentHeight + topMargin + bottomMargin > height
            source: MD.ListSnapshotSource {
                items: control.sheetState.playlists
            }
            spacing: 6

            delegate: MD.ListItem {
                id: selectPlaylistItem

                required property MD.LazyRow row
                readonly property var modelData: row.value
                itemRow: row

                radius: 12
                text: modelData.name || qsTr("Untitled")
                supportText: qsTr("%n wallpaper(s)", "", (modelData.entryIds || []).length)
                enabled: control.sheetState.selectedWallpaperCount > 0 && !control.sheetState.mutationQuerying
                onClicked: control.sheetState.addToPlaylist(modelData)

                trailing: MD.Icon {
                    name: MD.Token.icon.add
                    size: 24
                    color: selectPlaylistItem.mdState.textColor
                }
            }
        }
    }
}
