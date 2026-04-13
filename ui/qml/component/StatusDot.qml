pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD

ColumnLayout {
    id: root

    property color statusColor: MD.Token.color.error
    property string statusText: "!"

    spacing: 2
    Layout.alignment: Qt.AlignHCenter

    Rectangle {
        Layout.preferredWidth: 12
        Layout.preferredHeight: 12
        Layout.alignment: Qt.AlignHCenter
        radius: 6
        color: root.statusColor
    }

    MD.Text {
        Layout.alignment: Qt.AlignHCenter
        text: root.statusText
        typescale: MD.Token.typescale.label_small
        color: MD.Token.color.on_surface_variant
    }
}
