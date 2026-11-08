pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

ColumnLayout {
    id: root
    property var schemas: []
    property var values: ({})
    signal committed(string key, string value)
    Repeater {
        model: root.schemas
        delegate: W.SettingField {
            required property var modelData
            Layout.fillWidth: true
            schema: modelData
            value: root.values[modelData.key] ?? modelData.default_value
            onCommitted: (key, value) => root.committed(key, value)
        }
    }
}
