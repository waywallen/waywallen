pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui

MD.Card {
    id: root

    required property var wallpaper
    property bool expanded: false

    implicitHeight: expanded ? expandedCol.implicitHeight : 100

    Behavior on implicitHeight {
        NumberAnimation { duration: 200; easing.type: Easing.OutCubic }
    }

    WallpaperApplyQuery {
        id: applyQuery
    }

    MouseArea {
        anchors.fill: parent
        onClicked: root.expanded = !root.expanded
    }

    ColumnLayout {
        id: expandedCol
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 16
        spacing: 8

        // Preview thumbnail (if available)
        Image {
            Layout.fillWidth: true
            Layout.preferredHeight: visible ? 120 : 0
            visible: wallpaper.preview !== undefined && wallpaper.preview !== ""
            source: wallpaper.preview || ""
            fillMode: Image.PreserveAspectCrop
            clip: true
        }

        // Title + type
        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                MD.Text {
                    text: wallpaper.name || "Untitled"
                    typescale: MD.Token.typescale.title_small
                    color: MD.Token.color.on_surface
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                }

                MD.Text {
                    text: wallpaper.wpType || ""
                    typescale: MD.Token.typescale.label_medium
                    color: MD.Token.color.on_surface_variant
                }
            }

            MD.Text {
                visible: applyQuery.status === 3
                text: "✓"
                typescale: MD.Token.typescale.title_medium
                color: MD.Token.color.primary
            }
        }

        // Expanded: resource path + apply controls
        ColumnLayout {
            Layout.fillWidth: true
            visible: root.expanded
            spacing: 8

            MD.Text {
                text: wallpaper.resource || ""
                typescale: MD.Token.typescale.body_small
                color: MD.Token.color.on_surface_variant
                Layout.fillWidth: true
                wrapMode: Text.WrapAnywhere
            }

            // Apply controls
            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                ColumnLayout {
                    spacing: 4
                    MD.Text {
                        text: "Resolution"
                        typescale: MD.Token.typescale.label_small
                        color: MD.Token.color.on_surface_variant
                    }
                    RowLayout {
                        spacing: 4
                        MD.TextField {
                            id: widthField
                            Layout.preferredWidth: 70
                            text: "1920"
                            inputMethodHints: Qt.ImhDigitsOnly
                        }
                        MD.Text {
                            text: "×"
                            color: MD.Token.color.on_surface_variant
                        }
                        MD.TextField {
                            id: heightField
                            Layout.preferredWidth: 70
                            text: "1080"
                            inputMethodHints: Qt.ImhDigitsOnly
                        }
                    }
                }

                ColumnLayout {
                    spacing: 4
                    MD.Text {
                        text: "FPS"
                        typescale: MD.Token.typescale.label_small
                        color: MD.Token.color.on_surface_variant
                    }
                    MD.TextField {
                        id: fpsField
                        Layout.preferredWidth: 50
                        text: "30"
                        inputMethodHints: Qt.ImhDigitsOnly
                    }
                }

                Item { Layout.fillWidth: true }

                MD.Button {
                    text: applyQuery.querying ? "Applying…" : "Apply"
                    mdState.type: MD.Enum.BtFilled
                    enabled: !applyQuery.querying

                    onClicked: {
                        applyQuery.wallpaperId = wallpaper.id || "";
                        applyQuery.width = parseInt(widthField.text) || 1920;
                        applyQuery.height = parseInt(heightField.text) || 1080;
                        applyQuery.fps = parseInt(fpsField.text) || 30;
                        applyQuery.reload();
                    }
                }
            }

            Item { height: 8 }
        }
    }
}
