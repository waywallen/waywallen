pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

W.DiscoverPage {
    id: root

    required sourceId
    required artistId
    required property string artistName
    property string artistAvatarUrl: ""
    property string artistUrl: ""

    title: qsTr("Artist")
    showHeader: MD.MProp.size.isCompact

    browserHeading: RowLayout {
        spacing: 16

        W.ThumbnailImage {
            Layout.preferredWidth: 56
            Layout.preferredHeight: 56
            source: root.artistAvatarUrl
            maxSize: Qt.size(112, 112)
            fillMode: Image.PreserveAspectCrop
            radius: 28
            visible: root.artistAvatarUrl.length > 0
        }

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 4

            MD.Label {
                Layout.fillWidth: true
                text: root.artistName
                textFormat: Text.PlainText
                typescale: MD.Token.typescale.headline_small
                wrapMode: Text.WordWrap
            }

            MD.Label {
                Layout.fillWidth: true
                text: root.sourceName(root.sourceId)
                visible: text.length > 0
                typescale: MD.Token.typescale.body_medium
                color: MD.Token.color.on_surface_variant
            }
        }

        MD.StandardIconButton {
            icon.name: MD.Token.icon.open_in_new
            visible: root.artistUrl.length > 0
            onClicked: MD.Util.openUrlExternally(root.artistUrl)
            MD.ToolTip.text: qsTr("Open web page")
            MD.ToolTip.visible: mdState.interactions.hovered
        }
    }
}
