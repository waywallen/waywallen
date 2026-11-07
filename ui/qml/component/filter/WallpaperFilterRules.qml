pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import waywallen.control as WC
import waywallen.ui as W
import Qcm.Material as MD

MD.VerticalLazyList {
    id: root

    required property var popupWindow
    property var model: null
    property var supportedTypes: []
    property var typeLabels: ({})
    property var allTags: []
    property var valueLabels: ({})
    property var allContentRatings: []
    property color ruleBackgroundColor: root.MD.MProp.color.surface

    spacing: 2

    source: MD.ItemModelSource {
        id: ruleSource
        model: root.model
        keyMode: MD.ItemModelSource.PersistentIndexKey
    }

    delegate: Column {
        id: ruleRow
        required property MD.LazyRow row
        readonly property bool startsSection: {
            void (row.revision);
            return row.index === 0 || ruleSource.valueAt(row.index - 1)?.group !== row.value.group;
        }
        spacing: 2

        RowLayout {
            id: sectionRow
            width: parent.width
            visible: ruleRow.startsSection
            spacing: 8
            readonly property int groupId: ruleRow.row.value.group
            readonly property int sectionIndex: {
                void (ruleRow.row.revision);
                return root.model ? root.model.sectionIndexForGroup(groupId) : -1;
            }
            readonly property int currentOp: {
                if (!root.model || sectionIndex <= 0)
                    return -1;
                void (root.model.filterLogics);
                return root.model.logicOpAt(sectionIndex);
            }

            MD.Label {
                Layout.fillWidth: true
                text: qsTranslate("WallpaperFilterDialog", "Group %1").arg(sectionRow.sectionIndex + 1)
                typescale: MD.Token.typescale.label_medium
            }

            MD.SegmentedButtonGroup {
                visible: sectionRow.sectionIndex > 0

                MD.SegmentedButton {
                    mdState.size: MD.Enum.XS
                    text: qsTranslate("WallpaperFilterDialog", "AND")
                    checked: sectionRow.currentOp !== WC.LogicOp.LOGIC_OP_OR
                    onCheckedChangeRequested: root.model.setLogicOpAt(sectionRow.sectionIndex, WC.LogicOp.LOGIC_OP_AND)
                }

                MD.SegmentedButton {
                    mdState.size: MD.Enum.XS
                    text: qsTranslate("WallpaperFilterDialog", "OR")
                    checked: sectionRow.currentOp === WC.LogicOp.LOGIC_OP_OR
                    onCheckedChangeRequested: root.model.setLogicOpAt(sectionRow.sectionIndex, WC.LogicOp.LOGIC_OP_OR)
                }
            }

            MD.IconButton {
                mdState.size: MD.Enum.XS
                icon.name: MD.Token.icon.add
                Accessible.name: qsTr("Add rule")
                MD.ToolTip.visible: mdState.interactions.hovered
                MD.ToolTip.text: qsTr("Add rule")
                onClicked: root.model.appendRuleInGroup(sectionRow.groupId)
            }
            MD.IconButton {
                mdState.size: MD.Enum.XS
                icon.name: MD.Token.icon.delete
                Accessible.name: qsTr("Delete group")
                MD.ToolTip.visible: mdState.interactions.hovered
                MD.ToolTip.text: qsTr("Delete group")
                onClicked: root.model.deleteGroup(sectionRow.groupId)
            }
        }

        W.WallpaperFilter {
            row: ruleRow.row
            ruleModel: root.model
            model: ruleSource.objectForKey(ruleRow.row.key)
            width: parent.width
            popupWindow: root.popupWindow
            supportedTypes: root.supportedTypes
            typeLabels: root.typeLabels
            allTags: root.allTags
            valueLabels: root.valueLabels
            allContentRatings: root.allContentRatings
            backgroundColor: root.ruleBackgroundColor
        }
    }
}
