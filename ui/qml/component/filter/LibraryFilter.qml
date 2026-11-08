pragma ComponentBehavior: Bound
import QtQml
import QtQuick
import waywallen.control as WC
import waywallen.ui as W
import Qcm.Material as MD

QtObject {
    id: root

    property var filter: null
    property double value: 0
    property int condition: WC.IntCondition.INT_CONDITION_EQUAL
    property WC.wallpaperIntFilter subfilter
    property real availableWidth: 280
    property bool _syncing: false

    readonly property var conditionModel: [
        {
            name: qsTr("includes"),
            value: WC.IntCondition.INT_CONDITION_EQUAL
        },
        {
            name: qsTr("excludes"),
            value: WC.IntCondition.INT_CONDITION_EQUAL_NOT
        }
    ]

    readonly property var libraryOptions: {
        const options = Array.from(W.App.libraryManager.libraries).map(library => ({
                    name: library.name,
                    path: library.path,
                    pluginName: library.pluginName,
                    value: library.id
                }));
        if (value > 0 && !options.some(option => option.value === value))
            options.unshift({
                name: qsTr("Unavailable library #%1").arg(value),
                value: value
            });
        return options;
    }

    readonly property Component valueDelegate: Component {
        MD.ComboBox {
            id: libraryCombo
            width: Math.min(280, root.availableWidth)
            mdState.size: MD.Enum.XS
            model: root.libraryOptions
            textRole: "name"
            valueRole: "value"
            currentValue: root.value
            displayText: currentIndex >= 0 ? currentText : qsTr("Select library")
            onSelectionChangeRequested: (index, value) => root.value = value
            delegate: MD.MenuItem {
                required property var model
                required property int index
                width: libraryCombo.popup?.contentItem?.width ?? implicitWidth
                text: model.name
                selected: libraryCombo.currentIndex === index
                highlighted: libraryCombo.highlightedIndex === index
                focusPolicy: Qt.NoFocus
                MD.ToolTip.text: model.path ? model.path + " · " + model.pluginName : text
                MD.ToolTip.visible: mdState.interactions.hovered
            }
            MD.ToolTip.text: {
                const option = root.libraryOptions[currentIndex];
                return option && option.path ? option.path + " · " + option.pluginName : displayText;
            }
            MD.ToolTip.visible: mdState.interactions.hovered
        }
    }

    function syncFromFilter() {
        if (!filter)
            return;
        const active = filter.hasIntFilter ? filter.intFilter : subfilter;
        _syncing = true;
        condition = active.condition === WC.IntCondition.INT_CONDITION_EQUAL_NOT ? WC.IntCondition.INT_CONDITION_EQUAL_NOT : WC.IntCondition.INT_CONDITION_EQUAL;
        value = active.value;
        _syncing = false;
    }

    function commitToFilter() {
        if (!filter || _syncing)
            return;
        subfilter.condition = condition;
        subfilter.value = value;
        filter.intFilter = subfilter;
    }

    onFilterChanged: syncFromFilter()
    onConditionChanged: commitToFilter()
    onValueChanged: commitToFilter()
}
