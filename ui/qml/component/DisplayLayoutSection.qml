pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

// Staged layout editor for one display + location; pushed by DisplaysPage's Apply.
ColumnLayout {
    id: section

    // --- inputs ---
    property var display: null // waywallen.Display
    property int location: 0 // 0 = desktop, 1 = lock screen (proto LayoutLocation)
    property string title: "Desktop"

    // Read the layout live off `display` so reseed() sees fresh values, not a stale binding.
    function readSource() {
        if (!display)
            return ({});
        return location === 1 ? (display.lockDisplayLayout || ({})) : (display.displayLayout || display.effectiveLayout || ({}));
    }
    readonly property var srcOverride: {
        if (!display)
            return ({});
        return location === 1 ? (display.lockLayoutOverride || ({})) : (display.layoutOverride || ({}));
    }
    // --- value tables (mirror proto FillMode / Rotation) ---
    readonly property var kFillModeValues: [1, 2, 3, 7]
    readonly property var kFillModeLabels: ["Stretch", "Fit", "Crop", "Center"]
    readonly property var kRotationValues: [1, 2, 3, 4]
    readonly property var kRotationLabels: ["0°", "90°", "180°", "270°"]
    function fillmodeIndex(v) {
        const i = kFillModeValues.indexOf(v);
        return i < 0 ? 0 : i;
    }
    function clampPercent(v) {
        return Math.max(0, Math.min(100, Math.round(Number(v) || 0)));
    }

    // --- staged vs. baseline ---
    property int baseFillmode: 3
    property int baseX: 50
    property int baseY: 50
    property int baseRotation: 1
    property int stFillmode: 3
    property int stX: 50
    property int stY: 50
    property int stRotation: 1

    readonly property bool hasOverride: {
        const o = srcOverride || ({});
        return o.fillmodeSet === true || o.locationSet === true || o.alignSet === true || o.rotationSet === true;
    }
    readonly property bool dirty: stFillmode !== baseFillmode || stX !== baseX || stY !== baseY || stRotation !== baseRotation
    readonly property bool locationEnabled: stFillmode !== 1 // not stretched

    function reseed() {
        const l = readSource();
        const fm = Number(l.fillmode || 0);
        baseFillmode = kFillModeValues.indexOf(fm) >= 0 ? fm : 3;
        baseX = clampPercent(l.locationX ?? 50);
        baseY = clampPercent(l.locationY ?? 50);
        const rot = Number(l.rotation || 0);
        baseRotation = kRotationValues.indexOf(rot) >= 0 ? rot : 1;
        stFillmode = baseFillmode;
        stX = baseX;
        stY = baseY;
        stRotation = baseRotation;
    }

    // Discard staged edits, back to the persisted values.
    function discard() {
        stFillmode = baseFillmode;
        stX = baseX;
        stY = baseY;
        stRotation = baseRotation;
    }

    // Push only changed fields so the rest keep inheriting the global default.
    function apply() {
        if (!display || !dirty)
            return;
        setQuery.name = display.name;
        setQuery.displayId = display.id;
        setQuery.location = location;
        setQuery.fillmodeSet = stFillmode !== baseFillmode;
        setQuery.fillmode = stFillmode;
        setQuery.locationSet = stX !== baseX || stY !== baseY;
        setQuery.locationX = stX;
        setQuery.locationY = stY;
        setQuery.alignSet = false;
        setQuery.rotationSet = stRotation !== baseRotation;
        setQuery.rotation = stRotation;
        setQuery.clearFillmode = false;
        setQuery.clearLocation = false;
        setQuery.clearAlign = false;
        setQuery.clearRotation = false;
        baseFillmode = stFillmode;
        baseX = stX;
        baseY = stY;
        baseRotation = stRotation;
        setQuery.reload();
    }

    // Clear this location's override, reverting to the global default.
    function revertToGlobal() {
        if (!display)
            return;
        discard();
        setQuery.name = display.name;
        setQuery.displayId = display.id;
        setQuery.location = location;
        setQuery.fillmodeSet = false;
        setQuery.locationSet = false;
        setQuery.alignSet = false;
        setQuery.rotationSet = false;
        setQuery.clearFillmode = true;
        setQuery.clearLocation = true;
        setQuery.clearAlign = true;
        setQuery.clearRotation = true;
        setQuery.reload();
    }

    spacing: 4

    W.DisplayLayoutSetQuery {
        id: setQuery
    }

    onDisplayChanged: reseed()
    Component.onCompleted: reseed()

    // Refresh the baseline on external updates, but never clobber a dirty edit.
    Connections {
        target: section.display
        enabled: !!section.display
        function onLayoutChanged() {
            if (section.location === 0 && !section.dirty)
                section.reseed();
        }
        function onLockLayoutChanged() {
            if (section.location === 1 && !section.dirty)
                section.reseed();
        }
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: 8

        MD.Text {
            text: section.title
            typescale: MD.Token.typescale.label_large
            color: MD.Token.color.on_surface
        }

        Item {
            Layout.fillWidth: true
        }

        MD.IconButton {
            mdState.size: MD.Enum.XS
            enabled: section.hasOverride
            icon.name: MD.Token.icon.refresh
            MD.ToolTip {
                visible: parent.hovered
                text: "Revert to global default"
            }
            onClicked: section.revertToGlobal()
        }
    }

    Flow {
        Layout.fillWidth: true
        spacing: 12

        ColumnLayout {
            width: Math.min(parent.width, 220)
            spacing: 4

            MD.Text {
                text: "Fill mode"
                typescale: MD.Token.typescale.label_medium
                color: MD.Token.color.on_surface_variant
            }

            MD.ComboBox {
                id: fillmodeBox
                Layout.fillWidth: true
                model: section.kFillModeLabels
                onActivated: idx => section.stFillmode = section.kFillModeValues[idx]
            }
            // Restoring Binding keeps currentIndex on the staged value after the ComboBox writes it.
            Binding {
                target: fillmodeBox
                property: "currentIndex"
                value: section.fillmodeIndex(section.stFillmode)
            }
        }

        ColumnLayout {
            width: Math.min(parent.width, 260)
            spacing: 4
            enabled: section.locationEnabled
            opacity: enabled ? 1.0 : 0.4

            MD.Text {
                text: "Horizontal"
                typescale: MD.Token.typescale.label_medium
                color: MD.Token.color.on_surface_variant
            }

            W.ValueSlider {
                id: hSlider
                Layout.fillWidth: true
                from: 0
                to: 100
                stepSize: 1
                valueText: section.clampPercent(value)
                valueMaxText: "100"
                valueHorizontalAlignment: Text.AlignLeft
                onMoved: section.stX = section.clampPercent(value)
            }
            Binding {
                target: hSlider
                property: "value"
                value: section.stX
            }
        }

        ColumnLayout {
            width: Math.min(parent.width, 260)
            spacing: 4
            enabled: section.locationEnabled
            opacity: enabled ? 1.0 : 0.4

            MD.Text {
                text: "Vertical"
                typescale: MD.Token.typescale.label_medium
                color: MD.Token.color.on_surface_variant
            }

            W.ValueSlider {
                id: vSlider
                Layout.fillWidth: true
                from: 0
                to: 100
                stepSize: 1
                valueText: section.clampPercent(value)
                valueMaxText: "100"
                valueHorizontalAlignment: Text.AlignLeft
                onMoved: section.stY = section.clampPercent(value)
            }
            Binding {
                target: vSlider
                property: "value"
                value: section.stY
            }
        }

        ColumnLayout {
            width: Math.min(parent.width, implicitWidth)
            spacing: 4

            MD.Text {
                text: "Rotation"
                typescale: MD.Token.typescale.label_medium
                color: MD.Token.color.on_surface_variant
            }

            MD.SegmentedButtonGroup {
                size: MD.Enum.XS

                // checkable:false stops the ButtonGroup overwriting the staged `checked` binding.
                MD.SegmentedButton {
                    checkable: false
                    text: section.kRotationLabels[0]
                    checked: section.stRotation === section.kRotationValues[0]
                    onClicked: section.stRotation = section.kRotationValues[0]
                }
                MD.SegmentedButton {
                    checkable: false
                    text: section.kRotationLabels[1]
                    checked: section.stRotation === section.kRotationValues[1]
                    onClicked: section.stRotation = section.kRotationValues[1]
                }
                MD.SegmentedButton {
                    checkable: false
                    text: section.kRotationLabels[2]
                    checked: section.stRotation === section.kRotationValues[2]
                    onClicked: section.stRotation = section.kRotationValues[2]
                }
                MD.SegmentedButton {
                    checkable: false
                    text: section.kRotationLabels[3]
                    checked: section.stRotation === section.kRotationValues[3]
                    onClicked: section.stRotation = section.kRotationValues[3]
                }
            }
        }
    }
}
