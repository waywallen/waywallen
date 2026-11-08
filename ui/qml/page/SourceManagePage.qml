pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
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

    function editLibrary(library) {
        if (sourceQuery.querying || removeQuery.querying)
            return;
        const source = (sourceQuery.sources || []).find(source => source.name === library.pluginName);
        root.MD.MProp.page.pushItem('waywallen.ui/EditLibraryPage', {
            libraryId: library.id,
            originalName: library.name,
            initialValues: Object.assign({}, library.settings),
            schemas: source ? source.librarySettings : []
        });
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

    W.SourceListQuery {
        id: sourceQuery
    }
    Component.onCompleted: sourceQuery.reload()
    Connections {
        target: W.Notify
        function onDaemonReady() {
            sourceQuery.reload();
        }
        function onPluginChanged() {
            sourceQuery.reload();
        }
    }

    contentItem: Item {
        implicitHeight: Math.max(m_view.contentHeight + m_view.topMargin + m_view.bottomMargin, emptyLabel.implicitHeight + 16)
        implicitWidth: m_view.implicitWidth

        MD.VerticalLazyList {
            id: m_view
            width: parent.width
            height: parent.height
            source: MD.ListSnapshotSource {
                items: W.App.libraryManager.libraries
            }
            spacing: 8

            extraTopMargin: 8
            extraBottomMargin: 8
            extraLeftMargin: 12
            extraRightMargin: 12

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
                        trailing: MD.ActionToolBar {
                            actions: [
                                MD.Action {
                                    enabled: root.visibilityReady
                                    icon.name: MD.Token.icon.close
                                    text: qsTr("Clear hidden state")
                                    onTriggered: root.libraryQuery.toggleLibraryHidden(missingSourceItem.modelData)
                                }
                            ]
                        }
                    }
                }
            }

            delegate: MD.ListItem {
                id: sourceItem
                required property MD.LazyRow row
                readonly property var modelData: row.value
                itemRow: row
                readonly property string fullPath: String(modelData.path || "")

                width: m_view.contentWidth
                radius: 8

                mdState.backgroundColor: MD.Token.color.surface_container
                onClicked: root.editLibrary(sourceItem.modelData)

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

                trailing: MD.ActionToolBar {
                    actions: [
                        MD.Action {
                            enabled: !sourceQuery.querying && !removeQuery.querying
                            icon.name: MD.Token.icon.edit
                            text: qsTr("Edit library")
                            onTriggered: root.editLibrary(sourceItem.modelData)
                        },
                        MD.Action {
                            visible: root.libraryQuery !== null
                            enabled: root.visibilityReady
                            checkable: true
                            checked: !root.hiddenIds.includes(sourceItem.modelData.id)
                            icon.name: checked ? MD.Token.icon.visibility : MD.Token.icon.visibility_off
                            text: checked ? qsTr("Hide source") : qsTr("Show source")
                            onTriggered: root.libraryQuery.toggleLibraryHidden(sourceItem.modelData.id)
                        },
                        MD.Action {
                            enabled: !removeQuery.querying
                            icon.name: MD.Token.icon.delete
                            text: qsTr("Remove library")
                            onTriggered: {
                                removeQuery.libraryId = sourceItem.modelData.id;
                                removeQuery.reload();
                            }
                        }
                    ]
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
