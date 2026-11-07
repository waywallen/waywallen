pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    required property var policy

    readonly property var ruleTypes: [
        {
            key: "excludedApplicationIds",
            patternKey: "excludedApplicationIdPatterns",
            label: qsTr("Application ID")
        },
        {
            key: "excludedWindowTitles",
            patternKey: "excludedWindowTitlePatterns",
            label: qsTr("Window title")
        }
    ]
    readonly property var rules: {
        const result = [];
        for (let index = 0; index < ruleTypes.length; ++index) {
            for (let mode = 0; mode < 2; ++mode) {
                const key = ruleKey(index, mode);
                for (const value of (policy[key] || []))
                    result.push({
                        key: key,
                        type: index,
                        value: value
                    });
            }
        }
        return result;
    }

    signal rulesEdited(var changes)
    signal flushRequested

    implicitWidth: 480
    padding: 0
    showHeader: true
    showBackground: false
    title: qsTr("Excluded windows")
    scrolling: !m_flick.atYBeginning

    leadingAction: MD.Action {
        icon.name: MD.Token.icon.arrow_back
        onTriggered: {
            root.prepareClose();
            root.MD.MProp.page.pop();
        }
    }

    function prepareClose() {
        root.forceActiveFocus();
        root.flushRequested();
    }

    function addRule() {
        const value = exclusionInput.text;
        const key = ruleKey(newType.currentIndex, inputMode(value));
        const values = Array.from(policy[key] || []);
        if (!canAddRule(newType.currentIndex, value))
            return;
        const changes = {};
        changes[key] = values.concat([value]).sort();
        root.rulesEdited(changes);
        exclusionInput.clear();
    }

    function removeRule(rule) {
        const changes = {};
        changes[rule.key] = Array.from(policy[rule.key] || []).filter(value => value !== rule.value);
        root.rulesEdited(changes);
    }

    function ruleKey(type, mode) {
        return mode === 0 ? ruleTypes[type].key : ruleTypes[type].patternKey;
    }

    function ruleCount(type) {
        return (policy[ruleKey(type, 0)] || []).length + (policy[ruleKey(type, 1)] || []).length;
    }

    function inputMode(value) {
        return value.includes("*") || value.includes("?") ? 1 : 0;
    }

    function canAddRule(type, value) {
        return value.length > 0 && ruleCount(type) < 64 && !Array.from(policy[ruleKey(type, inputMode(value))] || []).includes(value);
    }

    contentItem: MD.VerticalFlickable {
        MD.NestedScroll.smoothWheelEnabled: W.Global.smoothScrolling
        id: m_flick
        leftMargin: 16
        rightMargin: 16
        bottomMargin: 12

        ColumnLayout {
            width: m_flick.contentWidth
            spacing: 16

            MD.Text {
                Layout.fillWidth: true
                text: qsTr("Matching is case-sensitive. Wildcards: * matches any number of characters, ? matches one character. Other characters are literal.")
                wrapMode: Text.Wrap
                typescale: MD.Token.typescale.body_small
                color: MD.Token.color.on_surface_variant
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                W.ApplyTextField {
                    id: exclusionInput
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    placeholderText: qsTr("New exclusion")
                    applyText: qsTr("Add")
                    canApply: root.canAddRule(newType.currentIndex, text)
                    onApplied: root.addRule()
                }

                MD.ComboBox {
                    id: newType
                    Layout.preferredWidth: 150
                    label: qsTr("Type")
                    Accessible.name: qsTr("Type")
                    model: root.ruleTypes.map(type => type.label)
                    currentIndex: 0
                }
            }

            MD.Text {
                text: qsTr("Exclusions")
                typescale: MD.Token.typescale.title_small
                color: MD.Token.color.on_surface_variant
                Layout.topMargin: 8
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0

                Repeater {
                    model: root.rules

                    delegate: MD.ListItem {
                        id: ruleItem
                        required property var modelData
                        Layout.fillWidth: true
                        model: null
                        count: root.rules.length
                        text: modelData.value
                        wrapMode: Text.Wrap
                        maximumLineCount: 3
                        corners: MD.Util.listCorners(index, count, 16)
                        mdState.backgroundColor: MD.Token.color.surface_container

                        trailing: RowLayout {
                            spacing: 8

                            W.Tag {
                                text: root.ruleTypes[ruleItem.modelData.type].label
                            }

                            MD.IconButton {
                                icon.name: MD.Token.icon.delete
                                Accessible.name: qsTr("Remove")
                                onClicked: root.removeRule(ruleItem.modelData)
                            }
                        }
                    }
                }

                MD.Text {
                    Layout.fillWidth: true
                    visible: root.rules.length === 0
                    text: qsTr("No excluded windows")
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                }
            }

            Repeater {
                model: W.App.displayManager.displays || []

                delegate: MD.Text {
                    required property var modelData
                    Layout.fillWidth: true
                    visible: modelData.unsupportedWindowExclusions !== 0
                    text: qsTr("%1 cannot apply all window exclusions. Update its display client or use supported rule types.").arg(modelData.displayLabel)
                    wrapMode: Text.Wrap
                    typescale: MD.Token.typescale.body_small
                    color: MD.Token.color.error
                }
            }
        }
    }
}
