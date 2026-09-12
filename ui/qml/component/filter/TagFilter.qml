pragma ComponentBehavior: Bound
import QtQml
import QtQuick
import QtQuick.Layouts
import waywallen.control as WC
import waywallen.ui as W
import Qcm.Material as MD

// Tag membership rule. The value is a set of tags (multi-select). The
// daemon matches "has any" (IS) / "has none" (IS_NOT) against item tags.
QtObject {
    id: root
    required property var popupWindow
    property var filter: null
    property var values: []
    property int condition: WC.StringCondition.STRING_CONDITION_UNSPECIFIED
    // All DB tag names, supplied by the host for the picker dialog.
    property var allTags: []
    // Source-declared labels for those names, display only.
    property var valueLabels: ({})
    // Width the inline tag flow may use before wrapping (rule row width).
    property int availableWidth: 0
    property WC.wallpaperTagFilter subfilter
    property bool _syncing: false

    readonly property var conditionModel: [
        {
            name: qsTr("has any"),
            value: WC.StringCondition.STRING_CONDITION_IS
        },
        {
            name: qsTr("has none"),
            value: WC.StringCondition.STRING_CONDITION_IS_NOT
        },
        {
            name: qsTr("any"),
            value: WC.StringCondition.STRING_CONDITION_UNSPECIFIED
        }
    ]

    function toggleValue(tag) {
        const next = (root.values || []).slice();
        const i = next.indexOf(tag);
        if (i >= 0)
            next.splice(i, 1);
        else
            next.push(tag);
        root.values = next;
    }

    readonly property Component valueDelegate: Component {
        Flow {
            id: valueFlow
            property var tagPresentation: null
            visible: root.condition !== WC.StringCondition.STRING_CONDITION_UNSPECIFIED
            width: root.availableWidth > 0 ? root.availableWidth : implicitWidth
            spacing: 6

            Repeater {
                model: root.values
                delegate: W.Tag {
                    required property var modelData
                    text: W.I18n.valueLabel(root.valueLabels, modelData)
                }
            }

            MD.SmallIconButton {
                icon.name: MD.Token.icon.edit
                onClicked: {
                    if (valueFlow.tagPresentation?.active)
                        return;
                    valueFlow.tagPresentation = root.popupWindow.presentPopup(tagDialogComponent);
                }
            }

            Component {
                id: tagDialogComponent

                W.TagPickerDialog {
                    allTags: root.allTags
                    tagLabels: root.valueLabels
                    selected: root.values
                    onCommit: function (tags) {
                        root.values = tags;
                    }
                }
            }

            Component.onDestruction: valueFlow.tagPresentation?.cancel()
        }
    }

    function syncFromFilter() {
        if (!filter)
            return;
        if (!filter.hasTagFilter)
            filter.tagFilter = subfilter;
        const active = filter.hasTagFilter ? filter.tagFilter : subfilter;
        _syncing = true;
        condition = active.condition;
        values = active.values;
        _syncing = false;
    }

    function commitToFilter() {
        if (!filter || _syncing)
            return;
        subfilter.condition = condition;
        subfilter.values = root.values;
        filter.tagFilter = subfilter;
    }

    onFilterChanged: syncFromFilter()
    onConditionChanged: commitToFilter()
    onValuesChanged: commitToFilter()
}
