pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui

MD.Page {
    id: root

    WallpaperListQuery {
        id: wallpaperQuery
        Component.onCompleted: reload()
    }

    WallpaperScanQuery {
        id: scanQuery
    }

    WallpaperApplyQuery {
        id: applyQuery
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

    showBackground: false

    contentItem: RowLayout {
        spacing: 12

        // --- Left: wallpaper grid ---
        MD.Pane {
            Layout.fillWidth: true
            Layout.fillHeight: true
            radius: MD.Token.shape.corner.large
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

                    Item {
                        Layout.fillWidth: true
                    }

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

                // Grid via ListView + WidthProvider
                MD.VerticalListView {
                    id: m_grid_view
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    cacheBuffer: 300
                    displayMarginBeginning: 300
                    displayMarginEnd: 300
                    topMargin: 8
                    bottomMargin: 8

                    MD.WidthProvider {
                        id: m_wp
                        total: m_grid_view.width
                        minimum: 150
                        spacing: 12
                        leftMargin: 8
                        rightMargin: 8
                    }

                    model: root.filteredWallpapers

                    delegate: WallpaperCard {
                        widthProvider: m_wp
                        onClicked: root.selectedWallpaper = wallpaper
                    }
                }

                // Empty state
                MD.Text {
                    Layout.alignment: Qt.AlignCenter
                    visible: !root.filteredWallpapers || root.filteredWallpapers.length === 0
                    text: wallpaperQuery.querying ? "Loading…" : "No wallpapers found"
                    typescale: MD.Token.typescale.body_large
                    color: MD.Token.color.on_surface_variant
                }
            }
        }

        // --- Right: wallpaper detail panel ---
        MD.Pane {
            Layout.preferredWidth: 280
            Layout.fillHeight: true
            Layout.maximumWidth: 280
            visible: root.selectedWallpaper !== null
            radius: MD.Token.shape.corner.large
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

                        // Resource path — single line, elide head so the
                        // meaningful tail (workshop id / filename) stays visible.
                        MD.Text {
                            Layout.fillWidth: true
                            text: root.selectedWallpaper?.resource || ""
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            elide: Text.ElideLeft
                            maximumLineCount: 1
                            wrapMode: Text.NoWrap
                        }

                        MD.Divider {
                            Layout.fillWidth: true
                        }

                        // Resolution
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 4

                            MD.Text {
                                text: "Resolution"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }
                            RowLayout {
                                spacing: 8
                                MD.TextField {
                                    id: widthField
                                    Layout.preferredWidth: 80
                                    text: "1920"
                                    inputMethodHints: Qt.ImhDigitsOnly
                                }
                                MD.Text {
                                    text: "×"
                                    color: MD.Token.color.on_surface_variant
                                }
                                MD.TextField {
                                    id: heightField
                                    Layout.preferredWidth: 80
                                    text: "1080"
                                    inputMethodHints: Qt.ImhDigitsOnly
                                }
                            }
                        }

                        // FPS
                        ColumnLayout {
                            spacing: 4

                            MD.Text {
                                text: "FPS"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }
                            MD.TextField {
                                id: fpsField
                                Layout.preferredWidth: 80
                                text: "30"
                                inputMethodHints: Qt.ImhDigitsOnly
                            }
                        }

                        // Apply button
                        MD.Button {
                            Layout.fillWidth: true
                            text: applyQuery.querying ? "Applying…" : "Apply"
                            mdState.type: MD.Enum.BtFilled
                            enabled: !applyQuery.querying

                            onClicked: {
                                applyQuery.wallpaperId = root.selectedWallpaper?.id || "";
                                applyQuery.width = parseInt(widthField.text) || 1920;
                                applyQuery.height = parseInt(heightField.text) || 1080;
                                applyQuery.fps = parseInt(fpsField.text) || 30;
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
