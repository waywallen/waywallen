pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.ListGridBaseDelegate {
    id: root

    property var wallpaper: model.modelData
    cellHeight: widthProvider.width + 32

    MD.Card {
        id: m_card
        x: parent.cellX
        y: parent.cellY + 6
        width: parent.widthProvider.width
        height: root.cellHeight - 12
        type: MD.Enum.CardFilled
        clip: true

        onClicked: root.clicked()

        contentItem: ColumnLayout {
            spacing: 0

            // Preview thumbnail
            Image {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.margins: 0
                visible: root.wallpaper.preview !== undefined && root.wallpaper.preview !== ""
                source: root.wallpaper.preview ? "file://" + root.wallpaper.preview : ""
                fillMode: Image.PreserveAspectFit
            }

            // Title + type
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 2

                MD.Text {
                    text: root.wallpaper.name || "Untitled"
                    typescale: MD.Token.typescale.title_small
                    color: MD.Token.color.on_surface
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }

                MD.Text {
                    text: root.wallpaper.wpType || ""
                    typescale: MD.Token.typescale.label_medium
                    color: MD.Token.color.on_surface_variant
                }
            }
        }
    }
}
