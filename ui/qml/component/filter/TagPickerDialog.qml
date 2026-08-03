pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T
import Qcm.Material as MD

// Multi-select tag picker. `selected` seeds the current selection on open;
// edits are pending until Apply, which emits `commit(newTags)`.
MD.Dialog {
    id: control

    property var allTags: []
    property var tagLabels: []
    property var selected: []
    property string dialogTitle: qsTr("Select tags")
    signal commit(var tags)

    title: dialogTitle
    parent: T.Overlay.overlay
    horizontalPadding: 16
    implicitWidth: Math.min(330, parent ? parent.width - 48 : 330)
    standardButtons: T.Dialog.Cancel | T.Dialog.Reset | T.Dialog.Apply

    property var pending: []
    function togglePending(tag) {
        const next = (control.pending || []).slice();
        const i = next.indexOf(tag);
        if (i >= 0)
            next.splice(i, 1);
        else
            next.push(tag);
        control.pending = next;
    }
    function labelFor(tag) {
        const index = (control.allTags || []).indexOf(tag);
        return index >= 0 && index < (control.tagLabels || []).length
            ? String(control.tagLabels[index]) : String(tag);
    }

    // True when the pending selection actually differs from what was
    // seeded on open (order-independent). Reset/Apply only make sense then.
    function _hasChanges() {
        const a = control.pending || [];
        const b = control.selected || [];
        if (a.length !== b.length)
            return true;
        const as = a.slice().sort();
        const bs = b.slice().sort();
        for (let i = 0; i < as.length; ++i)
            if (as[i] !== bs[i])
                return true;
        return false;
    }

    // Gate Reset/Apply on whether there are pending changes. Bind directly
    // on the standard buttons the dialog builds for its button box.
    Component.onCompleted: {
        const apply = control.standardButton(T.Dialog.Apply);
        if (apply)
            apply.enabled = Qt.binding(control._hasChanges);
        const reset = control.standardButton(T.Dialog.Reset);
        if (reset)
            reset.enabled = Qt.binding(control._hasChanges);
    }

    onAboutToShow: control.pending = (control.selected || []).slice()
    onApplied: {
        control.commit(control.pending);
        control.accept();
    }
    onReset: control.pending = (control.selected || []).slice()

    contentItem: MD.VerticalFlickable {
        id: tagFlick
        contentWidth: width
        contentHeight: m_col.implicitHeight
        implicitHeight: Math.min(m_col.implicitHeight, 360)

        ColumnLayout {
            id: m_col
            width: tagFlick.contentWidth
            spacing: 8

            MD.Text {
                Layout.fillWidth: true
                visible: !control.allTags || control.allTags.length === 0
                text: qsTr("No tags in library")
                typescale: MD.Token.typescale.body_medium
                color: MD.Token.color.on_surface_variant
                wrapMode: Text.WordWrap
            }

            Flow {
                Layout.fillWidth: true
                visible: control.allTags && control.allTags.length > 0
                spacing: 8
                Repeater {
                    model: control.allTags
                    delegate: MD.FilterChip {
                        required property var modelData
                        checkable: false
                        text: control.labelFor(modelData)
                        checked: (control.pending || []).indexOf(modelData) >= 0
                        onClicked: control.togglePending(modelData)
                    }
                }
            }
        }
    }
}
