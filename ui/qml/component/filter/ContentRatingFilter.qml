pragma ComponentBehavior: Bound
import QtQml
import QtQuick
import waywallen.control as WC
import waywallen.ui as W
import Qcm.Material as MD

// Content-rating rule. Single value matched against item.content_rating.
QtObject {
    id: root
    property var filter: null
    property string value: ""
    property int condition: WC.StringCondition.STRING_CONDITION_UNSPECIFIED
    property WC.wallpaperStringFilter subfilter
    // Available content-rating values, supplied by the host for the menu.
    property var allRatings: []
    // Source-declared labels for those values, display only.
    property var valueLabels: ({})
    property bool _syncing: false

    readonly property var conditionModel: [
        { name: qsTr("is"),     value: WC.StringCondition.STRING_CONDITION_IS },
        { name: qsTr("is not"), value: WC.StringCondition.STRING_CONDITION_IS_NOT },
        { name: qsTr("any"),    value: WC.StringCondition.STRING_CONDITION_UNSPECIFIED }
    ]

    readonly property var ratingOptions: {
        const src = allRatings && allRatings.length > 0
                  ? allRatings
                  : ["Everyone", "Questionable", "Mature"];
        return src.map(r => ({ name: W.I18n.valueLabel(root.valueLabels, r), value: r }));
    }

    readonly property Component valueDelegate: Component {
        MD.InputChip {
            id: valueChip
            visible: root.condition !== WC.StringCondition.STRING_CONDITION_UNSPECIFIED
            text: W.I18n.valueLabel(root.valueLabels, root.value)
            onClicked: valueMenu.open()

            MD.Menu {
                id: valueMenu
                parent: valueChip
                y: parent.height
                model: root.ratingOptions
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
