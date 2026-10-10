pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.control as WC

// The four choices sit in one row while it fits. In a narrower parent the
// second pair drops to its own line, so no button ends up past the edge.
Item {
    id: control

    property int value: WC.Flip.FLIP_NONE
    signal selected(int value)

    readonly property real rowWidth: m_first.implicitWidth + m_first.spacing + m_second.implicitWidth
    readonly property bool wrapped: width > 0 && width + 1 < rowWidth
    readonly property int rowSpacing: 4

    implicitWidth: rowWidth
    // Without this a layout keeps the item at its implicit width and it overflows.
    Layout.fillWidth: true
    implicitHeight: wrapped ? m_first.implicitHeight + rowSpacing + m_second.implicitHeight : m_first.implicitHeight

    MD.SegmentedButtonGroup {
        id: m_first

        size: MD.Enum.XS

        MD.SegmentedButton {
            text: qsTr("None")
            checked: !control.value || control.value === WC.Flip.FLIP_NONE
            onClicked: control.selected(WC.Flip.FLIP_NONE)
        }
        MD.SegmentedButton {
            position: control.wrapped ? MD.Enum.PosLast : MD.Enum.PosMiddle
            text: qsTr("Horizontal")
            checked: control.value === WC.Flip.FLIP_HORIZONTAL
            onClicked: control.selected(WC.Flip.FLIP_HORIZONTAL)
        }
    }
    MD.SegmentedButtonGroup {
        id: m_second

        x: control.wrapped ? 0 : m_first.width + m_first.spacing
        y: control.wrapped ? m_first.height + control.rowSpacing : 0
        size: MD.Enum.XS

        MD.SegmentedButton {
            position: control.wrapped ? MD.Enum.PosFirst : MD.Enum.PosMiddle
            text: qsTr("Vertical")
            checked: control.value === WC.Flip.FLIP_VERTICAL
            onClicked: control.selected(WC.Flip.FLIP_VERTICAL)
        }
        MD.SegmentedButton {
            text: qsTr("Both")
            checked: control.value === WC.Flip.FLIP_BOTH
            onClicked: control.selected(WC.Flip.FLIP_BOTH)
        }
    }
}
