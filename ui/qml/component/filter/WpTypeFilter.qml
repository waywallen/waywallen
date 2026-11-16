pragma ComponentBehavior: Bound
import QtQml
import QtQuick
import waywallen.control as WC
import waywallen.ui as W
import Qcm.Material as MD

QtObject {
    id: root
    property var filter: null
    property string value: ""
    property int condition: WC.StringCondition.STRING_CONDITION_UNSPECIFIED
    property WC.wallpaperStringFilter subfilter
    property var supportedTypes: []
    // Type key -> label, as the renderer plugins named them.
    property var typeLabels: ({})
    property bool _syncing: false

    readonly property var conditionModel: [
        { name: qsTr("is"),     value: WC.StringCondition.STRING_CONDITION_IS },
        { name: qsTr("is not"), value: WC.StringCondition.STRING_CONDITION_IS_NOT }
    ]

    readonly property var typeOptions: {
        const src = supportedTypes && supportedTypes.length > 0
                  ? supportedTypes
                  : ["image", "video", "scene"];
        return src.map(t => ({ name: W.I18n.valueLabel(root.typeLabels, t), value: t }));
    }

    function labelFor(v) {
        const item = typeOptions.find(e => e.value === v);
        return item ? item.name : v;
    }

    readonly property Component valueDelegate: Component {
        MD.InputChip {
            id: valueChip
            visible: root.condition !== WC.StringCondition.STRING_CONDITION_UNSPECIFIED
            text: root.labelFor(root.value)
            onSelectionRequested: valueMenu.open()

            MD.Menu {
                id: valueMenu
                parent: valueChip
                y: parent.height
                model: root.typeOptions
                contentDelegate: MD.MenuItem {
                    required property var modelData
                    text: modelData.name
                    onClicked: {
                        root.value = modelData.value;
                        valueMenu.close();
                    }
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
