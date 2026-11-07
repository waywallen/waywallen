pragma ComponentBehavior: Bound
import QtQuick
import Qcm.Material as MD
import waywallen.control as WC
import waywallen.ui as W

MD.Page {
    id: root

    implicitWidth: 448
    implicitHeight: 600
    title: qsTr("Hidden wallpapers")
    scrolling: !wallpaperList.atYBeginning

    actions: [
        MD.Action {
            text: qsTr("Refresh")
            icon.name: MD.Token.icon.refresh
            enabled: !hiddenQuery.querying
            onTriggered: hiddenQuery.delayReload()
        }
    ]

    W.WallpaperListQuery {
        id: hiddenQuery
        hiddenFilter: WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_ONLY
        limit: 100
        forwardError: false
    }

    W.WallpaperHideQuery {
        id: unhideQuery
        forwardError: false
        hidden: false
        onUpdated: function (wallpaperIds, hidden, updatedCount) {
            W.Action.toast(qsTr("Unhidden"));
            if (updatedCount === 0)
                hiddenQuery.delayReload();
        }
        onStatusChanged: {
            if (status === 3)
                W.Action.toast(error || qsTr("Unhide failed"), 6000, 1, null);
        }
    }

    Connections {
        target: W.Notify
        function onWallpaperHiddenChanged(wallpaperIds, hidden) {
            hiddenQuery.delayReload();
        }
        function onWallpaperSyncFinished(count, error) {
            hiddenQuery.delayReload();
        }
        function onDaemonReady() {
            hiddenQuery.delayReload();
        }
    }

    Component.onCompleted: hiddenQuery.delayReload()

    contentItem: Item {
        implicitWidth: root.implicitWidth
        implicitHeight: root.implicitHeight

        MD.VerticalListView {
            id: wallpaperList

            anchors.fill: parent
            clip: true
            model: hiddenQuery.data
            busy: hiddenQuery.querying && count > 0
            spacing: 6
            topMargin: 8
            bottomMargin: 16
            leftMargin: 16
            rightMargin: 16

            delegate: MD.ListItem {
                id: wallpaperItem
                required property int index
                required property var model

                width: wallpaperList.contentWidth
                radius: 10
                text: model.name || qsTr("Untitled")
                heightMode: MD.Enum.ListItemTwoLine
                wrapMode: Text.Wrap
                elide: Text.ElideRight
                maximumLineCount: 2
                mdState.backgroundColor: MD.Token.color.surface_container

                leader: W.ThumbnailImage {
                    implicitWidth: 72
                    implicitHeight: 48
                    source: wallpaperItem.model.preview || ""
                    resource: wallpaperItem.model.resource || ""
                    wpType: wallpaperItem.model.wpType || ""
                    timeline: (wallpaperItem.model.subType || 0) === 1
                    fillMode: Image.PreserveAspectCrop
                }

                trailing: MD.BusyIconButton {
                    mdState.size: MD.Enum.XS
                    enabled: !unhideQuery.querying
                    busy: unhideQuery.querying && unhideQuery.wallpaperId === wallpaperItem.model.id_proto ? MD.Enum.Busy : MD.Enum.Idle
                    icon.name: MD.Token.icon.visibility
                    onClicked: {
                        unhideQuery.wallpaperId = wallpaperItem.model.id_proto;
                        unhideQuery.reload();
                    }
                    MD.ToolTip.visible: mdState.interactions.hovered
                    MD.ToolTip.text: qsTr("Unhide")
                }
            }
        }

        MD.BusyIndicator {
            anchors.centerIn: parent
            running: hiddenQuery.querying && wallpaperList.count === 0
        }

        MD.Text {
            anchors.centerIn: parent
            width: Math.max(0, parent.width - 48)
            visible: hiddenQuery.status === 2 && wallpaperList.count === 0
            text: qsTr("No hidden wallpapers")
            typescale: MD.Token.typescale.body_large
            color: MD.Token.color.on_surface_variant
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
        }

        MD.Text {
            anchors.centerIn: parent
            width: Math.max(0, parent.width - 48)
            visible: hiddenQuery.status === 3
            text: hiddenQuery.error || qsTr("Failed to load hidden wallpapers")
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.error
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
        }
    }
}
