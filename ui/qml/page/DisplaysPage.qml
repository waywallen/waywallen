pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import Qcm.Material as MD
import waywallen.ui

MD.Page {
    id: root

    readonly property real displayGapPx: 80

    property var selectedId: null

    DisplayListQuery {
        id: displayQuery
        Component.onCompleted: reload()
    }

    function layoutRects() {
        const out = [];
        let x = 0;
        for (const d of DisplayManager.displays || []) {
            out.push({ x: x, y: 0, w: d.width, h: d.height, d: d });
            x += d.width + root.displayGapPx;
        }
        return out;
    }

    readonly property var rects: layoutRects()

    readonly property real boundsW: {
        let max = 0;
        for (const r of rects) max = Math.max(max, r.x + r.w);
        return max || 1;
    }
    readonly property real boundsH: {
        let max = 0;
        for (const r of rects) max = Math.max(max, r.y + r.h);
        return max || 1;
    }

    function selectedDisplay() {
        if (root.selectedId === null) return null;
        for (const d of DisplayManager.displays || []) {
            if (d.id === root.selectedId) return d;
        }
        return null;
    }

    readonly property var selected: selectedDisplay()

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            Layout.topMargin: 12
            Layout.bottomMargin: 8
            spacing: 8

            MD.Text {
                text: "Displays"
                typescale: MD.Token.typescale.title_large
                color: MD.Token.color.on_surface
            }

            Item { Layout.fillWidth: true }

            Item {
                Layout.preferredWidth: 40
                Layout.preferredHeight: 40

                MD.IconButton {
                    anchors.fill: parent
                    icon.name: MD.Token.icon.refresh
                    enabled: !displayQuery.querying
                    opacity: displayQuery.querying ? 0.0 : 1.0
                    onClicked: displayQuery.reload()
                }

                MD.CircularIndicator {
                    anchors.centerIn: parent
                    width: 24
                    height: 24
                    visible: displayQuery.querying
                    running: displayQuery.querying
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 12

            Item {
                id: canvas
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.leftMargin: 16
                Layout.bottomMargin: 16

                readonly property real padding: 24
                readonly property real viewScale: {
                    const availW = Math.max(1, width - padding * 2);
                    const availH = Math.max(1, height - padding * 2);
                    return Math.min(availW / root.boundsW, availH / root.boundsH);
                }
                readonly property real offsetX: (width - root.boundsW * viewScale) / 2
                readonly property real offsetY: (height - root.boundsH * viewScale) / 2

                MouseArea {
                    anchors.fill: parent
                    onClicked: root.selectedId = null
                }

                MD.Text {
                    anchors.centerIn: parent
                    visible: (root.rects.length === 0)
                    text: displayQuery.querying ? "Loading…" : "No displays registered"
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                }

                Repeater {
                    model: root.rects

                    delegate: Item {
                        id: rectItem
                        required property int index
                        required property var modelData

                        readonly property var d: modelData.d
                        readonly property bool hasLink: (d.links && d.links.length > 0)
                        readonly property bool isSelected: (root.selectedId === d.id)

                        x: canvas.offsetX + modelData.x * canvas.viewScale
                        y: canvas.offsetY + modelData.y * canvas.viewScale
                        width: modelData.w * canvas.viewScale
                        height: modelData.h * canvas.viewScale

                        Shape {
                            anchors.fill: parent
                            preferredRendererType: Shape.CurveRenderer
                            antialiasing: true

                            ShapePath {
                                strokeColor: rectItem.isSelected
                                             ? MD.Token.color.primary
                                             : MD.Token.color.outline
                                strokeWidth: rectItem.isSelected ? 3 : 1.5
                                fillColor: rectItem.hasLink
                                           ? MD.Token.color.primary_container
                                           : MD.Token.color.surface_container_highest
                                capStyle: ShapePath.RoundCap
                                joinStyle: ShapePath.RoundJoin

                                PathRectangle {
                                    x: 0
                                    y: 0
                                    width: rectItem.width
                                    height: rectItem.height
                                    radius: 10
                                }
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.selectedId = rectItem.d.id
                        }

                        ColumnLayout {
                            anchors.centerIn: parent
                            spacing: 4

                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                text: rectItem.d.name || ("Display " + rectItem.d.id)
                                typescale: MD.Token.typescale.title_small
                                color: rectItem.hasLink
                                       ? MD.Token.color.on_primary_container
                                       : MD.Token.color.on_surface
                            }

                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                text: rectItem.d.width + " × " + rectItem.d.height
                                typescale: MD.Token.typescale.label_medium
                                color: rectItem.hasLink
                                       ? MD.Token.color.on_primary_container
                                       : MD.Token.color.on_surface_variant
                            }
                        }

                        MD.Text {
                            anchors.left: parent.left
                            anchors.top: parent.top
                            anchors.margins: 6
                            text: "#" + rectItem.d.id
                            typescale: MD.Token.typescale.label_small
                            color: rectItem.hasLink
                                   ? MD.Token.color.on_primary_container
                                   : MD.Token.color.on_surface_variant
                        }
                    }
                }
            }

            // --- Details panel ---
            MD.Card {
                Layout.preferredWidth: 280
                Layout.fillHeight: true
                Layout.rightMargin: 16
                Layout.bottomMargin: 16
                type: MD.Enum.CardFilled

                contentItem: ColumnLayout {
                    spacing: 8

                    MD.Text {
                        Layout.fillWidth: true
                        text: root.selected ? (root.selected.name || ("Display " + root.selected.id))
                                            : "No selection"
                        typescale: MD.Token.typescale.title_medium
                        color: MD.Token.color.on_surface
                        wrapMode: Text.WordWrap
                    }

                    MD.Text {
                        Layout.fillWidth: true
                        visible: !root.selected
                        text: "Click a display to see its bindings."
                        typescale: MD.Token.typescale.body_small
                        color: MD.Token.color.on_surface_variant
                        wrapMode: Text.WordWrap
                    }

                    RowLayout {
                        visible: !!root.selected
                        spacing: 8
                        MD.Text {
                            text: "ID:"
                            typescale: MD.Token.typescale.label_medium
                            color: MD.Token.color.on_surface_variant
                        }
                        MD.Text {
                            text: root.selected ? "#" + root.selected.id : ""
                            typescale: MD.Token.typescale.body_medium
                            color: MD.Token.color.on_surface
                        }
                    }

                    RowLayout {
                        visible: !!root.selected
                        spacing: 8
                        MD.Text {
                            text: "Size:"
                            typescale: MD.Token.typescale.label_medium
                            color: MD.Token.color.on_surface_variant
                        }
                        MD.Text {
                            text: root.selected
                                  ? root.selected.width + " × " + root.selected.height
                                  : ""
                            typescale: MD.Token.typescale.body_medium
                            color: MD.Token.color.on_surface
                        }
                    }

                    RowLayout {
                        visible: !!root.selected && root.selected.refreshMhz > 0
                        spacing: 8
                        MD.Text {
                            text: "Refresh:"
                            typescale: MD.Token.typescale.label_medium
                            color: MD.Token.color.on_surface_variant
                        }
                        MD.Text {
                            text: root.selected
                                  ? (root.selected.refreshMhz / 1000).toFixed(3) + " Hz"
                                  : ""
                            typescale: MD.Token.typescale.body_medium
                            color: MD.Token.color.on_surface
                        }
                    }

                    MD.Divider {
                        Layout.fillWidth: true
                        Layout.topMargin: 4
                        Layout.bottomMargin: 4
                        visible: !!root.selected
                    }

                    MD.Text {
                        visible: !!root.selected
                        text: "Bindings"
                        typescale: MD.Token.typescale.title_small
                        color: MD.Token.color.on_surface
                    }

                    MD.Text {
                        Layout.fillWidth: true
                        visible: !!root.selected
                                 && (!root.selected.links || root.selected.links.length === 0)
                        text: "Idle — no renderer bound."
                        typescale: MD.Token.typescale.body_small
                        color: MD.Token.color.on_surface_variant
                        wrapMode: Text.WordWrap
                    }

                    Repeater {
                        model: root.selected ? root.selected.links : []
                        delegate: RowLayout {
                            required property var modelData
                            Layout.fillWidth: true
                            spacing: 8

                            MD.Icon {
                                name: MD.Token.icon.play_arrow
                                size: 18
                                color: MD.Token.color.primary
                            }
                            MD.Text {
                                Layout.fillWidth: true
                                text: modelData.rendererId
                                typescale: MD.Token.typescale.body_small
                                color: MD.Token.color.on_surface
                                font.family: "monospace"
                                elide: Text.ElideMiddle
                            }
                            MD.Text {
                                text: "z=" + modelData.zOrder
                                typescale: MD.Token.typescale.label_small
                                color: MD.Token.color.on_surface_variant
                            }
                        }
                    }

                    Item { Layout.fillHeight: true }
                }
            }
        }
    }
}
