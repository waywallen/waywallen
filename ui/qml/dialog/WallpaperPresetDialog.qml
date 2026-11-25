pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD

MD.Dialog {
    id: root

    required property bool busy
    required property bool currentValuesReady
    required property var presets

    property string wallpaperId: ""
    property string errorText: ""
    property string operation: ""
    property var preset: null
    property bool returnToList: false

    readonly property bool managing: operation === "manage"
    readonly property bool naming: operation === "save" || operation === "rename"
    readonly property bool usesCurrentValues: operation === "save" || operation === "update" || operation === "apply"
    readonly property bool canSubmit: !managing && !busy && (!usesCurrentValues || currentValuesReady) &&
        (operation === "save" || preset !== null) && (!naming || (nameField.text.trim().length > 0 &&
        (operation !== "rename" || nameField.text.trim() !== preset?.name)))

    signal requested(string operation, string presetId, string name)
    signal refreshRequested()

    title: {
        switch (operation) {
        case "save": return qsTr("Save preset");
        case "apply": return qsTr("Apply preset");
        case "update": return qsTr("Update preset");
        case "rename": return qsTr("Rename preset");
        case "remove": return qsTr("Delete preset");
        default: return qsTr("Manage presets");
        }
    }
    parent: MD.Overlay.overlay
    implicitWidth: Math.min(managing ? 380 : 320, parent ? parent.width - 32 : 380)
    closePolicy: busy ? MD.Popup.NoAutoClose : MD.Popup.CloseOnEscape
    onOpened: focusName()

    function openFor(id, op, entry) {
        wallpaperId = id;
        returnToList = false;
        begin(op, entry);
        open();
    }

    function begin(op, entry) {
        errorText = "";
        operation = op;
        preset = entry;
        if (naming) {
            nameField.text = op === "rename" ? entry.name : "";
            if (opened)
                focusName();
        }
    }

    function manage(op, entry) {
        returnToList = true;
        begin(op, entry);
    }

    function focusName() {
        if (naming) {
            nameField.forceActiveFocus();
            nameField.selectAll();
        }
    }

    function completed() {
        if (returnToList && presets.length > 0) {
            returnToList = false;
            begin("manage", null);
        } else {
            close();
        }
    }

    function submit() {
        if (!canSubmit)
            return;
        errorText = "";
        requested(operation, preset?.id ?? "", naming ? nameField.text.trim() : "");
    }

    contentItem: ColumnLayout {
        spacing: 8

        MD.Text {
            Layout.fillWidth: true
            visible: root.errorText.length > 0
            text: root.errorText
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: MD.Token.color.error
        }

        ListView {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(count * 40, 240)
            visible: root.managing
            enabled: !root.busy
            model: root.managing ? root.presets : []
            clip: true
            keyNavigationEnabled: false
            MD.ScrollIndicator.vertical: MD.ScrollIndicator {}

            delegate: RowLayout {
                id: row
                required property var modelData

                width: ListView.view.width
                height: 40
                spacing: 4

                MD.Text {
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    text: row.modelData.name
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    MD.ToolTip.text: text
                    MD.ToolTip.visible: truncated && nameHover.hovered
                    HoverHandler { id: nameHover }
                }

                MD.IconButton {
                    icon.name: MD.Token.icon.update
                    mdState.size: MD.Enum.XS
                    enabled: root.currentValuesReady
                    Accessible.name: qsTr("Update from current values")
                    MD.ToolTip.text: Accessible.name
                    MD.ToolTip.visible: mdState.interactions.hovered
                    onClicked: root.manage("update", row.modelData)
                }

                MD.IconButton {
                    icon.name: MD.Token.icon.edit
                    mdState.size: MD.Enum.XS
                    Accessible.name: qsTr("Rename preset")
                    MD.ToolTip.text: Accessible.name
                    MD.ToolTip.visible: mdState.interactions.hovered
                    onClicked: root.manage("rename", row.modelData)
                }

                MD.IconButton {
                    icon.name: MD.Token.icon.delete
                    mdState.size: MD.Enum.XS
                    Accessible.name: qsTr("Delete preset")
                    MD.ToolTip.text: Accessible.name
                    MD.ToolTip.visible: mdState.interactions.hovered
                    onClicked: root.manage("remove", row.modelData)
                }
            }
        }

        MD.TextField {
            id: nameField

            Layout.fillWidth: true
            visible: root.naming
            enabled: !root.busy
            mdState.size: MD.Enum.S
            placeholderText: qsTr("Preset name")
            onAccepted: root.submit()
        }

        MD.Text {
            Layout.fillWidth: true
            visible: !root.managing && !root.naming
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            text: {
                const name = root.preset?.name ?? "";
                switch (root.operation) {
                case "apply": return qsTr('Apply "%1" and replace current settings not saved in a preset?').arg(name);
                case "update": return qsTr('Replace the saved values in "%1" with the current property values?').arg(name);
                case "remove": return qsTr('Delete preset "%1"?').arg(name);
                default: return "";
                }
            }
        }

        MD.Button {
            visible: !root.currentValuesReady && !root.busy
            text: qsTr("Refresh properties")
            onClicked: root.refreshRequested()
        }
    }

    Component.onCompleted: {
        const buttons = root.footer as MD.DialogButtonBox;
        buttons.addItem(cancelButton);
        buttons.addItem(submitButton);
    }

    MD.Button {
        id: cancelButton
        text: root.managing ? qsTr("Close") : qsTr("Cancel")
        enabled: !root.busy
        mdState.type: MD.Enum.BtText
        onClicked: root.completed()
    }

    MD.Button {
        id: submitButton
        visible: !root.managing
        enabled: root.canSubmit
        text: root.title
        mdState.type: MD.Enum.BtText
        onClicked: root.submit()
    }
}
