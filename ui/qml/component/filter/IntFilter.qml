pragma ComponentBehavior: Bound
import QtQml
import QtQuick
import waywallen.control as WC
import Qcm.Material as MD

QtObject {
    id: root
    property var filter: null
    property int value: 0
    property int condition: WC.IntCondition.INT_CONDITION_UNSPECIFIED
    property WC.wallpaperIntFilter subfilter
    property bool _syncing: false

    readonly property var conditionModel: [
        { name: qsTr("equal"),         value: WC.IntCondition.INT_CONDITION_EQUAL },
        { name: qsTr("not equal"),     value: WC.IntCondition.INT_CONDITION_EQUAL_NOT },
        { name: qsTr("less"),          value: WC.IntCondition.INT_CONDITION_LESS },
        { name: qsTr("less equal"),    value: WC.IntCondition.INT_CONDITION_LESS_EQUAL },
        { name: qsTr("greater"),       value: WC.IntCondition.INT_CONDITION_GREATER },
        { name: qsTr("greater equal"), value: WC.IntCondition.INT_CONDITION_GREATER_EQUAL }
    ]

    readonly property Component valueDelegate: Component {
        MD.StepperInput {
            visible: root.condition !== WC.IntCondition.INT_CONDITION_UNSPECIFIED
            implicitHeight: 32
            valueWidth: 64
            from: -2147483648
            to: 2147483647
            step: 1
            decimals: 0
            value: root.value
            onValueChangeRequested: value => root.value = value
        }
    }

    function syncFromFilter() {
        if (!filter)
            return;
        if (!filter.hasIntFilter)
            filter.intFilter = subfilter;
        const active = filter.hasIntFilter ? filter.intFilter : subfilter;
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
        filter.intFilter = subfilter;
    }

    onFilterChanged: syncFromFilter()
    onConditionChanged: commitToFilter()
    onValueChanged: commitToFilter()
}
