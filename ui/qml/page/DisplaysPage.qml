pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root

    title: 'Displays'
    showHeader: true
    showBackground: false
    readonly property real displayGapPx: 80

    property var selectedId: null

    function layoutRects() {
        const out = [];
        let x = 0;
        for (const d of W.App.displayManager.displays || []) {
            out.push({
                x: x,
                y: 0,
                w: d.width,
                h: d.height,
                d: d
            });
            x += d.width + root.displayGapPx;
        }
        return out;
    }

    readonly property var rects: layoutRects()

    readonly property real boundsW: {
        let max = 0;
        for (const r of rects)
            max = Math.max(max, r.x + r.w);
        return max || 1;
    }
    readonly property real boundsH: {
        let max = 0;
        for (const r of rects)
            max = Math.max(max, r.y + r.h);
        return max || 1;
    }

    function selectedDisplay() {
        if (root.selectedId === null)
            return null;
        for (const d of W.App.displayManager.displays || []) {
            if (d.id === root.selectedId)
                return d;
        }
        return null;
    }

    readonly property var selected: selectedDisplay()

    ColumnLayout {
        anchors.fill: parent
        anchors.leftMargin: 12
        anchors.rightMargin: 12
        spacing: 16

        MD.Pane {
            id: displaysPane
            Layout.fillWidth: true
            Layout.fillHeight: true
            leftPadding: 16
            rightPadding: 16
            radius: 16
            backgroundColor: MD.MProp.color.surface

            contentItem: Item {
                id: canvas
                implicitHeight: 48

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
                    text: "No displays registered"
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
                                strokeColor: rectItem.isSelected ? MD.Token.color.primary : MD.Token.color.outline
                                strokeWidth: rectItem.isSelected ? 3 : 1.5
                                fillColor: rectItem.hasLink ? MD.Token.color.primary_container : MD.Token.color.surface_container_highest
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
                                color: rectItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface
                            }

                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                text: rectItem.d.width + " × " + rectItem.d.height
                                typescale: MD.Token.typescale.label_medium
                                color: rectItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface_variant
                            }
                        }

                        MD.Text {
                            anchors.left: parent.left
                            anchors.top: parent.top
                            anchors.margins: 6
                            text: "#" + rectItem.d.id
                            typescale: MD.Token.typescale.label_small
                            color: rectItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface_variant
                        }
                    }
                }
            }
        }

        // --- Inline details panel (squeezes out below canvas) ---
        MD.Pane {
            id: detailsPane
            Layout.fillWidth: true
            Layout.preferredHeight: root.selected ? implicitHeight : 0

            leftPadding: 16
            rightPadding: 16
            bottomPadding: 12

            radius: 16
            backgroundColor: MD.MProp.color.surface
            visible: Layout.preferredHeight > 0.5
            clip: true

            Behavior on Layout.preferredHeight {
                NumberAnimation {
                    duration: 200
                    easing.type: Easing.InOutCubic
                }
            }

            contentItem: ColumnLayout {
                id: detailsContent
                spacing: 8

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    MD.Text {
                        Layout.fillWidth: true
                        text: root.selected ? (root.selected.name || ("Display " + root.selected.id)) : ""
                        typescale: MD.Token.typescale.title_medium
                        color: MD.Token.color.on_surface
                        elide: Text.ElideRight
                    }

                    MD.IconButton {
                        icon.name: MD.Token.icon.close
                        onClicked: root.selectedId = null
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 24

                    RowLayout {
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
                        spacing: 8
                        MD.Text {
                            text: "Size:"
                            typescale: MD.Token.typescale.label_medium
                            color: MD.Token.color.on_surface_variant
                        }
                        MD.Text {
                            text: root.selected ? root.selected.width + " × " + root.selected.height : ""
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
                            text: root.selected ? (root.selected.refreshMhz / 1000).toFixed(3) + " Hz" : ""
                            typescale: MD.Token.typescale.body_medium
                            color: MD.Token.color.on_surface
                        }
                    }

                    Item {
                        Layout.fillWidth: true
                    }
                }

                MD.Divider {
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    Layout.bottomMargin: 4
                }

                MD.Text {
                    text: "Bindings"
                    typescale: MD.Token.typescale.title_small
                    color: MD.Token.color.on_surface
                }

                MD.Text {
                    Layout.fillWidth: true
                    visible: !!root.selected && (!root.selected.links || root.selected.links.length === 0)
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
            }
        }
        Item {}
    }
}
