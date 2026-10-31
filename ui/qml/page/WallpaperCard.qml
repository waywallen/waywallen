pragma ValueTypeBehavior: Assertable
import QtQuick
import QExtra as QE
import Qcm.Material as MD
import waywallen.ui as W

Item {
    id: root

    readonly property int _baseRadius: Math.min(MD.Token.shape.corner.extra_small, root.spacing)
    readonly property int _radius: root.selected ? root._selectedRadius : root._baseRadius
    readonly property real _selectedInset: root._selectedRadius / 2
    readonly property int _selectedRadius: Math.min(MD.Token.shape.corner.large, root.spacing)
    readonly property real cardHeight: Math.min(root.itemHeight, root.height)
    readonly property real cardWidth: Math.min(root.itemWidth, root.width)
    property real fallbackAspectRatio: 1
    readonly property int index: row.index
    property real itemHeight: height
    property real itemWidth: width
    property bool masonry: false
    required property MD.LazyRow row
    property bool selected: false
    property real spacing: 12
    readonly property var wallpaper: row.value.item

    signal clicked(int modifiers)
    signal selectionRequested(int modifiers)

    focusPolicy: Qt.StrongFocus
    implicitHeight: Math.max(64, width / m_thumb.aspectRatio)

    Rectangle {
        anchors.fill: parent
        color: MD.Token.color.primary_container
        visible: root.selected
    }
    Item {
        id: m_card

        anchors.centerIn: parent
        height: root.cardHeight
        width: root.cardWidth

        Item {
            id: m_cell

            anchors.fill: parent
            anchors.margins: root.selected ? root._selectedInset : 0

            W.ThumbnailImage {
                id: m_thumb

                anchors.fill: parent
                fallbackAspectRatio: root.fallbackAspectRatio
                fillMode: Image.PreserveAspectCrop
                loadingMode: !root.row.active || root.row.usage === MD.LazyRow.Pooled ? QE.Image.Suspended : root.row.usage === MD.LazyRow.Cached ? QE.Image.Prefetch : QE.Image.Automatic
                preparationScope: root.row.view
                radius: root._radius
                resource: root.wallpaper?.resource ?? ""
                resourceSize: Qt.size(Number(root.wallpaper?.width ?? 0), Number(root.wallpaper?.height ?? 0))
                retainWhileLoading: true
                source: root.wallpaper?.preview ?? ""
                wpType: root.wallpaper?.wpType ?? ""
            }

            // Scrim aligns to the image control's bounds; spans the
            // title-top → image-bottom overlap.
            Rectangle {
                anchors.bottom: m_thumb.bottom
                anchors.left: m_thumb.left
                anchors.right: m_thumb.right
                height: Math.max(0, m_thumb.height - m_title.y)
                radius: root._radius
                visible: height > 0

                gradient: Gradient {
                    GradientStop {
                        color: "transparent"
                        position: 0.0
                    }
                    GradientStop {
                        color: Qt.rgba(0, 0, 0, 0.6)
                        position: 1.0
                    }
                }
            }
            MD.Text {
                id: m_title

                anchors.bottom: parent.bottom
                anchors.bottomMargin: 6
                anchors.left: parent.left
                anchors.right: parent.right
                color: "white"
                elide: Text.ElideRight
                horizontalAlignment: Text.AlignHCenter
                leftPadding: 8
                maximumLineCount: 2
                rightPadding: 8
                text: root.wallpaper?.name || qsTr("Untitled")
                typescale: MD.Token.typescale.title_small
                wrapMode: Text.WordWrap
            }
            Rectangle {
                anchors.margins: 6
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.topMargin: root.wallpaper?.hidden ? 42 : 6
                color: MD.Token.color.tertiary_container
                height: 24
                radius: 12
                visible: root.wallpaper?.updateAvailable ?? false
                width: Math.min(Math.ceil(updateMetrics.advanceWidth) + 16, parent.width - (root.selected ? 44 : 12))

                TextMetrics {
                    id: updateMetrics

                    font: updateLabel.font
                    text: updateLabel.text
                }
                MD.Text {
                    id: updateLabel

                    anchors.fill: parent
                    anchors.leftMargin: 8
                    anchors.rightMargin: 8
                    color: MD.Token.color.on_tertiary_container
                    elide: Text.ElideRight
                    maximumLineCount: 1
                    text: qsTr("Update available")
                    typescale: MD.Token.typescale.label_small
                    verticalAlignment: Text.AlignVCenter
                }
            }
            MouseArea {
                property bool selectionRequestedByHold: false

                acceptedButtons: Qt.LeftButton | Qt.RightButton
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor

                onCanceled: selectionRequestedByHold = false
                onClicked: mouse => {
                    if (selectionRequestedByHold) {
                        selectionRequestedByHold = false;
                        return;
                    }
                    if (mouse.button === Qt.RightButton) {
                        root.selectionRequested(mouse.modifiers);
                        return;
                    }
                    root.clicked(mouse.modifiers);
                }
                onPressAndHold: mouse => {
                    if (mouse.button !== Qt.LeftButton)
                        return;
                    selectionRequestedByHold = true;
                    root.selectionRequested(mouse.modifiers);
                }
                onPressed: selectionRequestedByHold = false
            }
        }
    }
    Rectangle {
        anchors.left: m_card.left
        anchors.margins: 8
        anchors.top: m_card.top
        border.color: MD.Token.color.primary_container
        border.width: 3
        color: MD.Token.color.primary
        height: 32
        radius: width / 2
        visible: root.selected
        width: 32

        MD.Icon {
            anchors.centerIn: parent
            color: MD.Token.color.on_primary
            name: MD.Token.icon.check
            size: 20
        }
    }
    Rectangle {
        anchors.margins: 8
        anchors.right: m_card.right
        anchors.top: m_card.top
        border.color: MD.Token.color.secondary
        border.width: 3
        color: MD.Token.color.secondary_container
        height: 32
        radius: width / 2
        visible: !!(root.wallpaper?.hidden)
        width: 32

        MD.Icon {
            anchors.centerIn: parent
            color: MD.Token.color.on_secondary_container
            name: MD.Token.icon.visibility_off
            size: 20
        }
    }
}
