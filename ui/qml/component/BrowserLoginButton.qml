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
                maximumWidth: 420
                model: root.browserHelpers
                contentDelegate: MD.SelectionMenuItem {
                    id: helperItem
                    required property var modelData
                    readonly property string pluginId: modelData.pluginId || ""
                    text: W.I18n.tr(modelData.labelText) || modelData.label || modelData.id
                    supportText: modelData.available ? "" : (modelData.error || qsTr("This browser is unavailable."))
                    Accessible.description: [pluginId, supportText].filter(value => value.length > 0).join("\n")
                    supporting: pluginId.length > 0 ? pluginSupporting : null
                    verticalPadding: pluginId.length > 0 ? 6 : 0
                    enabled: modelData.available
                    selected: modelData.id === root.browserHelperId
                    onTriggered: W.BrowserLoginState.selectHelper(root.sourceId, root.actionId, modelData.id)

                    Component {
                        id: pluginSupporting

                        Item {
                            implicitWidth: Math.max(pluginTag.implicitWidth, errorLabel.visible ? errorLabel.implicitWidth : 0)
                            implicitHeight: 4 + pluginTag.implicitHeight + (errorLabel.visible ? 4 + errorLabel.implicitHeight : 0)

                            W.Tag {
                                id: pluginTag
                                y: 4
                                x: helperItem.mirrored ? parent.width - width : 0
                                width: Math.min(implicitWidth, parent.width)
                                text: helperItem.pluginId
                                bgColor: MD.Token.color.surface_container_high
                                fgColor: MD.Token.color.on_surface_variant
                                textItem.width: Math.max(0, width - 16)
                                textItem.maximumLineCount: 1
                                textItem.elide: Text.ElideRight
                                textItem.wrapMode: Text.NoWrap

                                HoverHandler {
                                    id: pluginHover
                                }

                                MD.ToolTip.text: helperItem.pluginId
                                MD.ToolTip.visible: pluginHover.hovered && textItem.truncated
                            }

                            MD.Text {
                                id: errorLabel
                                y: pluginTag.y + pluginTag.height + 4
                                width: parent.width
                                visible: text.length > 0
                                text: helperItem.supportText
                                color: helperItem.mdState.supportTextColor
                                typescale: MD.Token.typescale.body_small
                                horizontalAlignment: helperItem.mirrored ? Text.AlignRight : Text.AlignLeft
                                wrapMode: Text.NoWrap
                            }
                        }
                    }
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
