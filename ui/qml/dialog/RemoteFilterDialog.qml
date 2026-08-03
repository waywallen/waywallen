pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T
import waywallen.ui as W
import Qcm.Material as MD

MD.Dialog {
    id: root
    title: qsTr("Filters")
    required property var popupWindow
    horizontalPadding: 0
    implicitWidth: Math.min(440, parent ? parent.width - 48 : 440)
    standardButtons: T.Dialog.Close

    property var filters: []
    property var selectedValues: []
    property var valuePresentation: null
    property var activeFilter: null
    property var confirmationFilter: null

    signal apply(var values)

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
        return sanitize(filter && filter.values ? filter.values : []);
    }
    function filterLabels(filter) {
        const values = filterValues(filter);
        const labels = Array.from(filter && filter.valueLabels ? filter.valueLabels : [], label => String(label));
        return labels.length === values.length ? labels : values;
    }
    function labelFor(filter, value) {
        const index = filterValues(filter).indexOf(value);
        return index >= 0 ? filterLabels(filter)[index] : String(value);
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
        return [qsTr("Any")].concat(filterLabels(filter));
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
        const confirmation = String(filter.confirmation ?? "");
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
            dialogTitle: root.activeFilter ? String(root.activeFilter.title ?? "") : qsTr("Select values")
            allTags: root.filterValues(root.activeFilter)
            tagLabels: root.filterLabels(root.activeFilter)
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
                            text: String(filterRow.modelData.title ?? "")
                            typescale: MD.Token.typescale.title_small
                        }
                        MD.Label {
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: String(filterRow.modelData.description ?? "")
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                        }
                        MD.ComboBox {
                            Layout.fillWidth: true
                            popupMaximumHeight: 320
                            model: root.selectOptions(filterRow.modelData)
                            currentIndex: root.selectIndex(filterRow.modelData)
                            onActivated: function (idx) {
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
                                text: String(filterRow.modelData.title ?? "")
                                typescale: MD.Token.typescale.title_small
                            }
                            MD.IconButton {
                                icon.name: MD.Token.icon.edit
                                onClicked: root.openValueDialog(filterRow.modelData)
                            }
                        }
                        MD.Label {
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: String(filterRow.modelData.description ?? "")
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                        }
                        Flow {
                            Layout.fillWidth: true
                            visible: root.selectedFor(filterRow.modelData).length > 0
                            spacing: 6

                            Repeater {
                                model: root.selectedFor(filterRow.modelData)
                                delegate: W.Tag {
                                    required property var modelData
                                    text: root.labelFor(filterRow.modelData, modelData)
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
                                text: String(filterRow.modelData.title ?? "")
                                typescale: MD.Token.typescale.title_small
                            }
                            MD.Label {
                                Layout.fillWidth: true
                                visible: text.length > 0
                                text: String(filterRow.modelData.description ?? "")
                                typescale: MD.Token.typescale.body_small
                                color: MD.Token.color.on_surface_variant
                                wrapMode: Text.WordWrap
                            }
                        }

                        MD.Switch {
                            id: toggleControl
                            checked: root.hasFilterValue(filterRow.modelData)
                            onClicked: {
                                root.requestToggle(filterRow.modelData, !root.hasFilterValue(filterRow.modelData));
                                checked = Qt.binding(() => root.hasFilterValue(filterRow.modelData));
                            }
                        }
                    }
                }
            }
        }
    }

    MD.Dialog {
        id: m_confirm
        title: root.confirmationFilter ? String(root.confirmationFilter.title ?? "") : ""
        modal: true
        anchors.centerIn: T.Overlay.overlay
        standardButtons: T.Dialog.Cancel | T.Dialog.Ok
        onAccepted: {
            root.setFilterValues(root.confirmationFilter, root.filterValues(root.confirmationFilter).slice(0, 1));
            root.confirmationFilter = null;
        }
        onRejected: root.confirmationFilter = null

        contentItem: MD.Label {
            text: root.confirmationFilter ? String(root.confirmationFilter.confirmation ?? "") : ""
            wrapMode: Text.WordWrap
        }
    }
}
