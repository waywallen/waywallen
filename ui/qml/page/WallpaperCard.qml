pragma ValueTypeBehavior: Assertable
import QtQuick
import Qcm.Material as MD
import waywallen.ui as W

Item {
    id: root

    required property var model
    required property int index
    property var wallpaper: model
    property bool selected: false
    property real itemWidth: width
    property real itemHeight: height

    width: GridView.view ? GridView.view.cellWidth : 0
    height: GridView.view ? GridView.view.cellHeight : 0

    focusPolicy: Qt.StrongFocus

    signal clicked(int modifiers)
    signal selectionRequested(int modifiers)

    readonly property int _baseRadius: MD.Token.shape.corner.extra_small
    readonly property int _selectedRadius: MD.Token.shape.corner.large
    readonly property int _radius: root.selected ? root._selectedRadius : root._baseRadius
    readonly property real _selectedInset: root._selectedRadius / 2
    readonly property real cardWidth: Math.min(root.itemWidth, root.width)
    readonly property real cardHeight: Math.min(root.itemHeight, root.height)

    Rectangle {
        anchors.fill: parent
        visible: root.selected
        color: MD.Token.color.primary_container
    }

    Item {
        id: m_card
        width: root.cardWidth
        height: root.cardHeight
        anchors.centerIn: parent

        Item {
            id: m_cell
            anchors.fill: parent
            anchors.margins: 6 + (root.selected ? root._selectedInset : 0)

            W.ThumbnailImage {
                id: m_thumb
                retainWhileLoading: true
                anchors.fill: parent
                source: root.wallpaper?.preview ?? ""
                resource: root.wallpaper?.resource ?? ""
                wpType: root.wallpaper?.wpType ?? ""
                fillMode: Image.PreserveAspectCrop
                radius: root._radius
            }

            // Scrim aligns to the image control's bounds; spans the
            // title-top → image-bottom overlap.
            Rectangle {
                anchors.left: m_thumb.left
                anchors.right: m_thumb.right
                anchors.bottom: m_thumb.bottom
                height: Math.max(0, m_thumb.height - m_title.y)
                visible: height > 0
                radius: root._radius
                gradient: Gradient {
                    GradientStop {
                        position: 0.0
                        color: "transparent"
                    }
                    GradientStop {
                        position: 1.0
                        color: Qt.rgba(0, 0, 0, 0.6)
                    }
                }
            }

            MD.Text {
                id: m_title
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 6
                text: root.wallpaper?.name || qsTr("Untitled")
                typescale: MD.Token.typescale.title_small
                color: "white"
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                elide: Text.ElideRight
                maximumLineCount: 2
                leftPadding: 8
                rightPadding: 8
            }

            Rectangle {
                anchors.top: parent.top
                anchors.right: parent.right
                anchors.margins: 6
                anchors.topMargin: root.wallpaper?.hidden ? 42 : 6
                width: Math.min(Math.ceil(updateMetrics.advanceWidth) + 16, parent.width - (root.selected ? 44 : 12))
                height: 24
                radius: 12
                visible: root.wallpaper?.updateAvailable ?? false
                color: MD.Token.color.tertiary_container

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
                    text: qsTr("Update available")
                    typescale: MD.Token.typescale.label_small
                    color: MD.Token.color.on_tertiary_container
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }
            }

            MouseArea {
                property bool selectionRequestedByHold: false

                anchors.fill: parent
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                cursorShape: Qt.PointingHandCursor
                onPressed: selectionRequestedByHold = false
                onCanceled: selectionRequestedByHold = false
                onPressAndHold: mouse => {
                    if (mouse.button !== Qt.LeftButton)
                        return;
                    selectionRequestedByHold = true;
                    root.selectionRequested(mouse.modifiers);
                }
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
            }
        }
    }

    Rectangle {
        anchors.top: m_card.top
        anchors.left: m_card.left
        anchors.margins: 8
        width: 32
        height: 32
        radius: width / 2
        visible: root.selected
        color: MD.Token.color.primary
        border.color: MD.Token.color.primary_container
        border.width: 3

        MD.Icon {
            anchors.centerIn: parent
            name: MD.Token.icon.check
            size: 20
            color: MD.Token.color.on_primary
        }
    }

    Rectangle {
        anchors.top: m_card.top
        anchors.right: m_card.right
        anchors.margins: 8
        width: 32
        height: 32
        radius: width / 2
        visible: !!(root.wallpaper?.hidden)
        color: MD.Token.color.secondary_container
        border.color: MD.Token.color.secondary
        border.width: 3

        MD.Icon {
            anchors.centerIn: parent
            name: MD.Token.icon.visibility_off
            size: 20
            color: MD.Token.color.on_secondary_container
        }
    }
}
