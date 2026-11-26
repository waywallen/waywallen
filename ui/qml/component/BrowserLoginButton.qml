pragma ComponentBehavior: Bound
import QtQuick
import Qcm.Material as MD
import waywallen.ui as W

Item {
    id: root

    required property string sourceId
    required property string actionId
    property string text: ""
    property var browserHelpers: []
    property string defaultBrowserHelperId: ""
    property bool actionEnabled: true
    property bool busy: false
    property bool refreshing: false
    property string availabilityError: ""
    property bool centered: false
    readonly property string browserHelperId: W.BrowserLoginState.helperId(sourceId, actionId, browserHelpers, defaultBrowserHelperId)
    readonly property var selectedHelper: browserHelpers.find(helper => helper.id === browserHelperId) ?? null
    readonly property string helperLabel: selectedHelper ? (W.I18n.tr(selectedHelper.labelText) || selectedHelper.label || selectedHelper.id) : ""
    readonly property string helperError: selectedHelper && !selectedHelper.available ? (selectedHelper.error || qsTr("This browser is unavailable.")) : ""
    readonly property string errorText: helperError || availabilityError

    signal loginRequested(string browserHelperId)
    signal refreshRequested

    implicitWidth: Math.max(plainButton.implicitWidth, splitButton.implicitWidth)
    implicitHeight: content.implicitHeight

    Column {
        id: content
        width: root.width
        spacing: 6

        MD.Button {
            id: plainButton
            x: root.centered ? (root.width - width) / 2 : 0
            visible: root.browserHelpers.length === 0
            text: root.text
            enabled: root.actionEnabled && !root.busy
            mdState.type: MD.Enum.BtFilledTonal
            onClicked: root.loginRequested("")
        }

        MD.SplitButton {
            id: splitButton
            x: root.centered ? (root.width - width) / 2 : 0
            visible: root.browserHelpers.length > 0
            text: root.text
            enabled: root.actionEnabled && !root.busy
            mdState.type: MD.Enum.BtFilledTonal
            expanded: helperMenu.visible
            onClicked: root.loginRequested(root.browserHelperId)
            onTrailingClicked: helperMenu.visible ? helperMenu.close() : helperMenu.openFor(splitButton)
            MD.ToolTip.text: qsTr("Browser: %1").arg(root.helperLabel)
            MD.ToolTip.visible: helperHover.hovered

            HoverHandler {
                id: helperHover
            }

            Binding {
                target: splitButton.leadingControl
                property: "enabled"
                value: root.actionEnabled && !root.busy && !!root.selectedHelper?.available
            }

            MD.Menu {
                id: helperMenu
                model: root.browserHelpers
                contentDelegate: MD.SelectionMenuItem {
                    required property var modelData
                    text: W.I18n.tr(modelData.labelText) || modelData.label || modelData.id
                    supportText: modelData.available ? "" : (modelData.error || qsTr("This browser is unavailable."))
                    enabled: modelData.available
                    selected: modelData.id === root.browserHelperId
                    onTriggered: W.BrowserLoginState.selectHelper(root.sourceId, root.actionId, modelData.id)
                }
                MD.MenuSeparator {}
                MD.MenuItem {
                    text: qsTr("Refresh browsers")
                    icon.name: MD.Token.icon.refresh
                    enabled: !root.refreshing
                    onTriggered: root.refreshRequested()
                }
            }
        }

        MD.Text {
            width: root.width
            visible: root.errorText.length > 0
            text: root.helperError.length > 0 ? qsTr("%1: %2").arg(root.helperLabel).arg(root.helperError) : root.errorText
            horizontalAlignment: root.centered ? Text.AlignHCenter : Text.AlignLeft
            wrapMode: Text.WordWrap
            color: MD.Token.color.error
            typescale: MD.Token.typescale.body_small
        }

        MD.Button {
            x: root.centered ? (root.width - width) / 2 : 0
            visible: root.browserHelpers.length === 0 && root.availabilityError.length > 0
            text: qsTr("Refresh browsers")
            enabled: !root.refreshing
            mdState.type: MD.Enum.BtText
            onClicked: root.refreshRequested()
        }
    }
}
