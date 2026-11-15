pragma ComponentBehavior: Bound
import QtQml
import QtQuick
import waywallen.control as WC
import Qcm.Material as MD

QtObject {
    id: root
    property var filter: null
    property string value: ""
    property int condition: WC.StringCondition.STRING_CONDITION_UNSPECIFIED
    property WC.wallpaperStringFilter subfilter
    property bool _syncing: false

    readonly property var conditionModel: [
        {
            name: qsTr("contains"),
            value: WC.StringCondition.STRING_CONDITION_CONTAINS
        },
        {
            name: qsTr("not contains"),
            value: WC.StringCondition.STRING_CONDITION_CONTAINS_NOT
        },
        {
            name: qsTr("is"),
            value: WC.StringCondition.STRING_CONDITION_IS
        },
        {
            name: qsTr("is not"),
            value: WC.StringCondition.STRING_CONDITION_IS_NOT
        }
    ]

    readonly property Component valueDelegate: Component {
        MD.InputChip {
            id: valueChip
            visible: root.condition !== WC.StringCondition.STRING_CONDITION_UNSPECIFIED
            text: root.value
            rightPadding: edit ? 0 : 16
            onSelectionRequested: edit = true
            editDelegate: FocusScope {
                id: editor
                implicitWidth: Math.max(16, input.implicitWidth) + 4 + confirmButton.implicitWidth
                implicitHeight: Math.max(input.implicitHeight, confirmButton.implicitHeight)

                function submit() {
                    root.value = input.text;
                    valueChip.edit = false;
                }

                MD.TextInput {
                    id: input
                    anchors.left: parent.left
                    anchors.right: confirmButton.left
                    anchors.rightMargin: 4
                    anchors.verticalCenter: parent.verticalCenter
                    focus: true
                    typescale: MD.Token.typescale.label_large
                    color: valueChip.mdState.textColor
                    text: root.value
                    onAccepted: editor.submit()
                }

                MD.StandardIconButton {
                    id: confirmButton
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    implicitBackgroundSize: 0
                    padding: 7
                    icon.width: 18
                    icon.height: 18
                    icon.name: MD.Token.icon.check
                    Accessible.name: qsTr("Apply")
                    onClicked: editor.submit()
                    MD.ToolTip.visible: mdState.interactions.hovered
                    MD.ToolTip.text: qsTr("Apply")
                }
            }
        }
    }

    function syncFromFilter() {
        if (!filter)
            return;
        if (!filter.hasStringFilter)
            filter.stringFilter = subfilter;
        const active = filter.hasStringFilter ? filter.stringFilter : subfilter;
        _syncing = true;
        condition = active.condition;
        value = active.value;
        _syncing = false;
    }

    function commitToFilter() {
        if (!filter || _syncing)
            return;
        subfilter.condition = condition;
        subfilter.value = value;
        filter.stringFilter = subfilter;
    }

    onFilterChanged: syncFromFilter()
    onConditionChanged: commitToFilter()
    onValueChanged: commitToFilter()
}
