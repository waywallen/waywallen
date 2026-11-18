pragma ComponentBehavior: Bound
import QtQuick
import Qcm.Material as MD

Item {
    id: root

    property var texts: []
    property real maximumWidth: parent ? Math.max(0, parent.width - 10) : 512

    implicitWidth: labels.implicitWidth
    implicitHeight: labels.implicitHeight
    width: Math.min(implicitWidth, maximumWidth)
    height: implicitHeight
    visible: texts.length > 0 && width > 0
    clip: true
    enabled: false

    Row {
        id: labels
        anchors.right: parent.right
        spacing: 4

        Repeater {
            model: root.texts
            delegate: Rectangle {
                id: badge
                required property string modelData

                width: Math.min(Math.ceil(metrics.advanceWidth) + 8, root.maximumWidth)
                height: Math.ceil(metrics.height) + 2
                radius: 4
                color: Qt.rgba(0, 0, 0, 0.26)

                TextMetrics {
                    id: metrics
                    font: label.font
                    text: badge.modelData
                }

                MD.Text {
                    id: label
                    anchors.fill: parent
                    anchors.leftMargin: 4
                    anchors.rightMargin: 4
                    text: badge.modelData
                    textFormat: Text.PlainText
                    color: "white"
                    typescale: MD.Token.typescale.label_small
                    maximumLineCount: 1
                    elide: Text.ElideRight
                    verticalAlignment: Text.AlignVCenter
                }
            }
        }
    }
}
