pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root

    W.WallpaperListQuery {
        id: wallpaperQuery
        Component.onCompleted: reload()
    }

    W.WallpaperScanQuery {
        id: scanQuery
    }

    W.WallpaperApplyQuery {
        id: applyQuery
    }

    W.LibraryAutoDetectQuery {
        id: autoDetectQuery
        onStatusChanged: {
            if (status === 3) {
                scanQuery.reload();
                wallpaperQuery.reload();
            }
        }
    }

    property string typeFilter: ""
    property var filteredWallpapers: {
        const all = wallpaperQuery.wallpapers;
        if (!all || typeFilter === "")
            return all;
        return all.filter(wp => wp.wpType === typeFilter);
    }

    property var availableTypes: {
        const all = wallpaperQuery.wallpapers;
        if (!all)
            return [];
        const types = new Set();
        for (const wp of all)
            if (wp.wpType)
                types.add(wp.wpType);
        return ["", ...Array.from(types).sort()];
    }

    property var selectedWallpaper: null

    // Target display ids for Apply. Empty set = "All displays".
    property var applyTargetIds: []
    function isTargetAll() {
        return applyTargetIds.length === 0;
    }
    function toggleTarget(id) {
        const next = applyTargetIds.slice();
        const i = next.indexOf(id);
        if (i >= 0)
            next.splice(i, 1);
        else
            next.push(id);
        applyTargetIds = next;
    }
    showBackground: false
    padding: MD.MProp.size.isCompact ? 0 : 12

    contentItem: RowLayout {
        spacing: 12

        // --- Left: wallpaper grid ---
        MD.Pane {
            Layout.fillWidth: true
            Layout.fillHeight: true
            radius: root.MD.MProp.page.backgroundRadius
            padding: 0
            showBackground: true

            contentItem: ColumnLayout {
                spacing: 0

                // Toolbar
                RowLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    Layout.topMargin: 12
                    spacing: 8

                    MD.Text {
                        text: "Wallpapers"
                        typescale: MD.Token.typescale.title_large
                        color: MD.Token.color.on_surface
                    }

                    // Repeater {
                    //     model: root.availableTypes

                    //     MD.FilterChip {
                    //         required property string modelData
                    //         required property int index

                    //         text: modelData === "" ? "All" : modelData
                    //         checked: root.typeFilter === modelData
                    //         onClicked: root.typeFilter = modelData
                    //     }
                    // }

                    MD.ActionToolBar {
                        Layout.fillWidth: true
                        actions: [
                            MD.Action {
                                icon.name: MD.Token.icon.filter_list
                                text: 'Filters'
                            },
                            MD.Action {
                                icon.name: MD.Token.icon.hard_drive
                                text: 'Sources'
                                onTriggered: MD.Util.showPopup('waywallen.ui/PagePopup', {
                                    source: 'waywallen.ui/SourceManagePage'
                                }, win)
                            },
                            MD.Action {
                                icon.name: MD.Token.icon.refresh
                                text: 'Refresh'
                                onTriggered: {
                                    scanQuery.reload();
                                    wallpaperQuery.reload();
                                }
                            }
                        ]
                    }
                }

                // Grid + centered empty-state overlay
                Item {
                    Layout.fillWidth: true
                    Layout.fillHeight: true

                    MD.VerticalListView {
                        id: m_grid_view
                        anchors.fill: parent
                        clip: true
                        cacheBuffer: 300
                        displayMarginBeginning: 300
                        displayMarginEnd: 300
                        topMargin: 8
                        bottomMargin: 8
                        visible: root.filteredWallpapers && root.filteredWallpapers.length > 0

                        MD.WidthProvider {
                            id: m_wp
                            total: m_grid_view.width
                            minimum: 150
                            spacing: 12
                            leftMargin: 8
                            rightMargin: 8
                        }

                        model: wallpaperQuery.model

                        delegate: WallpaperCard {
                            widthProvider: m_wp
                            onClicked: root.selectedWallpaper = wallpaperQuery.model.item(index)
                        }
                    }

                    ColumnLayout {
                        anchors.centerIn: parent
                        spacing: 16
                        visible: !root.filteredWallpapers || root.filteredWallpapers.length === 0

                        MD.CircularIndicator {
                            Layout.alignment: Qt.AlignHCenter
                            visible: wallpaperQuery.querying
                            running: visible
                        }

                        MD.Text {
                            Layout.alignment: Qt.AlignHCenter
                            visible: !wallpaperQuery.querying
                            text: "No wallpapers found"
                            typescale: MD.Token.typescale.body_large
                            color: MD.Token.color.on_surface_variant
                        }

                        MD.BusyButton {
                            Layout.alignment: Qt.AlignHCenter
                            visible: !wallpaperQuery.querying
                            text: "Auto detect libraries"
                            busy: autoDetectQuery.querying
                            mdState.type: MD.Enum.BtFilledTonal
                            onClicked: {
                                if (!busy) autoDetectQuery.reload();
                            }
                        }
                    }
                }
            }
        }

        // --- Right: wallpaper detail panel ---
        MD.Pane {
            Layout.preferredWidth: 280
            Layout.fillHeight: true
            Layout.maximumWidth: 280
            visible: root.selectedWallpaper !== null
            radius: root.MD.MProp.page.backgroundRadius
            padding: 0
            showBackground: true

            contentItem: MD.Flickable {
                id: m_detail_flick
                contentHeight: m_detail_col.implicitHeight

                ColumnLayout {
                    id: m_detail_col
                    width: m_detail_flick.width
                    spacing: 0

                    // Preview
                    Image {
                        Layout.fillWidth: true
                        Layout.preferredHeight: visible ? 200 : 0
                        Layout.margins: 12
                        visible: root.selectedWallpaper?.preview !== undefined && root.selectedWallpaper?.preview !== ""
                        source: root.selectedWallpaper?.preview ? "file://" + root.selectedWallpaper.preview : ""
                        fillMode: Image.PreserveAspectFit
                    }

                    // Info section
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.leftMargin: 16
                        Layout.rightMargin: 16
                        Layout.bottomMargin: 16
                        spacing: 12

                        // Close button row
                        RowLayout {
                            Layout.fillWidth: true

                            MD.Text {
                                Layout.fillWidth: true
                                text: root.selectedWallpaper?.name || "Untitled"
                                typescale: MD.Token.typescale.title_large
                                color: MD.Token.color.on_surface
                                wrapMode: Text.Wrap
                                maximumLineCount: 2
                                elide: Text.ElideRight
                            }

                            MD.IconButton {
                                icon.name: MD.Token.icon.close
                                onClicked: root.selectedWallpaper = null
                            }
                        }

                        // Type
                        MD.Text {
                            text: root.selectedWallpaper?.wpType || ""
                            typescale: MD.Token.typescale.label_large
                            color: MD.Token.color.on_surface_variant
                        }

                        // Resource path — show only the last two segments
                        // (parent dir + filename) under a "Path" label.
                        // Full path is exposed via the tooltip / hover.
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2

                            function shortPath(p) {
                                const parts = (p || "").split("/").filter(s => s.length > 0);
                                return parts.slice(-2).join("/");
                            }

                            MD.Text {
                                text: "Path"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }
                            MD.Text {
                                Layout.fillWidth: true
                                text: parent.shortPath(root.selectedWallpaper?.resource)
                                typescale: MD.Token.typescale.body_small
                                color: MD.Token.color.on_surface_variant
                                elide: Text.ElideMiddle
                                maximumLineCount: 1
                                wrapMode: Text.NoWrap
                            }
                        }

                        // Media meta block: resolution / size / format.
                        // Hidden entirely when all three values are unknown.
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 4

                            readonly property bool hasResolution: (root.selectedWallpaper?.width ?? 0) !== 0 && (root.selectedWallpaper?.height ?? 0) !== 0
                            readonly property bool hasSize: (root.selectedWallpaper?.size ?? 0) !== 0
                            readonly property bool hasFormat: (root.selectedWallpaper?.format ?? "") !== ""
                            visible: hasResolution || hasSize || hasFormat

                            function formatSize(b) {
                                if (b <= 0) return "";
                                const u = ["B", "KB", "MB", "GB", "TB"];
                                let i = 0;
                                let v = b;
                                while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
                                return v.toFixed(i === 0 ? 0 : 1) + " " + u[i];
                            }

                            MD.Text {
                                text: "Media"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }

                            GridLayout {
                                Layout.fillWidth: true
                                columns: 2
                                columnSpacing: 12
                                rowSpacing: 2

                                // Resolution row
                                MD.Text {
                                    visible: parent.parent.hasResolution
                                    text: "Resolution"
                                    typescale: MD.Token.typescale.label_medium
                                    color: MD.Token.color.on_surface_variant
                                }
                                MD.Text {
                                    visible: parent.parent.hasResolution
                                    text: (root.selectedWallpaper?.width ?? 0) + "×" + (root.selectedWallpaper?.height ?? 0)
                                    typescale: MD.Token.typescale.body_medium
                                    color: MD.Token.color.on_surface
                                }

                                // Size row
                                MD.Text {
                                    visible: parent.parent.hasSize
                                    text: "Size"
                                    typescale: MD.Token.typescale.label_medium
                                    color: MD.Token.color.on_surface_variant
                                }
                                MD.Text {
                                    visible: parent.parent.hasSize
                                    text: parent.parent.formatSize(root.selectedWallpaper?.size ?? 0)
                                    typescale: MD.Token.typescale.body_medium
                                    color: MD.Token.color.on_surface
                                }

                                // Format row
                                MD.Text {
                                    visible: parent.parent.hasFormat
                                    text: "Format"
                                    typescale: MD.Token.typescale.label_medium
                                    color: MD.Token.color.on_surface_variant
                                }
                                MD.Text {
                                    visible: parent.parent.hasFormat
                                    text: (root.selectedWallpaper?.format ?? "").toLowerCase()
                                    typescale: MD.Token.typescale.body_medium
                                    color: MD.Token.color.on_surface
                                }
                            }
                        }

                        MD.Divider {
                            Layout.fillWidth: true
                        }

                        // Apply target — chip row over DisplayManager.displays
                        // plus a leading "All" chip. Multi-select; empty
                        // selection ⇒ "All" (applied to every display).
                        // Resolution / FPS are resolved daemon-side from
                        // plugin settings, not configured here.
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 4

                            MD.Text {
                                text: "Apply to"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }

                            Flow {
                                Layout.fillWidth: true
                                spacing: 6

                                MD.FilterChip {
                                    text: "All"
                                    checked: root.isTargetAll()
                                    onClicked: root.applyTargetIds = []
                                }

                                Repeater {
                                    model: W.App.displayManager.displays

                                    MD.FilterChip {
                                        required property var modelData
                                        text: modelData?.name || ("Display " + modelData?.id)
                                        checked: root.applyTargetIds.indexOf(modelData?.id) >= 0
                                        onClicked: root.toggleTarget(modelData?.id)
                                    }
                                }
                            }
                        }

                        // Apply button
                        MD.BusyButton {
                            Layout.fillWidth: true
                            text: "Apply"
                            busy: applyQuery.querying
                            mdState.type: MD.Enum.BtFilled

                            onClicked: {
                                if (busy)
                                    return;

                                applyQuery.wallpaperId = root.selectedWallpaper?.id_proto || "";
                                applyQuery.displayIds = root.applyTargetIds;
                                applyQuery.reload();
                            }
                        }

                        // Status
                        RowLayout {
                            visible: applyQuery.status === 3
                            spacing: 8

                            MD.Icon {
                                name: MD.Token.icon.check
                                size: 20
                                color: MD.Token.color.primary
                            }
                            MD.Text {
                                text: "Applied"
                                typescale: MD.Token.typescale.label_large
                                color: MD.Token.color.primary
                            }
                        }
                    }
                }
            }
        }
    }
}
