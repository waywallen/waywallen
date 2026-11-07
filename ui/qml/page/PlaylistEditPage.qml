pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import QExtra as QE
import Qcm.Material as MD
import waywallen.ui as W
import waywallen.control as WC

MD.Page {
    id: root

    required property double playlistId
    property int initialIntervalSecs: 300
    property WC.wallpaperSortRule emptySortRule
    readonly property int sortKey: previewQuery.sorts.length ? previewQuery.sorts[0].key : 0
    readonly property bool sortAsc: !previewQuery.sorts.length || previewQuery.sorts[0].direction !== WC.SortDirection.SORT_DIRECTION_DESC
    readonly property var sortOptions: [
        {
            name: qsTr("Default order"),
            key: 0
        },
        {
            name: qsTr("Name"),
            key: WC.WallpaperSortKey.WALLPAPER_SORT_KEY_NAME
        },
        {
            name: qsTr("Type"),
            key: WC.WallpaperSortKey.WALLPAPER_SORT_KEY_WP_TYPE
        },
        {
            name: qsTr("Size"),
            key: WC.WallpaperSortKey.WALLPAPER_SORT_KEY_SIZE
        },
        {
            name: qsTr("Last modified"),
            key: WC.WallpaperSortKey.WALLPAPER_SORT_KEY_LAST_MODIFIED
        }
    ]
    property bool smartDraft: false
    readonly property bool creating: playlistId <= 0
    readonly property bool smart: creating || smartDraft || playlist.sourceKind === "smart"
    property var baselineSelection
    property bool selectionDirty: false

    implicitWidth: 448
    implicitHeight: 600
    title: draftReady && draftName.length > 0 ? draftName : qsTr("Edit playlist")
    scrolling: !memberList.atYBeginning

    readonly property var playlist: detailQuery.playlist || ({})
    property bool draftReady: false
    property string baselineName: ""
    property int baselineMode: 1
    property int baselineIntervalSecs: 0
    property bool baselineSynchronizedSelection: true
    property string draftName: ""
    property int draftMode: 1
    property bool draftSynchronizedSelection: true
    property string draftHours: "0"
    property string draftMinutes: "0"
    property string draftSeconds: "0"
    property double draftRevision: 0
    property var baselineEntryIds: []
    property var draftEntryIds: []
    readonly property int draftIntervalInputSecs: (parseInt(draftHours) || 0) * 3600 + (parseInt(draftMinutes) || 0) * 60 + (parseInt(draftSeconds) || 0)
    readonly property bool intervalDirty: draftIntervalInputSecs !== baselineIntervalSecs
    readonly property int draftIntervalSecs: intervalDirty ? Math.max(10, draftIntervalInputSecs) : baselineIntervalSecs
    readonly property bool dirty: draftReady && (creating || selectionDirty || (smart && filterModel.dirty) || draftName !== baselineName || draftMode !== baselineMode || intervalDirty || draftSynchronizedSelection !== baselineSynchronizedSelection || !sameEntryIds(draftEntryIds, baselineEntryIds))
    readonly property bool canEdit: draftReady && (creating || detailQuery.status === 2) && !mutationQuery.querying

    Component.onCompleted: {
        tagListQuery.reload();
        ratingListQuery.reload();
        if (creating) {
            smartDraft = true;
            draftName = qsTr("Smart playlist");
            baselineName = draftName;
            baselineIntervalSecs = initialIntervalSecs;
            setDraftInterval(initialIntervalSecs);
            baselineSelection = previewQuery.selection;
            draftReady = true;
            previewQuery.reload();
        }
    }
    function syncFilterEditor() {
        filterModel.replaceState(previewQuery.filters, previewQuery.filterLogics);
    }

    function flushFilterDraft() {
        if (smart && filterModel.dirty)
            filterModel.apply();
    }

    function pickSort(key) {
        if (key === 0) {
            previewQuery.sorts = [];
            return;
        }
        const ascending = key === sortKey ? !sortAsc : sortAsc;
        const rule = emptySortRule;
        rule.key = key;
        rule.direction = ascending ? WC.SortDirection.SORT_DIRECTION_ASC : WC.SortDirection.SORT_DIRECTION_DESC;
        previewQuery.sorts = [rule];
    }

    function sameEntryIds(left, right) {
        if (left.length !== right.length)
            return false;
        for (let i = 0; i < left.length; ++i) {
            if (String(left[i]) !== String(right[i]))
                return false;
        }
        return true;
    }

    function setDraftInterval(seconds) {
        draftHours = String(Math.floor(seconds / 3600));
        draftMinutes = String(Math.floor((seconds % 3600) / 60));
        draftSeconds = String(seconds % 60);
    }

    function resetDraft() {
        if (creating) {
            draftName = baselineName;
            draftMode = baselineMode;
            draftSynchronizedSelection = baselineSynchronizedSelection;
            setDraftInterval(baselineIntervalSecs);
            previewQuery.restoreSelection(baselineSelection);
            syncFilterEditor();
            selectionDirty = false;
            return;
        }
        const snapshot = detailQuery.playlist || ({});
        if (Number(snapshot.id || 0) <= 0)
            return;
        draftReady = false;
        baselineName = String(snapshot.name || "");
        baselineMode = Number(snapshot.mode || 1);
        baselineIntervalSecs = Number(snapshot.intervalSecs || 0);
        baselineSynchronizedSelection = snapshot.synchronizedSelection !== false;
        draftName = baselineName;
        draftMode = baselineMode;
        draftSynchronizedSelection = baselineSynchronizedSelection;
        setDraftInterval(baselineIntervalSecs);
        baselineEntryIds = detailQuery.entryIds.slice();
        draftEntryIds = baselineEntryIds.slice();
        draftRevision = detailQuery.revision;
        if (snapshot.sourceKind === "smart") {
            baselineSelection = snapshot.smartSelection;
            previewQuery.restoreSelection(baselineSelection);
            syncFilterEditor();
            selectionDirty = false;
            previewQuery.reload();
        }
        draftReady = true;
    }

    function acceptDraft() {
        const acceptedIntervalSecs = draftIntervalSecs;
        baselineName = draftName;
        baselineMode = draftMode;
        baselineIntervalSecs = acceptedIntervalSecs;
        baselineSynchronizedSelection = draftSynchronizedSelection;
        setDraftInterval(baselineIntervalSecs);
        baselineEntryIds = draftEntryIds.slice();
        baselineSelection = previewQuery.selection;
        selectionDirty = false;
    }

    function removeDraftEntry(entryId) {
        const index = draftEntryIds.indexOf(entryId);
        if (index < 0)
            return;
        const next = draftEntryIds.slice();
        next.splice(index, 1);
        draftEntryIds = next;
    }

    function applyDraft() {
        if (!canEdit)
            return;
        flushFilterDraft();
        if (!dirty)
            return;
        if (smart)
            mutationQuery.saveSmart(playlistId, draftName, draftMode, draftIntervalSecs, draftSynchronizedSelection, previewQuery.selection, draftRevision);
        else
            mutationQuery.update(playlistId, draftName, draftMode, draftIntervalSecs, draftSynchronizedSelection, draftEntryIds, draftRevision);
    }

    MD.Action {
        id: resetAction

        text: qsTr("Reset")
        icon.name: MD.Token.icon.restart_alt
        enabled: root.canEdit && root.dirty
        onTriggered: root.resetDraft()
    }

    MD.Action {
        id: applyAction

        text: qsTr("Apply")
        icon.name: MD.Token.icon.check
        enabled: root.canEdit && root.dirty
        busy: mutationQuery.querying
        onTriggered: root.applyDraft()
    }

    actions: [resetAction, applyAction]

    // Renderer-declared labels for the wallpaper type keys shown per row.
    W.RendererPluginListQuery {
        id: pluginQuery
    }

    W.WallpaperListQuery {
        id: previewQuery
        limit: 50
        forwardError: false
        onSelectionChanged: root.selectionDirty = root.draftReady && !matchesSelection(root.baselineSelection)
    }

    W.WallpaperFilterRuleModel {
        id: filterModel
        onApply: previewQuery.replaceFilterState(items(), filterLogics)
        onDirtyChanged: if (dirty)
            Qt.callLater(root.flushFilterDraft)
    }

    W.TagListQuery {
        id: tagListQuery
    }

    W.ContentRatingListQuery {
        id: ratingListQuery
    }

    W.PlaylistDetailQuery {
        id: detailQuery

        playlistId: root.playlistId
        forwardError: false
        onPlaylistChanged: {
            if (!root.dirty)
                root.resetDraft();
        }
    }

    W.PlaylistMutationQuery {
        id: mutationQuery

        forwardError: false
        onDone: {
            if (mutationQuery.status === 3) {
                W.Action.toast(mutationQuery.error || qsTr("Playlist update failed"), 6000, 1, null);
                detailQuery.delayReload();
                return;
            }
            root.acceptDraft();
            if (root.creating)
                root.playlistId = mutationQuery.createdId;
            W.Action.toast(qsTr("Playlist updated"));
            detailQuery.reload();
        }
    }

    Connections {
        target: W.Notify
        function onPlaylistChanged() {
            if (!root.creating)
                detailQuery.delayReload();
            if (root.smart)
                previewQuery.delayReload();
        }
        function onWallpaperSyncFinished() {
            if (root.smart) {
                previewQuery.delayReload();
                tagListQuery.reload();
                ratingListQuery.reload();
            }
        }
        function onWallpaperHiddenChanged() {
            if (root.smart)
                previewQuery.delayReload();
        }
    }

    contentItem: Item {
        implicitWidth: root.implicitWidth
        implicitHeight: root.implicitHeight

        MD.VerticalLazyList {
            id: memberList

            Connections {
                target: memberSource.model
                function onHasMoreChanged() {
                    memberSource.notifyFetchStateChanged();
                }
            }

            anchors.fill: parent
            clip: true
            source: MD.ItemModelSource {
                id: memberSource
                model: root.smart ? previewQuery.data : detailQuery.data
                keyRole: "itemKey"
            }
            spacing: 0
            extraTopMargin: 8
            extraBottomMargin: 20
            extraLeftMargin: 16
            extraRightMargin: 16

            header: ColumnLayout {
                width: memberList.contentWidth
                spacing: 12

                MD.TextField {
                    Layout.fillWidth: true
                    mdState.size: MD.Enum.S
                    enabled: root.canEdit
                    placeholderText: qsTr("Name")
                    text: root.draftName
                    onTextEdited: root.draftName = text
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    visible: root.smart
                    enabled: root.canEdit
                    spacing: 8

                    MD.Text {
                        Layout.fillWidth: true
                        text: qsTr("Wallpapers matching these filters are updated automatically.")
                        wrapMode: Text.WordWrap
                    }

                    RowLayout {
                        Layout.fillWidth: true

                        MD.Label {
                            Layout.fillWidth: true
                            text: qsTranslate("WallpaperFilterDialog", "Rules")
                            typescale: MD.Token.typescale.title_medium
                        }

                        MD.IconButton {
                            icon.name: MD.Token.icon.clear_all
                            onClicked: {
                                filterModel.removeRows(0, filterModel.rowCount());
                                filterModel.filterLogics = [];
                            }
                            MD.ToolTip.text: qsTranslate("WallpaperFilterDialog", "Clear rules")
                            MD.ToolTip.visible: mdState.interactions.hovered
                        }

                        MD.IconButton {
                            icon.name: MD.Token.icon.add_box
                            onClicked: filterModel.appendNewGroup()
                            MD.ToolTip.text: qsTranslate("WallpaperFilterDialog", "Add group")
                            MD.ToolTip.visible: mdState.interactions.hovered
                        }
                    }

                    W.WallpaperFilterRules {
                        Layout.fillWidth: true
                        ruleBackgroundColor: MD.Token.color.surface_container
                        implicitHeight: contentHeight
                        interactive: false
                        model: filterModel
                        popupWindow: root.Window.window
                        supportedTypes: pluginQuery.supportedTypes || []
                        typeLabels: pluginQuery.typeLabels || ({})
                        allTags: tagListQuery.tags
                        allContentRatings: ratingListQuery.ratings
                    }
                }

                Flow {
                    Layout.fillWidth: true
                    spacing: 12

                    ColumnLayout {
                        width: implicitWidth
                        spacing: 4

                        MD.Text {
                            text: qsTr("Mode")
                            typescale: MD.Token.typescale.label_medium
                            color: MD.Token.color.on_surface_variant
                        }

                        MD.SegmentedButtonGroup {
                            Layout.fillWidth: true
                            size: MD.Enum.XS
                            enabled: root.canEdit

                            MD.SegmentedButton {
                                text: qsTr("Sequential")
                                checked: root.draftMode === 1
                                onCheckedChangeRequested: root.draftMode = 1
                            }

                            MD.SegmentedButton {
                                text: qsTr("Shuffle")
                                checked: root.draftMode === 2
                                onCheckedChangeRequested: root.draftMode = 2
                            }

                            MD.SegmentedButton {
                                text: qsTr("Random")
                                checked: root.draftMode === 3
                                onCheckedChangeRequested: root.draftMode = 3
                            }
                        }
                    }

                    ColumnLayout {
                        width: implicitWidth
                        spacing: 4

                        MD.Text {
                            text: qsTr("Rotation interval")
                            typescale: MD.Token.typescale.label_medium
                            color: MD.Token.color.on_surface_variant
                        }

                        RowLayout {
                            id: intervalRow

                            Layout.fillWidth: true
                            spacing: 6

                            MD.TextField {
                                id: hoursField

                                Layout.preferredWidth: 48
                                mdState.size: MD.Enum.XS
                                enabled: root.canEdit
                                inputMethodHints: Qt.ImhDigitsOnly
                                validator: IntValidator {
                                    bottom: 0
                                    top: 999
                                }
                                text: root.draftHours
                                onTextEdited: root.draftHours = text
                            }

                            MD.Text {
                                text: qsTr("h", "hours")
                                typescale: MD.Token.typescale.body_small
                                color: MD.Token.color.on_surface_variant
                            }

                            MD.TextField {
                                id: minutesField

                                Layout.preferredWidth: 48
                                mdState.size: MD.Enum.XS
                                enabled: root.canEdit
                                inputMethodHints: Qt.ImhDigitsOnly
                                validator: IntValidator {
                                    bottom: 0
                                    top: 59
                                }
                                text: root.draftMinutes
                                onTextEdited: root.draftMinutes = text
                            }

                            MD.Text {
                                text: qsTr("m", "minutes")
                                typescale: MD.Token.typescale.body_small
                                color: MD.Token.color.on_surface_variant
                            }

                            MD.TextField {
                                id: secondsField

                                Layout.preferredWidth: 48
                                mdState.size: MD.Enum.XS
                                enabled: root.canEdit
                                inputMethodHints: Qt.ImhDigitsOnly
                                validator: IntValidator {
                                    bottom: 0
                                    top: 59
                                }
                                text: root.draftSeconds
                                onTextEdited: root.draftSeconds = text
                            }

                            MD.Text {
                                text: qsTr("s", "seconds")
                                typescale: MD.Token.typescale.body_small
                                color: MD.Token.color.on_surface_variant
                            }
                        }
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 12

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        MD.Text {
                            text: qsTr("Synchronized selection")
                            typescale: MD.Token.typescale.body_large
                            color: MD.Token.color.on_surface
                        }

                        MD.Text {
                            Layout.fillWidth: true
                            text: root.draftMode === 1 ? qsTr("Sequential mode always uses the same wallpaper on every target") : qsTr("Use the same wallpaper on every target; rotation timing stays synchronized when disabled")
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                        }
                    }

                    MD.Switch {
                        enabled: root.canEdit && root.draftMode !== 1
                        checked: root.draftMode === 1 || root.draftSynchronizedSelection
                        onCheckedChangeRequested: checked => root.draftSynchronizedSelection = checked
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    spacing: 8

                    MD.Text {
                        text: qsTr("Wallpapers")
                        typescale: MD.Token.typescale.title_small
                        color: MD.Token.color.on_surface
                    }

                    MD.EmbedChip {
                        id: sortChip
                        visible: root.smart
                        enabled: root.canEdit
                        text: (root.sortOptions.find(option => option.key === root.sortKey) || root.sortOptions[0]).name
                        trailingIconName: root.sortKey === 0 ? "" : (root.sortAsc ? MD.Token.icon.arrow_downward : MD.Token.icon.arrow_upward)
                        mdState.borderWidth: 1
                        onClicked: sortMenu.open()

                        MD.Menu {
                            id: sortMenu
                            parent: sortChip
                            y: parent.height
                            model: root.sortOptions
                            contentDelegate: MD.MenuItem {
                                required property var modelData
                                text: modelData.name
                                icon.name: modelData.key === root.sortKey ? (root.sortKey === 0 ? MD.Token.icon.check : (root.sortAsc ? MD.Token.icon.arrow_downward : MD.Token.icon.arrow_upward)) : ' '
                                onClicked: {
                                    root.pickSort(modelData.key);
                                    sortMenu.close();
                                }
                            }
                        }
                    }

                    Item {
                        Layout.fillWidth: true
                    }

                    MD.Text {
                        text: String(root.smart ? previewQuery.total : root.draftEntryIds.length)
                        typescale: MD.Token.typescale.label_large
                        color: MD.Token.color.on_surface_variant
                    }
                }

                MD.Divider {
                    Layout.fillWidth: true
                    Layout.bottomMargin: 8
                }
            }

            footer: ColumnLayout {
                width: memberList.contentWidth
                spacing: 8

                MD.BusyIndicator {
                    Layout.alignment: Qt.AlignHCenter
                    visible: running
                    running: (root.smart ? previewQuery.querying : detailQuery.querying) && memberList.count === 0
                }

                MD.Text {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.fillWidth: true
                    visible: root.smart ? previewQuery.status === 2 && previewQuery.total === 0 : detailQuery.status === 2 && root.draftEntryIds.length === 0
                    text: root.smart ? qsTr("Waiting for matching wallpapers") : qsTr("No wallpapers in this playlist")
                    typescale: MD.Token.typescale.body_large
                    color: MD.Token.color.on_surface_variant
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                }

                MD.Text {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.fillWidth: true
                    visible: root.smart ? previewQuery.status === 3 : detailQuery.status === 3
                    text: (root.smart ? previewQuery.error : detailQuery.error) || qsTr("Playlist unavailable")
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.error
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                }
            }

            delegate: Item {
                id: wallpaperDelegate
                required property MD.LazyRow row
                readonly property var model: row.value

                readonly property string entryId: String(model.id_proto || "")
                readonly property bool retained: root.smart || root.draftEntryIds.indexOf(entryId) >= 0
                width: memberList.contentWidth
                implicitHeight: retained ? wallpaperItem.implicitHeight + 6 : 0
                visible: retained

                MD.ListItem {
                    id: wallpaperItem

                    itemRow: wallpaperDelegate.row
                    width: parent.width
                    height: implicitHeight
                    radius: 10
                    text: model.name || qsTr("Untitled")
                    supportText: (root.smart ? "" : String(root.draftEntryIds.indexOf(wallpaperDelegate.entryId) + 1) + " · ") + W.I18n.valueLabel(pluginQuery.typeLabels, model.wpType)
                    heightMode: MD.Enum.ListItemTwoLine
                    wrapMode: Text.Wrap
                    elide: Text.ElideRight
                    maximumLineCount: 2
                    mdState.backgroundColor: MD.Token.color.surface_container

                    leader: W.ThumbnailImage {
                        loadingMode: !wallpaperDelegate.row.active || wallpaperDelegate.row.usage === MD.LazyRow.Pooled ? QE.Image.Suspended : wallpaperDelegate.row.usage === MD.LazyRow.Cached ? QE.Image.Prefetch : QE.Image.Automatic
                        preparationScope: wallpaperDelegate.row.view
                        implicitWidth: 72
                        implicitHeight: 48
                        source: wallpaperDelegate.model.preview || ""
                        resource: wallpaperDelegate.model.resource || ""
                        wpType: wallpaperDelegate.model.wpType || ""
                        timeline: (wallpaperDelegate.model.subType || 0) === 1
                        fillMode: Image.PreserveAspectCrop
                    }

                    trailing: MD.IconButton {
                        visible: !root.smart
                        mdState.size: MD.Enum.XS
                        enabled: root.canEdit
                        icon.name: MD.Token.icon.delete
                        onClicked: root.removeDraftEntry(wallpaperDelegate.entryId)
                        MD.ToolTip.visible: mdState.interactions.hovered
                        MD.ToolTip.text: qsTr("Remove from playlist")
                    }
                }
            }
        }
    }
}
