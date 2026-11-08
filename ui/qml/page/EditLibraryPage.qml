pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root

    required property var libraryId
    required property string originalName
    required property var initialValues
    required property var schemas
    property var originalValues: ({})
    property var values: ({})
    property string errorText: ""
    readonly property bool hasChanges: nameField.text.trim() !== originalName || Object.keys(values).some(key => values[key] !== originalValues[key])
    readonly property bool canApply: !editQuery.querying && nameField.text.trim().length > 0 && hasChanges

    title: qsTr("Edit library")
    implicitWidth: 480
    scrolling: !editFlick.atYBeginning

    function submit() {
        if (!canApply)
            return;
        errorText = "";
        editQuery.name = nameField.text.trim();
        editQuery.settings = values;
        editQuery.reload();
    }

    Component.onCompleted: {
        nameField.text = originalName;
        const next = Object.assign({}, initialValues);
        for (const schema of schemas)
            next[schema.key] = next[schema.key] ?? schema.default_value;
        originalValues = Object.assign({}, next);
        values = next;
        nameField.forceActiveFocus();
    }

    W.LibraryEditQuery {
        id: editQuery
        libraryId: root.libraryId
        forwardError: false
        onErrorOccurred: error => root.errorText = error
        onFinished: root.MD.MProp.page.pop()
    }

    contentItem: MD.VerticalFlickable {
        id: editFlick

        topMargin: 12
        bottomMargin: 16
        leftMargin: 16
        rightMargin: 16

        ColumnLayout {
            width: editFlick.contentWidth
            spacing: 12

            MD.TextField {
                id: nameField

                Layout.fillWidth: true
                mdState.size: MD.Enum.S
                placeholderText: qsTr("Library name")
                enabled: !editQuery.querying
                onTextEdited: root.errorText = ""
                onAccepted: root.submit()
            }
            W.LibrarySettingsFields {
                Layout.fillWidth: true
                visible: root.schemas.length > 0
                enabled: !editQuery.querying
                schemas: root.schemas
                values: root.values
                onCommitted: (key, value) => {
                    const next = Object.assign({}, root.values);
                    next[key] = value;
                    root.values = next;
                    root.errorText = "";
                }
            }
            MD.Text {
                Layout.fillWidth: true
                visible: text.length > 0
                text: root.errorText
                color: MD.Token.color.error
                wrapMode: Text.WordWrap
            }
            MD.BusyButton {
                Layout.fillWidth: true
                text: qsTr("Apply")
                busy: editQuery.querying
                enabled: root.canApply
                mdState.type: MD.Enum.BtFilled
                onClicked: root.submit()
            }
        }
    }
}
