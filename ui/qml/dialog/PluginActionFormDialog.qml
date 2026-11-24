pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Dialog {
    id: root

    property var action: ({})
    property var values: ({})
    property int resetRevision: 0
    signal submitted(var values)

    readonly property var fields: action.fields || []
    readonly property bool complete: {
        for (let i = 0; i < fields.length; ++i) {
            if (fields[i].required && String(values[fields[i].key] || "").length === 0)
                return false;
        }
        return true;
    }

    title: W.I18n.tr(action.labelText) || qsTr("Continue")
    parent: MD.Overlay.overlay
    modal: true
    horizontalPadding: 24
    implicitWidth: Math.min(440, parent ? parent.width - 48 : 440)
    standardButtons: MD.Dialog.Cancel | MD.Dialog.Ok

    function openFor(nextAction) {
        action = nextAction || ({});
        clearValues();
        open();
    }

    function clearValues() {
        values = ({});
        resetRevision += 1;
    }

    function setValue(key, value) {
        const next = Object.assign({}, values);
        next[key] = value;
        values = next;
    }

    onAboutToShow: {
        const accept = standardButton(MD.Dialog.Ok);
        if (accept) {
            accept.text = W.I18n.tr(action.labelText) || qsTr("Continue");
            accept.enabled = Qt.binding(function() { return root.complete; });
        }
    }
    onAccepted: {
        const submittedValues = Object.assign({}, values);
        clearValues();
        submitted(submittedValues);
    }
    onClosed: clearValues()

    contentItem: ColumnLayout {
        spacing: 12

        MD.Text {
            Layout.fillWidth: true
            visible: text.length > 0
            text: W.I18n.tr(root.action.descriptionText)
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.on_surface_variant
            wrapMode: Text.WordWrap
        }

        Repeater {
            model: root.fields
            delegate: ColumnLayout {
                id: fieldItem
                required property var modelData
                Layout.fillWidth: true
                spacing: 4

                MD.Text {
                    Layout.fillWidth: true
                    text: W.I18n.tr(fieldItem.modelData.labelText) || fieldItem.modelData.key
                    typescale: MD.Token.typescale.label_large
                    color: MD.Token.color.on_surface
                }

                MD.TextField {
                    id: input
                    Layout.fillWidth: true
                    mdState.size: MD.Enum.S
                    placeholderText: W.I18n.tr(fieldItem.modelData.placeholderText)
                    echoMode: fieldItem.modelData.secret ? TextInput.Password : TextInput.Normal
                    inputMethodHints: fieldItem.modelData.secret
                        ? Qt.ImhSensitiveData | Qt.ImhNoPredictiveText
                        : Qt.ImhNone
                    onTextEdited: root.setValue(fieldItem.modelData.key, text)

                    Connections {
                        target: root
                        function onResetRevisionChanged() {
                            input.text = "";
                        }
                    }
                }

                MD.Text {
                    Layout.fillWidth: true
                    visible: text.length > 0
                    // Bare http(s) URLs become clickable links, as in SettingField
                    text: W.I18n.tr(fieldItem.modelData.descriptionText).replace(/(https?:\/\/[^\s]+)/g,
                        '<a href="$1"><font color="' + String(MD.Token.color.primary) + '">$1</font></a>')
                    textFormat: Text.StyledText
                    onLinkActivated: link => W.App.openExternalLink(link)
                    typescale: MD.Token.typescale.body_small
                    color: MD.Token.color.on_surface_variant
                    wrapMode: Text.WordWrap
                }
            }
        }
    }
}
