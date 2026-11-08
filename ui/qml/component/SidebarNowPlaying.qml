pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import Qcm.Material as MD
import waywallen.ui as W

Item {
    id: root

    required property var model
    required property bool expanded
    property real expansionProgress: expanded ? 1 : 0
    readonly property real horizontalInset: 12
    readonly property real cardRadius: MD.Token.shape.corner.medium
    readonly property bool pointerHovered: carouselHover.hovered
    signal openRequested(string wallpaperId)

    implicitHeight: visible ? carousel.implicitHeight + (root.model?.count > 1 ? (carousel.chromeRowHeight + carousel.chromeRowSpacing) * expansionProgress : 0) : 0
    height: implicitHeight

    MD.PageIndicator {
        anchors.horizontalCenter: parent.horizontalCenter
        y: carousel.height + carousel.chromeRowSpacing * root.expansionProgress
        height: carousel.chromeRowHeight * root.expansionProgress
        opacity: root.expansionProgress
        visible: count > 1 && opacity > 0
        count: root.model?.count ?? 0
        currentIndex: carousel.currentIndex
    }

    MD.Carousel {
        id: carousel
        HoverHandler {
            id: carouselHover
        }
        hoverEnabled: true
        anchors.horizontalCenter: parent.horizontalCenter
        width: Math.min(root.width, itemExtent + root.horizontalInset * 2)
        model: root.model
        layout: MD.Enum.CarouselUncontained
        itemExtent: MD.Util.lerp(Math.max(0, root.width - root.horizontalInset * 2), Math.min(176, Math.max(0, root.width - root.horizontalInset * 2)), root.expansionProgress)
        itemSpacing: MD.Token.spacing.small
        contentPadding: model?.count > 1 ? 0 : root.horizontalInset
        contentPaddingVertical: 0
        minimumViewportHeight: MD.Util.lerp(72, 120, root.expansionProgress)
        wheelNavigationEnabled: false
        clipContainer: root.expansionProgress < 1
        header: qsTr("Current wallpapers")
        onClicked: index => {
            const item = model.item(index);
            if (item?.wallpaperId)
                root.openRequested(item.wallpaperId);
        }

        delegate: MD.CarouselItem {
            id: card
            required property var model
            required property var wallpaper
            required property string wallpaperId
            required property string targetSummary

            accessibilityTitle: wallpaper?.name || wallpaperId
            cornerRadius: root.cardRadius
            showBackground: false

            W.ThumbnailImage {
                id: thumbnail
                anchors.fill: parent
                source: card.wallpaper?.preview ?? ""
                resource: card.wallpaper?.resource ?? ""
                wpType: card.wallpaper?.wpType ?? ""
                timeline: (card.wallpaper?.subType ?? 0) === 1
                fillMode: Image.PreserveAspectCrop
                radius: card.effectiveCornerRadius
            }

            Rectangle {
                visible: opacity > 0
                opacity: root.expansionProgress
                anchors.left: thumbnail.left
                anchors.right: thumbnail.right
                anchors.bottom: thumbnail.bottom
                height: Math.min(thumbnail.height, textColumn.implicitHeight + 28)
                radius: card.effectiveCornerRadius
                gradient: Gradient {
                    GradientStop {
                        position: 0
                        color: "transparent"
                    }
                    GradientStop {
                        position: 1
                        color: MD.Util.transparent(MD.Token.color.scrim, 0.78)
                    }
                }
            }

            Column {
                id: textColumn
                visible: opacity > 0
                opacity: root.expansionProgress
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: parent.bottom
                anchors.leftMargin: 12
                anchors.rightMargin: 12
                anchors.bottomMargin: 9
                spacing: 1

                MD.Text {
                    width: parent.width
                    text: card.wallpaper?.name || card.wallpaperId
                    typescale: MD.Token.typescale.label_large
                    color: MD.Token.color.on_primary
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }

                MD.Text {
                    width: parent.width
                    visible: text.length > 0
                    text: card.targetSummary
                    typescale: MD.Token.typescale.label_small
                    color: MD.Util.transparent(MD.Token.color.on_primary, 0.82)
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }
            }
        }
    }
}
