pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root

    title: qsTr("Card badge rules")
    showHeader: true
    property string editingId: ""
    property int fieldIndex: 0
    property bool ruleEnabled: true

    function fieldLabel(field) {
        if (field === "content_rating")
            return qsTr("Content rating");
        if (field === "wp_type")
            return qsTr("Wallpaper type");
        return qsTr("Tag");
    }
    function editRule(rule) {
        editingId = rule.id;
        fieldIndex = W.WallpaperBadgeSettings.fields.indexOf(rule.field);
        ruleEnabled = rule.enabled !== false;
        matchField.text = rule.value;
        labelField.text = rule.text;
    }
    function resetForm() {
        editingId = "";
        fieldIndex = 0;
        ruleEnabled = true;
        matchField.clear();
        labelField.clear();
    }

    contentItem: MD.VerticalFlickable {
        id: flick
        leftMargin: 16
        rightMargin: 16
        topMargin: 8
        bottomMargin: 16

        ColumnLayout {
            width: flick.contentWidth
            spacing: 12

            MD.Text {
                Layout.fillWidth: true
                text: qsTr("Rules ignore case and match the whole value. Use * for any number of characters and ? for one character. A tag rule matches any tag on the wallpaper.")
                typescale: MD.Token.typescale.body_small
                wrapMode: Text.WordWrap
            }
            Repeater {
                model: W.WallpaperBadgeSettings.rules
                delegate: RowLayout {
                    id: ruleRow
                    required property var modelData
                    Layout.fillWidth: true

                    MD.Switch {
                        checked: ruleRow.modelData.enabled !== false
                        Accessible.name: ruleRow.modelData.text
                        onCheckedChangeRequested: checked => W.WallpaperBadgeSettings.setRuleEnabled(ruleRow.modelData.id, checked)
                    }
                    MD.Text {
                        Layout.fillWidth: true
                        Layout.minimumWidth: 0
                        text: qsTr("%1: %2 → %3").arg(root.fieldLabel(ruleRow.modelData.field)).arg(ruleRow.modelData.value).arg(ruleRow.modelData.text)
                        textFormat: Text.PlainText
                        maximumLineCount: 2
                        elide: Text.ElideRight
                        wrapMode: Text.WordWrap
                    }
                    MD.ActionToolBar {
                        actions: [editAction, removeAction]

                        MD.Action {
                            id: editAction
                            text: qsTr("Edit")
                            icon.name: MD.Token.icon.edit
                            onTriggered: root.editRule(ruleRow.modelData)
                        }
                        MD.Action {
                            id: removeAction
                            text: qsTr("Remove")
                            icon.name: MD.Token.icon.delete
                            onTriggered: {
                                if (root.editingId === ruleRow.modelData.id)
                                    root.resetForm();
                                W.WallpaperBadgeSettings.removeRule(ruleRow.modelData.id);
                            }
                        }
                    }
                }
            }
            MD.Divider { Layout.fillWidth: true }
            MD.ComboBox {
                Layout.fillWidth: true
                mdState.size: MD.Enum.S
                label: qsTr("Match field")
                model: W.WallpaperBadgeSettings.fields.map(field => root.fieldLabel(field))
                currentIndex: root.fieldIndex
                onSelectionChangeRequested: index => root.fieldIndex = index
            }
            MD.TextField {
                id: matchField
                Layout.fillWidth: true
                mdState.size: MD.Enum.S
                placeholderText: qsTr("Matching value")
                maximumLength: 256
            }
            MD.TextField {
                id: labelField
                Layout.fillWidth: true
                mdState.size: MD.Enum.S
                placeholderText: qsTr("Badge text")
                maximumLength: 64
            }
            RowLayout {
                MD.Button {
                    text: root.editingId.length > 0 ? qsTr("Save") : qsTr("Add")
                    enabled: matchField.text.trim().length > 0 && labelField.text.trim().length > 0
                        && (root.editingId.length > 0 || W.WallpaperBadgeSettings.rules.length < 32)
                    onClicked: {
                        if (W.WallpaperBadgeSettings.saveRule(root.editingId,
                            W.WallpaperBadgeSettings.fields[root.fieldIndex], matchField.text,
                            labelField.text, root.ruleEnabled))
                            root.resetForm();
                    }
                }
                MD.Button {
                    text: qsTr("Cancel")
                    visible: root.editingId.length > 0
                    onClicked: root.resetForm()
                }
            }
        }
    }
}
