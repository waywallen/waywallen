pragma ComponentBehavior: Bound
import QtQuick
import Qcm.Material as MD

Flow {
    id: control

    required property var targetState
    property string allToolTip: ""
    property bool allRequiresTargets: true

    spacing: 6

    MD.FilterChip {
        text: qsTr("All")
        enabled: control.enabled && (control.targetState.hasTargets || !control.allRequiresTargets)
        selected: control.targetState.allTargets
        onSelectionRequested: control.targetState.selectAll()

        MD.ToolTip.visible: mdState.interactions.hovered && control.allToolTip.length > 0
        MD.ToolTip.text: control.allToolTip
    }

    Repeater {
        model: control.targetState.targets

        MD.FilterChip {
            required property var modelData

            enabled: control.enabled
            width: Math.min(implicitWidth, modelData.maximumWidth)
            text: modelData.label
            icon.name: modelData.iconName
            selected: control.targetState.isSelected(modelData.key)
            onSelectionRequested: control.targetState.toggleTarget(modelData.key)

            MD.ToolTip.visible: mdState.interactions.hovered && modelData.toolTip.length > 0
            MD.ToolTip.text: modelData.toolTip
        }
    }
}
