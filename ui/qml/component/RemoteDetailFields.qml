pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD

ColumnLayout {
    id: root

    property var fields: []
    property var labelForField: field => String(field.label ?? "")

    spacing: 8
    visible: (fields?.length ?? 0) > 0

    Repeater {
        model: root.fields ?? []

        delegate: RowLayout {
            id: fieldRow
            required property var modelData
            readonly property string externalUrl: String(modelData.url ?? "")
            Layout.fillWidth: true
            spacing: 12

            MD.Text {
                objectName: "remoteFieldLabel-" + fieldRow.modelData.key
                Layout.preferredWidth: 104
                Layout.alignment: Qt.AlignTop
                text: root.labelForField(fieldRow.modelData)
                textFormat: Text.PlainText
                typescale: MD.Token.typescale.label_medium
                color: MD.Token.color.on_surface_variant
                wrapMode: Text.WordWrap
            }
            MD.Text {
                objectName: "remoteFieldValue-" + fieldRow.modelData.key
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignTop
                text: String(fieldRow.modelData.value ?? "")
                textFormat: Text.PlainText
                typescale: MD.Token.typescale.body_medium
                color: MD.Token.color.on_surface
                wrapMode: Text.WrapAnywhere
            }
            MD.IconButton {
                objectName: "remoteFieldLink-" + fieldRow.modelData.key
                Layout.alignment: Qt.AlignTop
                visible: fieldRow.externalUrl.length > 0
                icon.name: MD.Token.icon.web
                mdState.size: MD.Enum.XS
                MD.ToolTip.visible: mdState.interactions.hovered
                MD.ToolTip.text: fieldRow.externalUrl
                onClicked: MD.Util.openUrlExternally(fieldRow.externalUrl)
            }
        }
    }
}
