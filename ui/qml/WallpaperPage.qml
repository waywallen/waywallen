pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui

MD.Page {
    id: root

    // --- Queries ---
    WallpaperListQuery {
        id: wallpaperQuery
        Component.onCompleted: reload()
    }

    WallpaperScanQuery {
        id: scanQuery
    }

    property string typeFilter: ""
    property var filteredWallpapers: {
        const all = wallpaperQuery.wallpapers;
        if (!all || typeFilter === "")
            return all;
        return all.filter(wp => wp.wpType === typeFilter);
    }

    // Collect unique types from wallpapers
    property var availableTypes: {
        const all = wallpaperQuery.wallpapers;
        if (!all) return [];
        const types = new Set();
        for (const wp of all)
            if (wp.wpType) types.add(wp.wpType);
        return ["", ...Array.from(types).sort()];
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Toolbar: filter + scan
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            Layout.topMargin: 12
            Layout.bottomMargin: 8
            spacing: 8

            MD.Text {
                text: "Wallpapers"
                typescale: MD.Token.typescale.title_large
                color: MD.Token.color.on_surface
            }

            Item { Layout.fillWidth: true }

            // Type filter chips
            Repeater {
                model: root.availableTypes

                MD.FilterChip {
                    required property string modelData
                    required property int index

                    text: modelData === "" ? "All" : modelData
                    checked: root.typeFilter === modelData
                    onClicked: root.typeFilter = modelData
                }
            }

            MD.IconButton {
                icon.name: MD.Token.icon.refresh
                onClicked: {
                    scanQuery.reload();
                    wallpaperQuery.reload();
                }
            }
        }

        // Wallpaper grid
        MD.Flickable {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentHeight: grid.implicitHeight + 32

            GridLayout {
                id: grid
                width: parent.width - 32
                x: 16
                y: 8
                columns: Math.max(1, Math.floor((root.width - 32) / 280))
                rowSpacing: 12
                columnSpacing: 12

                Repeater {
                    model: root.filteredWallpapers

                    WallpaperCard {
                        required property var modelData
                        required property int index

                        Layout.fillWidth: true
                        wallpaper: modelData
                    }
                }
            }

            // Empty state
            MD.Text {
                anchors.centerIn: parent
                visible: !root.filteredWallpapers || root.filteredWallpapers.length === 0
                text: wallpaperQuery.querying ? "Loading…" : "No wallpapers found"
                typescale: MD.Token.typescale.body_large
                color: MD.Token.color.on_surface_variant
            }
        }
    }
}
