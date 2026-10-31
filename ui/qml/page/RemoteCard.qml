pragma ComponentBehavior: Bound
import QtQuick
import QExtra as QE
import Qcm.Material as MD
import waywallen.ui as W

Item {
    id: root

    readonly property int _radius: Math.min(MD.Token.shape.corner.extra_small, root.spacing)
    readonly property int acquisitionState: item.acquisitionState
    readonly property real cardHeight: Math.min(root.itemHeight, root.height)
    readonly property real cardWidth: Math.min(root.itemWidth, root.width)
    property real fallbackAspectRatio: 1
    readonly property int index: row.index
    readonly property var item: row.value.item
    property real itemHeight: height
    property real itemWidth: width
    property bool masonry: false
    readonly property string previewUrl: item.previewUrl
    required property int remoteCapability
    required property MD.LazyRow row
    property real spacing: 12
    readonly property string title: item.title

    signal clicked

    implicitHeight: Math.max(64, width / m_thumb.aspectRatio)

    Item {
        id: m_card

        anchors.centerIn: parent
        height: root.cardHeight
        width: root.cardWidth

        Item {
            id: m_cell

            anchors.fill: parent
            clip: true

            W.ThumbnailImage {
                id: m_thumb

                anchors.fill: parent
                fallbackAspectRatio: root.fallbackAspectRatio
                fillMode: Image.PreserveAspectCrop
                loadingMode: !root.row.active || root.row.usage === MD.LazyRow.Pooled ? QE.Image.Suspended : root.row.usage === MD.LazyRow.Cached ? QE.Image.Prefetch : QE.Image.Automatic
                preparationScope: root.row.view
                radius: root._radius
                resourceSize: Qt.size(Number(root.item?.width ?? 0), Number(root.item?.height ?? 0))
                source: root.previewUrl
                verticalAlignment: Image.AlignVCenter
            }
            Rectangle {
                anchors.bottom: parent.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                height: Math.max(0, parent.height - m_title.y)
                radius: root._radius
                visible: height > 0

                gradient: Gradient {
                    GradientStop {
                        color: "transparent"
                        position: 0.0
                    }
                    GradientStop {
                        color: Qt.rgba(0, 0, 0, 0.65)
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
                text: root.title.length > 0 ? root.title : qsTr("Untitled")
                typescale: MD.Token.typescale.title_small
                wrapMode: Text.WordWrap
            }
            Rectangle {
                color: MD.Token.color.primary
                height: m_badge.implicitHeight + 6
                radius: height / 2
                visible: (root.remoteCapability === 1 && root.acquisitionState === 3) || (root.remoteCapability === 2 && root.acquisitionState === 2)
                width: m_badge.implicitWidth + 12

                anchors {
                    margins: 6
                    right: parent.right
                    top: parent.top
                }
                MD.Label {
                    id: m_badge

                    anchors.centerIn: parent
                    color: MD.Token.color.on_primary
                    text: root.remoteCapability === 2 ? qsTr("Subscribed") : qsTr("Downloaded")
                    typescale: MD.Token.typescale.label_small
                }
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor

                onClicked: root.clicked()
            }
        }
    }
}
