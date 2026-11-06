pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import waywallen.ui as W
import Qcm.Material as MD

MD.Dialog {
    id: root
    title: qsTr("Filters")
    required property var popupWindow
    horizontalPadding: 0
    implicitWidth: Math.min(440, parent ? parent.width - 48 : 440)
    standardButtons: MD.Dialog.Close

    property var filters: []
    property var selectedValues: []
    property var matchModes: ({})
    property var valuePresentation: null
    property var activeFilter: null
    property var confirmationFilter: null

    signal apply(var values)
    signal applyMatchMode(string filterId, string mode)

    function sanitize(values) {
        let out = [];
        let seen = {};
        for (const raw of values ?? []) {
            const value = String(raw);
            if (value.length === 0 || seen[value] === true)
                continue;
            seen[value] = true;
            out.push(value);
        }
        return out;
    }
    function filterValues(filter) {
        return sanitize((filter?.options ?? []).map(option => option.value));
    }
    function valueLabel(filter, value) {
        return W.I18n.optionLabel(filter?.options ?? [], value);
    }
    function selectedMap() {
        let allowed = {};
        for (const filter of filters ?? []) {
            for (const value of filterValues(filter))
                allowed[value] = true;
        }
        let out = {};
        for (const value of sanitize(selectedValues)) {
            if (allowed[value] === true)
                out[value] = true;
        }
        return out;
    }
    function selectedFor(filter) {
        const selected = selectedMap();
        return filterValues(filter).filter(value => selected[value] === true);
    }
    function hasFilterValue(filter) {
        return selectedFor(filter).length > 0;
    }
    function usesInlineValues(filter) {
        return Number(filter?.type ?? 0) === 2 && filterValues(filter).length <= 6;
    }
    function toggleFilterValue(filter, value) {
        const selected = selectedFor(filter);
        const index = selected.indexOf(value);
        if (index >= 0)
            selected.splice(index, 1);
        else
            selected.push(value);
        setFilterValues(filter, selected);
    }
    function collect(filter, nextValues) {
        let selected = selectedMap();
        for (const value of filterValues(filter))
            delete selected[value];
        const allowed = {};
        for (const value of filterValues(filter))
            allowed[value] = true;
        for (const value of sanitize(nextValues)) {
            if (allowed[value] === true)
                selected[value] = true;
        }
        let out = [];
        for (const item of filters ?? []) {
            for (const value of filterValues(item)) {
                if (selected[value] === true)
                    out.push(value);
            }
        }
        return out;
    }
    function setFilterValues(filter, values) {
        root.apply(collect(filter, values));
    }
    function selectOptions(filter) {
        return [qsTr("Any")].concat(filterValues(filter).map(value => valueLabel(filter, value)));
    }
    function selectIndex(filter) {
        const selected = selectedFor(filter);
        if (selected.length === 0)
            return 0;
        return Math.max(0, filterValues(filter).indexOf(selected[0]) + 1);
    }
    function openValueDialog(filter) {
        if (root.valuePresentation?.active)
            return;
        root.activeFilter = filter;
        const presentation = root.popupWindow.presentPopup(valueDialogComponent);
        root.valuePresentation = presentation;
        presentation.activeChanged.connect(presentation, function () {
            if (!presentation.active && root.valuePresentation === presentation) {
                root.valuePresentation = null;
                root.activeFilter = null;
            }
        });
        if (!presentation.active) {
            root.valuePresentation = null;
            root.activeFilter = null;
        }
    }
    function requestToggle(filter, enabled) {
        if (!enabled) {
            setFilterValues(filter, []);
            return;
        }
        const confirmation = W.I18n.tr(filter.confirmationText);
        if (confirmation.length > 0) {
            confirmationFilter = filter;
            m_confirm.open();
            return;
        }
        setFilterValues(filter, filterValues(filter).slice(0, 1));
    }

    onClosed: root.valuePresentation?.close()
    Component.onDestruction: root.valuePresentation?.cancel()

    Component {
        id: valueDialogComponent

        W.TagPickerDialog {
            dialogTitle: root.activeFilter ? W.I18n.tr(root.activeFilter.titleText) : qsTr("Select values")
            options: root.activeFilter?.options ?? []
            selected: root.selectedFor(root.activeFilter)
            onCommit: function (values) {
                root.setFilterValues(root.activeFilter, values);
            }
        }
    }

    contentItem: MD.VerticalFlickable {
        id: filterFlick
        leftMargin: 16
        rightMargin: 16
        topMargin: 4
        bottomMargin: 4
        contentHeight: filterColumn.implicitHeight
        implicitHeight: Math.min(filterColumn.implicitHeight, 520)

        ColumnLayout {
            id: filterColumn
            width: filterFlick.contentWidth
            spacing: 16

            Repeater {
                model: root.filters ?? []

                delegate: ColumnLayout {
                    id: filterRow
                    required property int index
                    required property var modelData
                    readonly property int filterType: Number(modelData.type ?? 0)
                    Layout.fillWidth: true
                    spacing: 6

                    MD.Divider {
                        Layout.fillWidth: true
                        visible: filterRow.index > 0
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        visible: filterRow.filterType === 1

                        MD.Label {
                            text: W.I18n.tr(filterRow.modelData.titleText)
                            typescale: MD.Token.typescale.title_small
                        }
                        MD.Label {
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: W.I18n.tr(filterRow.modelData.descriptionText)
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                        }
                        MD.ComboBox {
                            Layout.fillWidth: true
                            mdState.size: MD.Enum.S
                            popupMaximumHeight: 320
                            model: root.selectOptions(filterRow.modelData)
                            currentIndex: root.selectIndex(filterRow.modelData)
                            onSelectionChangeRequested: function (idx) {
                                const values = root.filterValues(filterRow.modelData);
                                root.setFilterValues(filterRow.modelData, idx > 0 ? [values[idx - 1]] : []);
                            }
                        }
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        visible: filterRow.filterType === 2

                        RowLayout {
                            Layout.fillWidth: true
                            MD.Label {
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                text: W.I18n.tr(filterRow.modelData.titleText)
                                typescale: MD.Token.typescale.title_small
                                elide: Text.ElideRight
                            }
                            MD.SegmentedButtonGroup {
                                size: MD.Enum.XS
                                visible: (filterRow.modelData.matchModes ?? []).length > 1

                                MD.SegmentedButton {
                                    text: qsTr("Any")
                                    visible: (filterRow.modelData.matchModes ?? []).indexOf("any") >= 0
                                    checked: root.matchModes[filterRow.modelData.id] === "any"
                                    onCheckedChangeRequested: root.applyMatchMode(filterRow.modelData.id, "any")
                                }
                                MD.SegmentedButton {
                                    text: qsTr("All")
                                    visible: (filterRow.modelData.matchModes ?? []).indexOf("all") >= 0
                                    checked: root.matchModes[filterRow.modelData.id] === "all"
                                    onCheckedChangeRequested: root.applyMatchMode(filterRow.modelData.id, "all")
                                }
                            }
                            MD.IconButton {
                                icon.name: MD.Token.icon.edit
                                visible: !root.usesInlineValues(filterRow.modelData)
                                onClicked: root.openValueDialog(filterRow.modelData)
                            }
                        }
                        MD.Label {
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: W.I18n.tr(filterRow.modelData.descriptionText)
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                        }
                        Flow {
                            Layout.fillWidth: true
                            visible: root.usesInlineValues(filterRow.modelData)
                            spacing: 8

                            Repeater {
                                model: filterRow.modelData.options ?? []
                                delegate: MD.FilterChip {
                                    required property var modelData
                                    text: root.valueLabel(filterRow.modelData, modelData.value)
                                    selected: root.selectedFor(filterRow.modelData).indexOf(modelData.value) >= 0
                                    onSelectionRequested: root.toggleFilterValue(filterRow.modelData, modelData.value)
                                }
                            }
                        }
                        Flow {
                            Layout.fillWidth: true
                            visible: !root.usesInlineValues(filterRow.modelData) && root.selectedFor(filterRow.modelData).length > 0
                            spacing: 6

                            Repeater {
                                model: root.selectedFor(filterRow.modelData)
                                delegate: W.Tag {
                                    required property var modelData
                                    text: root.valueLabel(filterRow.modelData, modelData)
                                }
                            }
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 12
                        visible: filterRow.filterType === 3

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            MD.Label {
                                text: W.I18n.tr(filterRow.modelData.titleText)
                                typescale: MD.Token.typescale.title_small
                            }
                            MD.Label {
                                Layout.fillWidth: true
                                visible: text.length > 0
                                text: W.I18n.tr(filterRow.modelData.descriptionText)
                                typescale: MD.Token.typescale.body_small
                                color: MD.Token.color.on_surface_variant
                                wrapMode: Text.WordWrap
                            }
                        }

                        MD.Switch {
                            id: toggleControl
                            checked: root.hasFilterValue(filterRow.modelData)
                            onCheckedChangeRequested: checked => root.requestToggle(filterRow.modelData, checked)
                        }
                    }
                }
            }
        }
    }

    MD.Dialog {
        id: m_confirm
        title: root.confirmationFilter ? W.I18n.tr(root.confirmationFilter.titleText) : ""
        modal: true
        positioningItem: overlayItem
        x: (overlayWidth - width) / 2
        y: (overlayHeight - height) / 2
        standardButtons: MD.Dialog.Cancel | MD.Dialog.Ok
        onAccepted: {
            root.setFilterValues(root.confirmationFilter, root.filterValues(root.confirmationFilter).slice(0, 1));
            root.confirmationFilter = null;
        }
        onRejected: root.confirmationFilter = null

        contentItem: MD.Label {
            text: root.confirmationFilter ? W.I18n.tr(root.confirmationFilter.confirmationText) : ""
            wrapMode: Text.WordWrap
        }
    }
}
