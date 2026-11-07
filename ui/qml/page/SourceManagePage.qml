pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    title: qsTr("Library Manager")
    implicitWidth: 480
    // Bind locally; Qt.binding values in asynchronous initial properties can crash Qt.
    property QtObject visibilityState: null
    readonly property W.WallpaperListQuery libraryQuery: visibilityState ? visibilityState.query : null
    readonly property bool visibilityReady: visibilityState !== null && visibilityState.ready
    readonly property var hiddenIds: libraryQuery ? Array.from(libraryQuery.hiddenLibraryIds) : []
    readonly property var missingIds: {
        const libraries = Array.from(W.App.libraryManager.libraries);
        if (!libraryQuery || !W.App.libraryManager.initialized)
            return [];
        return hiddenIds.filter(id => !libraries.some(library => library.id === id));
    }

    actions: [
        MD.Action {
            icon.name: MD.Token.icon.image_search
            text: qsTr("Auto detect libraries")
            enabled: !autoDetectQuery.querying
            onTriggered: autoDetectQuery.reload()
        },
        MD.Action {
            icon.name: MD.Token.icon.add
            text: qsTr("Add library")
            onTriggered: root.MD.MProp.page.pushItem('waywallen.ui/AddLibraryPage')
        }
    ]

    W.LibraryAutoDetectQuery {
        id: autoDetectQuery
        onFinished: {
            if (addedCount === 0)
                W.Action.toast(qsTr("No new libraries found"));
        }
    }

    W.LibraryRemoveQuery {
        id: removeQuery
    }

    W.SourceListQuery { id: sourceQuery }
    Component.onCompleted: sourceQuery.reload()
    Connections {
        target: W.Notify
        function onDaemonReady() { sourceQuery.reload(); }
        function onPluginChanged() { sourceQuery.reload(); }
    }

    W.LibraryEditQuery {
        id: editQuery
        forwardError: false
        onErrorOccurred: function (error) {
            editDialog.errorText = error;
        }
        onFinished: editDialog.close()
    }

    MD.Dialog {
        id: editDialog
        property string pluginName: ""
        property string errorText: ""
        property string originalName: ""
        property var originalValues: ({})
        property var values: ({})
        readonly property var schemas: {
            const sources = sourceQuery.sources || [];
            for (const source of sources)
                if (source.name === pluginName) return source.librarySettings;
            return [];
        }
        readonly property bool hasChanges: nameField.text.trim() !== originalName
            || Object.keys(values).some(key => values[key] !== originalValues[key])
        readonly property bool canApply: !editQuery.querying
            && nameField.text.trim().length > 0 && hasChanges

        title: qsTr("Edit library")
        parent: MD.Overlay.overlay
        modal: true
        horizontalPadding: 24
        implicitWidth: Math.min(400, parent ? parent.width - 48 : 400)
        standardButtons: MD.Dialog.Cancel | MD.Dialog.Apply

        function openFor(library) {
            editQuery.libraryId = library.id;
            pluginName = library.pluginName;
            originalName = library.name;
            nameField.text = library.name;
            const next = Object.assign({}, library.settings);
            for (const schema of schemas)
                next[schema.key] = next[schema.key] ?? schema.default_value;
            originalValues = Object.assign({}, next);
            values = next;
            errorText = "";
            open();
        }

        function submit() {
            if (!canApply) return;
            errorText = "";
            editQuery.name = nameField.text.trim();
            editQuery.settings = values;
            editQuery.reload();
        }

        Component.onCompleted: {
            const apply = standardButton(MD.Dialog.Apply);
            if (apply) {
                apply.text = qsTr("Apply");
                apply.enabled = Qt.binding(function () { return editDialog.canApply; });
            }
        }
        onOpened: nameField.forceActiveFocus()
        onApplied: submit()
        contentItem: ColumnLayout {
            spacing: 12
            MD.TextField {
                id: nameField
                Layout.fillWidth: true
                mdState.size: MD.Enum.S
                placeholderText: qsTr("Library name")
                enabled: !editQuery.querying
                onTextEdited: editDialog.errorText = ""
                onAccepted: editDialog.submit()
            }
            W.LibrarySettingsFields {
                Layout.fillWidth: true
                visible: editDialog.schemas.length > 0
                enabled: !editQuery.querying
                schemas: editDialog.schemas
                values: editDialog.values
                onCommitted: (key, value) => {
                    const next = Object.assign({}, editDialog.values);
                    next[key] = value;
                    editDialog.values = next;
                    editDialog.errorText = "";
                }
            }
            MD.Text {
                Layout.fillWidth: true
                visible: text.length > 0
                text: editDialog.errorText
                color: MD.Token.color.error
                wrapMode: Text.WordWrap
            }
        }
    }

    contentItem: Item {
        implicitHeight: Math.max(m_view.contentHeight, emptyLabel.implicitHeight) + 16
        implicitWidth: m_view.implicitWidth

        MD.VerticalListView {
            id: m_view
            width: parent.width
            height: parent.height
            model: W.App.libraryManager.libraries
            spacing: 8

            topMargin: 8
            bottomMargin: 8
            leftMargin: 12
            rightMargin: 12

            footer: Column {
                width: m_view.contentWidth
                spacing: 8
                Repeater {
                    model: root.missingIds
                    delegate: MD.ListItem {
                        id: missingSourceItem
                        required property var modelData
                        width: m_view.contentWidth
                        text: qsTr("Removed source #%1").arg(modelData)
                        trailing: MD.IconButton {
                            enabled: root.visibilityReady
                            icon.name: MD.Token.icon.close
                            Accessible.name: qsTr("Clear hidden state")
                            MD.ToolTip.text: Accessible.name
                            MD.ToolTip.visible: mdState.interactions.hovered
                            onClicked: root.libraryQuery.toggleLibraryHidden(missingSourceItem.modelData)
                        }
                    }
                }
            }

            delegate: MD.ListItem {
                id: sourceItem
                required property var modelData
                readonly property string fullPath: String(modelData.path || "")

                width: m_view.contentWidth
                radius: 8

                mdState.backgroundColor: MD.Token.color.surface_container

                text: modelData.name
                supportText: qsTr("Plugin: %1").arg(modelData.pluginName)
                wrapMode: Text.Wrap
                maximumLineCount: 3
                below: MD.Text {
                    text: sourceItem.fullPath
                    wrapMode: Text.Wrap
                    typescale: MD.Token.typescale.body_medium
                    color: sourceItem.mdState.supportTextColor
                }

                leader: MD.Icon {
                    name: MD.Token.icon.folder
                    color: MD.Token.color.on_surface_variant
                }

                trailing: Row {
                    spacing: 4
                    MD.IconButton {
                        enabled: !editQuery.querying && !sourceQuery.querying
                        icon.name: MD.Token.icon.edit
                        Accessible.name: qsTr("Edit library")
                        MD.ToolTip.text: Accessible.name
                        MD.ToolTip.visible: mdState.interactions.hovered
                        onClicked: editDialog.openFor(sourceItem.modelData)
                    }
                    MD.IconToggleButton {
                        visible: root.libraryQuery !== null
                        enabled: root.visibilityReady
                        checked: !root.hiddenIds.includes(sourceItem.modelData.id)
                        icon.name: checked ? MD.Token.icon.visibility : MD.Token.icon.visibility_off
                        Accessible.name: checked ? qsTr("Hide source") : qsTr("Show source")
                        MD.ToolTip.text: Accessible.name
                        MD.ToolTip.visible: mdState.interactions.hovered
                        onCheckedChangeRequested: root.libraryQuery.toggleLibraryHidden(sourceItem.modelData.id)
                    }
                    MD.IconButton {
                        icon.name: MD.Token.icon.delete
                        Accessible.name: qsTr("Remove library")
                        MD.ToolTip.text: Accessible.name
                        MD.ToolTip.visible: mdState.interactions.hovered
                        onClicked: {
                            removeQuery.libraryId = sourceItem.modelData.id;
                            removeQuery.reload();
                        }
                    }
                }
            }
        }

        MD.Text {
            id: emptyLabel
            visible: W.App.libraryManager.initialized && W.App.libraryManager.count === 0 && root.missingIds.length === 0
            anchors.centerIn: parent
            width: Math.max(0, parent.width - 24)
            text: qsTr("No libraries")
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.on_surface_variant
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
        }
    }
}
