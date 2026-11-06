pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQml as Qml
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.control as WC
import waywallen.ui as W

MD.Page {
    id: root

    property bool _filterStateSeeded: false

    // Quick filters (skip-types, tag filter) are seeded from settings
    // once; after that the local selection is authoritative. Re-adopting
    // them on every settings echo would revert a just-applied toggle
    // whenever the round-trip lags.
    property bool _quickFiltersSeeded: false
    property var currentWallpaperSelect: null
    readonly property int defaultPlaylistIntervalSecs: 300
    property WallpaperDetailPanel detailPanel: WallpaperDetailPanel {
        fallbackWallpaper: root.selectedWallpaper
        nestedScrollEnabled: sheetSite.current
        parent: null
        showApply: true
        typeLabels: pluginQuery.typeLabels || ({})
        valueLabels: root.sourceValueLabels
        wallpaperId: root.requestedWallpaperId.length > 0 ? root.requestedWallpaperId : (root.selectedWallpaper?.id_proto ?? "")

        onBack: {
            dismissAnchoredPopups();
            detailPresenter.dismiss();
        }
        onHiddenChanged: function (hidden) {
            const filter = wallpaperQuery.hiddenFilter;
            if ((hidden && filter === WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_EXCLUDE) || (!hidden && filter === WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_ONLY))
                back();
        }
    }
    property WC.wallpaperSortRule emptySortRule
    property var filterPresentation: null
    readonly property int hiddenSelectedWallpaperCount: root.currentWallpaperSelect ? root.currentWallpaperSelect.hiddenSelectedCount : 0
    property var hiddenWallpapersPresentation: null
    property bool hiddenWallpapersReturnToList: true
    property double playlistEditorId: 0
    property var playlistEditorPresentation: null
    property bool playlistEditorReturnsToList: true
    readonly property bool playlistListLoading: playlistListQuery.querying && !root.playlistListReady
    property bool playlistListReady: false
    property var playlistListSheet: null
    property string playlistMutationPendingMessage: ""
    property string playlistMutationSuccessMessage: ""
    readonly property int removableSelectedWallpaperCount: root.currentWallpaperSelect ? root.currentWallpaperSelect.removableSelectedCount : 0
    property string requestedWallpaperId: ""
    property var selectedWallpaper: null
    readonly property int selectedWallpaperCount: root.currentWallpaperSelect ? root.currentWallpaperSelect.selectedCount : 0
    readonly property bool selectionActionSheetActive: root.selectionActive && root.currentWallpaperSelect && (root.currentWallpaperSelect.actions || []).length > 0
    readonly property bool selectionActive: root.currentWallpaperSelect ? root.currentWallpaperSelect.active : false
    readonly property bool selectionAllHidden: root.selectedWallpaperCount > 0 && root.hiddenSelectedWallpaperCount === root.selectedWallpaperCount
    readonly property int selectionSheetReserve: wallpaperSelectSheetRelay.currentComponent ? 360 : 160
    property bool sortAsc: true
    property int sortIndex: 0
    readonly property var sortOptions: [
        {
            name: qsTr("Name"),
            key: WC.WallpaperSortKey.WALLPAPER_SORT_KEY_NAME
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
    readonly property var sourceValueLabels: root.buildSourceValueLabels(sourceFilterQuery.sources)
    property var wallpaperSelectSheet: null
    property var wallpaperTweakSheet: null

    function _buildSortRule() {
        const rule = emptySortRule;
        rule.key = sortOptions[sortIndex].key;
        rule.direction = sortAsc ? WC.SortDirection.SORT_DIRECTION_ASC : WC.SortDirection.SORT_DIRECTION_DESC;
        return rule;
    }
    // Guard: don't overwrite daemon state with proto defaults when the
    // local mirror of settings hasn't been populated yet. Without this,
    // a click that lands before filterSettingsGet's first response
    // ships a SettingsSet with only the touched field; the daemon then
    // resets target_extent to 0 and clears the filter on commit.
    function _persistGlobalChange(mutator) {
        if (Object.keys(filterSettingsGet.global).length === 0)
            return;
        const nextGlobal = Object.assign({}, filterSettingsGet.global);
        mutator(nextGlobal);
        if (root._quickFiltersSeeded)
            nextGlobal.wallpaperHiddenLibraryIds = wallpaperQuery.hiddenLibraryIds;
        filterSettingsSet.global = nextGlobal;
        filterSettingsSet.plugins = filterSettingsGet.plugins;
        filterSettingsSet.reload();
    }
    function addSelectionToPlaylist(playlist) {
        const ids = root.selectedWallpaperIds();
        if (ids.length === 0 || !playlist || playlistMutation.querying)
            return;

        const merged = (playlist.entryIds || []).slice();
        const seen = {};
        for (let i = 0; i < merged.length; ++i)
            seen[String(merged[i])] = true;
        for (let j = 0; j < ids.length; ++j) {
            const key = String(ids[j]);
            if (seen[key] !== true) {
                merged.push(ids[j]);
                seen[key] = true;
            }
        }
        root.playlistMutationSuccessMessage = qsTr("Added to playlist");
        playlistMutation.setItems(playlist.id, merged, Number(playlist.revision || 0));
    }
    function adoptWallpaperSelect(storage) {
        if (storage !== userWallpaperSelect && storage !== playlistWallpaperSelect)
            return;
        if (root.currentWallpaperSelect !== storage) {
            if (root.currentWallpaperSelect)
                root.currentWallpaperSelect.clear();
            root.currentWallpaperSelect = storage;
            wallpaperSelectSheetRelay.reset();
        }
        root.syncWallpaperSelectSheet();
    }
    function applyPlaylistSelection() {
        if (playlistWallpaperSelect.playlistId <= 0 || playlistItemsMutation.querying)
            return;
        playlistItemsMutation.setItems(playlistWallpaperSelect.playlistId, playlistWallpaperSelect.orderedWallpaperIds(), playlistWallpaperSelect.revision);
    }
    function applySort() {
        wallpaperQuery.sorts = [_buildSortRule()];
    }
    function beginPlaylistItemSelection(playlistId, revision, entryIds) {
        if (!(Number(playlistId) > 0))
            return;
        if (root.playlistEditorPresentation?.active) {
            root.playlistEditorReturnsToList = false;
            root.playlistEditorPresentation.cancel();
        }
        if (root.playlistListSheet?.active)
            root.playlistListSheet.close();
        root.enterWallpaperSelect(playlistWallpaperSelect);
        playlistWallpaperSelect.beginPlaylistItems(playlistId, revision, entryIds);
        root.selectedWallpaper = null;
        if (m_grid_view)
            m_grid_view.currentIndex = -1;
        if (m_grid_view)
            m_grid_view.forceActiveFocus();
        root.syncWallpaperSelectSheet();
    }
    function beginWallpaperSelection(index) {
        root.enterWallpaperSelect(userWallpaperSelect);
        const row = index === undefined ? -1 : Number(index);
        if (!userWallpaperSelect.begin(row))
            return;

        root.selectedWallpaper = null;
        if (m_grid_view)
            m_grid_view.currentIndex = -1;
        if (m_grid_view)
            m_grid_view.forceActiveFocus();
        root.syncWallpaperSelectSheet();
    }
    function buildSourceValueLabels(sources) {
        const labels = {};
        for (const source of sources ?? []) {
            for (const filter of source.filters ?? []) {
                for (const option of filter.options ?? []) {
                    const value = String(option.value ?? "");
                    if (value.length === 0 || labels[value] !== undefined)
                        continue;
                    // `labelText` is a plugin message, `label` a plain string.
                    const label = option.labelText ?? option.label;
                    if (label === undefined || label === null || label === "")
                        continue;
                    labels[value] = label;
                }
            }
        }
        return labels;
    }
    function cancelWallpaperSelection() {
        if (root.currentWallpaperSelect === playlistWallpaperSelect) {
            root.finishPlaylistItemSelection();
            return;
        }
        root.clearWallpaperSelection();
    }
    function clearWallpaperSelection() {
        if (root.currentWallpaperSelect)
            root.currentWallpaperSelect.clear();
        root.currentWallpaperSelect = null;
        wallpaperSelectSheetRelay.reset();
        root.syncWallpaperSelectSheet();
    }
    function createEmptyPlaylist() {
        if (playlistSheetCreateMutation.querying)
            return;

        playlistSheetCreateMutation.create(qsTr("New playlist"), WC.PlaylistMode.SEQUENTIAL, root.defaultPlaylistIntervalSecs, true, []);
    }
    function createPlaylistFromSelection(name) {
        const ids = root.selectedWallpaperIds();
        if (ids.length === 0 || playlistMutation.querying)
            return;

        const title = String(name || "").trim();
        playlistMutation.create(title.length > 0 ? title : qsTr("New playlist"), WC.PlaylistMode.SEQUENTIAL, root.defaultPlaylistIntervalSecs, true, ids);
    }
    function deletePlaylist(playlist) {
        if (!playlist || playlistMutation.querying)
            return;

        root.playlistMutationSuccessMessage = qsTr("Playlist deleted");
        playlistMutation.remove(playlist.id);
    }
    function ensurePlaylistListSheet() {
        if (root.playlistListSheet?.active)
            return root.playlistListSheet;

        const sheet = root.Window.window.presentPopup(playlistListSheetComponent);
        if (sheet.active) {
            root.playlistListSheet = sheet;
            sheet.activeChanged.connect(sheet, function () {
                if (!sheet.active)
                    root.releasePlaylistListSheet(sheet);
            });
        }
        return sheet;
    }
    function ensureWallpaperSelectSheet() {
        if (root.wallpaperSelectSheet?.active)
            return root.wallpaperSelectSheet;

        const sheet = root.Window.window.presentPopup(wallpaperSelectSheetComponent);
        if (sheet.active) {
            root.wallpaperSelectSheet = sheet;
            sheet.activeChanged.connect(sheet, function () {
                if (!sheet.active) {
                    root.releaseWallpaperSelectSheet(sheet);
                    if (root.selectionActionSheetActive)
                        root.syncWallpaperSelectSheet();
                }
            });
        }
        return sheet;
    }
    function ensureWallpaperTweakSheet() {
        if (root.wallpaperTweakSheet?.active)
            return root.wallpaperTweakSheet;

        const sheet = root.Window.window.presentPopup(wallpaperTweakSheetComponent);
        if (sheet.active) {
            root.wallpaperTweakSheet = sheet;
            sheet.activeChanged.connect(sheet, function () {
                if (!sheet.active)
                    root.releaseWallpaperTweakSheet(sheet);
            });
        }
        return sheet;
    }
    function enterWallpaperSelect(storage) {
        if (!storage)
            return;
        root.adoptWallpaperSelect(storage);
        W.Action.enterWallpaperSelect(storage);
    }
    function finishPlaylistItemSelection() {
        root.clearWallpaperSelection();
        root.showPlaylistListSheet();
    }
    function handleWallpaperClick(index, modifiers) {
        const model = wallpaperQuery.data;
        if (!model)
            return;

        if ((modifiers & Qt.ShiftModifier) !== 0) {
            const select = root.interactionWallpaperSelect();
            root.enterWallpaperSelect(select);
            const anchor = select.anchorIndex >= 0 ? select.anchorIndex : (m_grid_view.currentIndex >= 0 ? m_grid_view.currentIndex : index);
            select.selectRange(anchor, index, true);
            select.selectionMode = true;
            select.anchorIndex = anchor;
            root.selectedWallpaper = null;
            root.syncWallpaperSelectSheet();
            return;
        }

        if (root.selectionActive || (modifiers & Qt.ControlModifier) !== 0) {
            const select = root.interactionWallpaperSelect();
            root.enterWallpaperSelect(select);
            select.toggleSelected(index);
            root.selectedWallpaper = null;
            root.syncWallpaperSelectSheet();
            return;
        }

        m_grid_view.currentIndex = index;
        userWallpaperSelect.anchorIndex = index;
        root.requestedWallpaperId = "";
        root.selectedWallpaper = model.item(index);
        detailPresenter.present();
    }
    function hideSelectedWallpapers() {
        const select = root.currentWallpaperSelect;
        if (!select || selectionHideQuery.querying)
            return;

        const raw = select.selectedWallpaperIds();
        const ids = [];
        for (let i = 0; i < raw.length; ++i) {
            const id = String(raw[i] || "");
            if (id.length > 0)
                ids.push(id);
        }
        if (ids.length === 0)
            return;

        selectionHideQuery.setHiddenForIds(ids, !root.selectionAllHidden);
    }
    function interactionWallpaperSelect() {
        return root.currentWallpaperSelect && root.currentWallpaperSelect.active ? root.currentWallpaperSelect : userWallpaperSelect;
    }
    function isSheetActive(sheet) {
        return !!sheet && (sheet.status === MD.PopupPresentation.Opening || sheet.status === MD.PopupPresentation.Open);
    }
    function openHiddenWallpapers() {
        if (root.hiddenWallpapersPresentation?.active)
            return;
        root.playlistListSheet?.close();
        const presentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
            source: 'waywallen.ui/HiddenWallpapersPage'
        });
        root.hiddenWallpapersPresentation = presentation;
        presentation.activeChanged.connect(presentation, function () {
            if (presentation.active || root.hiddenWallpapersPresentation !== presentation)
                return;
            root.hiddenWallpapersPresentation = null;
            if (root.hiddenWallpapersReturnToList)
                root.showPlaylistListSheet();
        });
    }
    function openPlaylistEditor(playlist, createSmart = false) {
        const playlistId = Number(typeof playlist === "object" ? playlist?.id ?? 0 : playlist ?? 0);
        if (!(playlistId > 0) && !createSmart)
            return;
        if (root.playlistEditorPresentation?.active && root.playlistEditorId === playlistId)
            return;
        if (root.playlistEditorPresentation?.active) {
            root.playlistEditorReturnsToList = false;
            root.playlistEditorPresentation.cancel();
        }
        if (root.playlistListSheet?.active)
            root.playlistListSheet.close();

        const presentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
            source: 'waywallen.ui/PlaylistEditPage',
            props: {
                playlistId: playlistId,
                initialIntervalSecs: root.defaultPlaylistIntervalSecs
            }
        });
        root.playlistEditorPresentation = presentation;
        root.playlistEditorId = playlistId;
        root.playlistEditorReturnsToList = true;
        presentation.activeChanged.connect(presentation, function () {
            if (!presentation.active)
                root.releasePlaylistEditor(presentation);
        });
    }
    function openWallpaper(wallpaperId) {
        root.clearWallpaperSelection();
        root.selectedWallpaper = null;
        root.requestedWallpaperId = String(wallpaperId || "");
        detailPresenter.present();
        if (m_grid_view)
            m_grid_view.currentIndex = -1;
    }
    function pickSort(idx) {
        if (idx === sortIndex) {
            sortAsc = !sortAsc;
        } else {
            // Switching key keeps the current asc/desc order.
            sortIndex = idx;
        }
        applySort();
        _persistGlobalChange(g => {
            g.wallpaperSorts = [_buildSortRule()];
        });
    }
    function releasePlaylistEditor(presentation) {
        if (root.playlistEditorPresentation !== presentation)
            return;
        const returnToList = root.playlistEditorReturnsToList;
        root.playlistEditorPresentation = null;
        root.playlistEditorId = 0;
        root.playlistEditorReturnsToList = true;
        if (returnToList)
            root.showPlaylistListSheet();
    }
    function releasePlaylistListSheet(sheet) {
        if (root.playlistListSheet === sheet)
            root.playlistListSheet = null;
    }
    function releaseWallpaperSelectSheet(sheet) {
        const target = sheet || root.wallpaperSelectSheet;
        if (!target)
            return;
        if (root.wallpaperSelectSheet === target)
            root.wallpaperSelectSheet = null;
    }
    function releaseWallpaperTweakSheet(sheet) {
        if (root.wallpaperTweakSheet === sheet)
            root.wallpaperTweakSheet = null;
    }
    function reloadAll() {
        pluginQuery.reload();
        playlistListQuery.reload();
        filterSettingsGet.reload();
        sourceFilterQuery.reload();
    }
    function removeSelectedWallpapers() {
        const select = root.currentWallpaperSelect;
        if (!select || selectionRemoveQuery.querying)
            return;

        const ids = select.removableSelectedWallpaperIds();
        if (ids.length === 0)
            return;

        selectionRemoveQuery.remove(ids);
    }
    function requestWallpaperSelection(index) {
        const model = wallpaperQuery.data;
        if (!model)
            return;

        root.beginWallpaperSelection(index);
    }
    function restoreSortFromSettings(rules) {
        if (!rules || rules.length === 0) {
            // No persisted sort yet — keep whatever defaults are in place
            // and push them down so the list query has at least one rule.
            applySort();
            return;
        }
        const r = rules[0];
        const idx = sortOptions.findIndex(o => o.key === r.key);
        if (idx >= 0)
            sortIndex = idx;
        sortAsc = r.direction !== WC.SortDirection.SORT_DIRECTION_DESC;
        applySort();
    }
    function selectedWallpaperIds() {
        return root.currentWallpaperSelect ? root.currentWallpaperSelect.selectedWallpaperIds() : [];
    }
    function showPlaylistListSheet() {
        playlistListQuery.reload();
        Qt.callLater(function () {
            if (!root.selectionActive && !root.playlistEditorPresentation?.active)
                root.ensurePlaylistListSheet();
        });
    }
    function syncWallpaperSelectSheet() {
        if (root.selectionActionSheetActive) {
            root.ensureWallpaperSelectSheet();
            return;
        }

        if (root.wallpaperSelectSheet?.active)
            root.wallpaperSelectSheet.close();
    }
    function togglePlaylistListSheet() {
        if (root.playlistListSheet?.active) {
            root.playlistListSheet.close();
            return;
        }
        if (root.wallpaperTweakSheet?.active)
            root.wallpaperTweakSheet.close();
        playlistListQuery.reload();
        root.ensurePlaylistListSheet();
    }
    function toggleWallpaperTweakSheet() {
        if (root.wallpaperTweakSheet?.active) {
            root.wallpaperTweakSheet.close();
            return;
        }
        if (root.playlistListSheet?.active)
            root.playlistListSheet.close();
        root.ensureWallpaperTweakSheet();
    }

    padding: MD.MProp.size.isCompact ? 0 : 12
    showBackground: false

    contentItem: MD.SplitView {
        id: detailSplit

        handle: null
        orientation: Qt.Horizontal
        spacing: 24

        // --- Left: wallpaper grid ---
        MD.Pane {
            MD.SplitViewBase.fillWidth: true
            padding: 0
            radius: root.MD.MProp.page.backgroundRadius
            showBackground: true

            contentItem: ColumnLayout {
                spacing: 0

                // Toolbar
                RowLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    Layout.topMargin: 4
                    spacing: 8

                    MD.EmbedChip {
                        id: sortChip

                        mdState.borderWidth: 1
                        text: root.sortOptions[root.sortIndex].name
                        trailingIconName: root.sortAsc ? MD.Token.icon.arrow_downward : MD.Token.icon.arrow_upward

                        onClicked: sortMenu.open()

                        MD.Menu {
                            id: sortMenu

                            model: root.sortOptions
                            parent: sortChip
                            y: parent.height

                            contentDelegate: MD.MenuItem {
                                required property int index
                                required property var modelData

                                icon.name: index === root.sortIndex ? (root.sortAsc ? MD.Token.icon.arrow_downward : MD.Token.icon.arrow_upward) : ' '
                                text: modelData.name

                                onClicked: {
                                    root.pickSort(index);
                                    sortMenu.close();
                                }
                            }
                        }
                    }

                    // Free-text search → wallpaperQuery.searchText.
                    // SearchChip debounces internally so this fires
                    // 1s after the user stops typing. Daemon-side
                    // the value becomes an extra `name CONTAINS`
                    // filter rule in its own group.
                    W.SearchChip {
                        id: m_search_field

                        Layout.preferredWidth: 120
                        placeholderText: qsTr("Search")

                        onTextEdited: wallpaperQuery.searchText = text
                    }
                    MD.ActionToolBar {
                        id: wallpaperActionToolBar

                        Layout.fillWidth: true
                        actions: [playlistListAction, tweakAction, filterAction, sourcesAction, refreshAction]
                    }
                }

                // Horizontal scan-progress strip below the toolbar.
                // Only shown when the grid has wallpapers to display
                // (the empty-state path uses the centered BusyIndicator).
                MD.LinearIndicator {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    Layout.topMargin: 4
                    running: visible
                    visible: m_grid_view.count > 0 && W.Notify.scanInProgress
                }

                // Grid + centered empty-state overlay
                Item {
                    Layout.fillHeight: true
                    Layout.fillWidth: true

                    W.WallpaperView {
                        id: m_grid_view

                        anchors.fill: parent
                        bottomMargin: root.selectionActionSheetActive ? root.selectionSheetReserve : itemSpacing / 2
                        focus: true
                        model: wallpaperQuery.data
                        roles: ["item", "selected"]
                        tweak: wallpaperTweakState
                        visible: m_grid_view.count > 0

                        delegate: WallpaperCard {
                            fallbackAspectRatio: wallpaperTweakState.itemAspectRatio
                            itemHeight: masonry ? height : m_grid_view.displayItemHeight
                            itemWidth: m_grid_view.displayItemWidth
                            masonry: m_grid_view.masonry
                            selected: row.value.selected ?? false
                            shimmer: m_grid_view.shimmer
                            spacing: m_grid_view.itemSpacing

                            onClicked: modifiers => root.handleWallpaperClick(index, modifiers)
                            onSelectionRequested: modifiers => root.requestWallpaperSelection(index)
                        }

                        Keys.onEscapePressed: event => {
                            if (root.selectionActive) {
                                root.cancelWallpaperSelection();
                                event.accepted = true;
                            }
                        }
                        onActivated: (index, modifiers) => root.handleWallpaperClick(index, modifiers)
                    }
                    MD.Button {
                        anchors.bottom: parent.bottom
                        anchors.bottomMargin: root.selectionActionSheetActive ? root.selectionSheetReserve : 16
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: qsTr("Retry")
                        visible: wallpaperQuery.status === 3 && m_grid_view.count > 0

                        onClicked: wallpaperQuery.retry()
                    }
                    MD.Button {
                        id: cancelSelectionButton

                        anchors.left: parent.left
                        anchors.leftMargin: 16
                        anchors.top: parent.top
                        anchors.topMargin: 12
                        icon.name: MD.Token.icon.close
                        mdState.selected: true
                        mdState.type: MD.Enum.BtElevated
                        text: String(root.selectedWallpaperCount)
                        visible: root.selectionActive
                        z: 10

                        onClicked: root.cancelWallpaperSelection()
                    }
                    MD.Loader {
                        active: m_grid_view.count === 0
                        anchors.centerIn: parent
                        sourceComponent: m_load_comp
                    }
                    Component {
                        id: m_load_comp

                        ColumnLayout {
                            readonly property string libraryHintIcon: '<font face="' + MD.Token.font.icon_family + '" style="font-size: ' + libraryHintSize + 'px;">' + sourcesAction.icon.name + '</font>'
                            readonly property int libraryHintSize: 22
                            readonly property bool showLibraryHint: !wallpaperQuery.querying && !wallpaperQuery.hasActiveFilters && wallpaperQuery.searchText.trim().length === 0 && W.App.libraryManager.count === 0

                            spacing: 16

                            MD.BusyIndicator {
                                Layout.alignment: Qt.AlignHCenter
                                running: wallpaperQuery.querying
                            }
                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                color: MD.Token.color.on_surface_variant
                                text: wallpaperQuery.status === 3 ? wallpaperQuery.error : qsTr("No wallpapers found")
                                typescale: MD.Token.typescale.body_large
                                visible: !wallpaperQuery.querying
                            }
                            MD.Button {
                                Layout.alignment: Qt.AlignHCenter
                                text: qsTr("Retry")
                                visible: wallpaperQuery.status === 3

                                onClicked: wallpaperQuery.retry()
                            }
                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                Layout.preferredWidth: Math.max(0, Math.min(420, m_grid_view.width - 32))
                                color: MD.Token.color.on_surface_variant
                                horizontalAlignment: Text.AlignHCenter
                                text: qsTr("Click the %1 button in the top right to add a library.").arg(parent.libraryHintIcon)
                                textFormat: Text.RichText
                                typescale: MD.Token.typescale.body_medium
                                visible: parent.showLibraryHint
                                wrapMode: Text.WordWrap
                            }
                            MD.BusyButton {
                                Layout.alignment: Qt.AlignHCenter
                                busy: autoDetectQuery.querying
                                mdState.type: MD.Enum.BtFilledTonal
                                text: qsTr("Auto detect libraries")
                                // Only offer auto-detect when the empty grid is
                                // genuinely "fresh user, nothing configured" —
                                // not when filters are excluding existing rows
                                // and not when libraries are already registered
                                // (in that case the user wants Refresh, not a
                                // second round of auto-detection).
                                visible: parent.showLibraryHint

                                onClicked: {
                                    if (!busy)
                                        autoDetectQuery.reload();
                                }
                            }
                        }
                    }
                }
            }
        }

        // --- Right: wallpaper detail panel ---
        MD.Pane {
            MD.SplitViewBase.maximumWidth: 280
            MD.SplitViewBase.minimumWidth: 280
            MD.SplitViewBase.preferredWidth: 280
            padding: 0
            radius: root.MD.MProp.page.backgroundRadius
            showBackground: true
            visible: sideSite.current

            contentItem: MD.PresentationSite {
                id: sideSite

                presenter: detailPresenter
            }
        }
    }

    Component.onCompleted: {
        applySort();
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready)
            reloadAll();
    }
    Component.onDestruction: {
        root.hiddenWallpapersReturnToList = false;
        root.hiddenWallpapersPresentation?.cancel();
        root.filterPresentation?.cancel();
        root.playlistEditorReturnsToList = false;
        root.playlistEditorPresentation?.cancel();
    }
    onSelectionActiveChanged: {
        if (selectionActive) {
            selectedWallpaper = null;
            requestedWallpaperId = "";
            if (m_grid_view)
                m_grid_view.currentIndex = -1;
        } else {
            wallpaperSelectSheetRelay.reset();
        }
        root.syncWallpaperSelectSheet();
    }

    W.WallpaperListQuery {
        id: wallpaperQuery

        onHiddenLibraryIdsChanged: {
            if (root._quickFiltersSeeded)
                root._persistGlobalChange(g => {});
        }
    }
    QtObject {
        id: libraryVisibilityState

        readonly property W.WallpaperListQuery query: wallpaperQuery
        readonly property bool ready: root._quickFiltersSeeded
    }
    W.WallpaperSelectStorage {
        id: userWallpaperSelect

        property list<MD.Action> actions: [hideSelectionAction, removeSelectionAction, createPlaylistFromSelectionAction, addToPlaylistAction]

        model: wallpaperQuery.data
    }
    W.PlaylistItemSelectStorage {
        id: playlistWallpaperSelect

        property list<MD.Action> actions: [applyPlaylistSelectionAction]

        model: wallpaperQuery.data
    }
    W.WallpaperScanQuery {
        id: scanQuery
    }
    W.WallpaperRemoveQuery {
        id: selectionRemoveQuery

        forwardError: false

        onRemovedMany: function (wallpaperIds, removedCount) {
            wallpaperQuery.reload();
            root.clearWallpaperSelection();
            W.Action.toast(qsTr("Removed %1").arg(removedCount));
        }
        onStatusChanged: {
            if (selectionRemoveQuery.status === 3) {
                const message = selectionRemoveQuery.error && selectionRemoveQuery.error.length > 0 ? selectionRemoveQuery.error : qsTr("Remove failed");
                W.Action.toast(message, 6000, 1, null);
            }
        }
    }
    W.WallpaperHideQuery {
        id: selectionHideQuery

        forwardError: false

        onStatusChanged: {
            if (selectionHideQuery.status === 3) {
                const message = selectionHideQuery.error && selectionHideQuery.error.length > 0 ? selectionHideQuery.error : qsTr("Hide failed");
                W.Action.toast(message, 6000, 1, null);
            }
        }
        onUpdated: function (wallpaperIds, hidden, updatedCount) {
            root.clearWallpaperSelection();
            W.Action.toast(hidden ? qsTr("Hidden %1").arg(updatedCount) : qsTr("Unhidden %1").arg(updatedCount));
        }
    }
    W.PlaylistListQuery {
        id: playlistListQuery
    }
    Connections {
        function onPlaylistsChanged() {
            root.playlistListReady = true;
        }
        function onStatusChanged(status) {
            if (status !== 1)
                root.playlistListReady = true;
        }

        target: playlistListQuery
    }
    W.PlaylistMutationQuery {
        id: playlistMutation

        forwardError: false

        onDone: {
            if (playlistMutation.status === 3) {
                root.playlistMutationSuccessMessage = "";
                root.playlistMutationPendingMessage = "";
                playlistMutationCleanupTimer.stop();
                W.Action.toast(qsTr("Playlist update failed"));
                return;
            }
            root.playlistMutationPendingMessage = root.playlistMutationSuccessMessage.length > 0 ? root.playlistMutationSuccessMessage : qsTr("Playlist updated");
            root.playlistMutationSuccessMessage = "";
            playlistMutationCleanupTimer.restart();
        }
    }
    W.PlaylistMutationQuery {
        id: playlistSheetCreateMutation

        forwardError: false

        onDone: {
            if (playlistSheetCreateMutation.status === 3)
                W.Action.toast(qsTr("Playlist creation failed"));
            else
                W.Action.toast(qsTr("Playlist created"));
        }
    }
    W.PlaylistMutationQuery {
        id: playlistPlaybackMutation

        forwardError: false

        onDone: {
            if (playlistPlaybackMutation.status === 3)
                W.Action.toast(qsTr("Playlist playback failed"));
        }
    }
    W.PlaylistMutationQuery {
        id: playlistItemsMutation

        forwardError: false

        onDone: {
            if (playlistItemsMutation.status === 3) {
                W.Action.toast(playlistItemsMutation.error || qsTr("Playlist update failed"), 6000, 1, null);
                return;
            }
            W.Action.toast(qsTr("Playlist updated"));
            root.finishPlaylistItemSelection();
        }
    }
    W.TweakState {
        id: wallpaperTweakState

        settingsCategory: "WallpaperView"
    }
    W.PlaylistListSheetState {
        id: playlistListSheetState

        page: root
        playlistCreateMutation: playlistSheetCreateMutation
        playlistListQuery: playlistListQuery
        playlistMutation: playlistMutation
        playlistPlaybackMutation: playlistPlaybackMutation
    }
    W.SelectSheetContentState {
        id: selectSheetContentState

        page: root
        playlistListQuery: playlistListQuery
        playlistMutation: playlistMutation
    }
    Qml.Timer {
        id: playlistMutationCleanupTimer

        interval: MD.Token.duration.short4 + 16
        repeat: false

        onTriggered: {
            const message = root.playlistMutationPendingMessage;
            root.playlistMutationPendingMessage = "";
            playlistListQuery.reload();
            root.clearWallpaperSelection();
            if (message.length > 0)
                W.Action.toast(message);
        }
    }
    QtObject {
        id: wallpaperSelectSheetRelay

        property var activeAction: null
        property Component activeComponent: null
        readonly property Component currentComponent: activeComponent ? activeComponent : defaultComponent
        property Component defaultComponent: null

        signal addToPlaylistRequested
        signal newPlaylistRequested

        function requestAddToPlaylist() {
            if (toggle(addToPlaylistAction, addToPlaylistSheetComponent)) {
                playlistListQuery.reload();
                addToPlaylistRequested();
            }
        }
        function requestNewPlaylist() {
            if (toggle(createPlaylistFromSelectionAction, newPlaylistSheetComponent))
                newPlaylistRequested();
        }
        function reset() {
            activeAction = null;
            activeComponent = null;
            defaultComponent = null;
        }
        function restoreDefault() {
            activeAction = null;
            activeComponent = null;
        }
        function toggle(action, component) {
            if (activeAction === action) {
                restoreDefault();
                return false;
            }
            activeAction = action;
            activeComponent = component;
            return true;
        }
    }

    // Daemon-driven syncs (manual click, LibraryAdd/Remove, startup)
    // all reach the UI through `Notify` (mirrors the daemon's
    // `GlobalEvent` broadcasts). Toast UX is handled here via
    // `Action.toast`; Notify itself is intentionally toast-free.
    Connections {
        function onDaemonReady() {
            root.reloadAll();
        }
        function onPlaylistChanged() {
            playlistListQuery.reload();
        }
        function onPluginChanged() {
            pluginQuery.reload();
            sourceFilterQuery.reload();
        }
        function onWallpaperHiddenChanged(wallpaperIds, hidden) {
            wallpaperQuery.reload();
        }
        function onWallpaperSyncFinished(count, error) {
            if (error && error.length > 0) {
                W.Action.toast(qsTr("Sync failed: %1").arg(error));
            } else {
                W.Action.toast(qsTr("Scanned %n wallpaper(s)", "", count));
            }
            wallpaperQuery.reload();
        }

        target: W.Notify
    }
    MD.Action {
        id: createPlaylistFromSelectionAction

        busy: playlistMutation.querying
        checked: wallpaperSelectSheetRelay.activeAction === createPlaylistFromSelectionAction
        enabled: root.selectedWallpaperCount > 0
        icon.name: MD.Token.icon.playlist_add
        text: qsTr("New playlist")

        onTriggered: wallpaperSelectSheetRelay.requestNewPlaylist()
    }
    MD.Action {
        id: addToPlaylistAction

        checked: wallpaperSelectSheetRelay.activeAction === addToPlaylistAction
        enabled: root.selectedWallpaperCount > 0 && (playlistListQuery.playlists || []).length > 0 && !playlistMutation.querying
        icon.name: MD.Token.icon.playlist_add
        text: qsTr("Add to playlist")

        onTriggered: wallpaperSelectSheetRelay.requestAddToPlaylist()
    }
    MD.Action {
        id: applyPlaylistSelectionAction

        busy: playlistItemsMutation.querying
        enabled: playlistWallpaperSelect.playlistId > 0 && !playlistItemsMutation.querying
        icon.name: MD.Token.icon.check
        text: qsTr("Apply")

        onTriggered: root.applyPlaylistSelection()
    }
    MD.Action {
        id: hideSelectionAction

        busy: selectionHideQuery.querying
        enabled: root.selectedWallpaperCount > 0 && !selectionHideQuery.querying
        icon.name: MD.Token.icon.visibility_off
        text: root.selectionAllHidden ? qsTr("Unhide %1").arg(root.selectedWallpaperCount) : qsTr("Hide %1").arg(root.selectedWallpaperCount)

        onTriggered: root.hideSelectedWallpapers()
    }
    MD.Action {
        id: removeSelectionAction

        busy: selectionRemoveQuery.querying
        enabled: root.removableSelectedWallpaperCount > 0 && !selectionRemoveQuery.querying
        icon.name: MD.Token.icon.delete
        text: qsTr("Remove %1").arg(root.removableSelectedWallpaperCount)

        onTriggered: root.removeSelectedWallpapers()
    }
    MD.Action {
        id: playlistListAction

        checked: W.App.displayManager.hasActivePlaylistDisplays
        icon.name: MD.Token.icon.playlist_play
        text: qsTr("Playlists")

        onTriggered: root.togglePlaylistListSheet()
    }
    MD.Action {
        id: tweakAction

        checked: root.isSheetActive(root.wallpaperTweakSheet)
        icon.name: MD.Token.icon.tune
        text: qsTr("Tweak")

        onTriggered: root.toggleWallpaperTweakSheet()
    }
    MD.Action {
        id: filterAction

        checked: wallpaperQuery.hasOtherFilters
        icon.name: MD.Token.icon.filter_alt
        text: qsTr("Filters")

        onTriggered: {
            if (root.filterPresentation?.active)
                return;
            root.filterPresentation = root.Window.window.presentPopup(filterDialogComponent);
        }
    }
    MD.Action {
        id: sourcesAction

        property var presentation: null

        checked: wallpaperQuery.hiddenLibraryIds.length > 0
        icon.name: MD.Token.icon.hard_drive
        text: qsTr("Library Manager")

        onTriggered: {
            if (presentation?.active)
                return;
            presentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
                source: 'waywallen.ui/SourceManagePage',
                props: {
                    visibilityState: libraryVisibilityState
                }
            });
        }
    }
    MD.Action {
        id: refreshAction

        enabled: !W.Notify.scanInProgress
        icon.name: MD.Token.icon.refresh
        text: qsTr("Refresh")

        onTriggered: scanQuery.reload()
    }
    W.RendererPluginListQuery {
        id: pluginQuery
    }

    // Sources already declare `{ value, label }` for their filter values.
    // Downloaded items keep the raw value as their tag / content rating, so
    // the library view borrows those labels for display only.
    W.RemoteAvailabilityQuery {
        id: sourceFilterQuery
    }
    W.LibraryAutoDetectQuery {
        id: autoDetectQuery
    }
    W.SettingsGetQuery {
        id: filterSettingsGet

        onGlobalChanged: {
            // Restore sort first so the filter pipeline below doesn't
            // dispatch a list reload with the stale sort.
            root.restoreSortFromSettings(global.wallpaperSorts || []);
            if (!root._quickFiltersSeeded) {
                wallpaperQuery.skipTypes = global.wallpaperSkipTypes || [];
                wallpaperQuery.filterTags = global.wallpaperFilterTags || [];
                wallpaperQuery.skipContentRatings = global.wallpaperSkipContentRatings || [];
                wallpaperQuery.hiddenFilter = Number(global.wallpaperHiddenFilter || 0);
                wallpaperQuery.hiddenLibraryIds = global.wallpaperHiddenLibraryIds || [];
                root._quickFiltersSeeded = true;
            }
            const filters = global.wallpaperFilters || [];
            const logics = global.wallpaperFilterLogics || [];
            const filterStateChanged = wallpaperQuery.replaceFilterState(filters, logics);
            if (filterStateChanged || !root._filterStateSeeded) {
                wallpaperFilterModel.replaceState(filters, logics);
                root._filterStateSeeded = true;
            }
        }
    }
    W.SettingsSetQuery {
        id: filterSettingsSet
    }
    W.WallpaperFilterRuleModel {
        id: wallpaperFilterModel

        function doQuery() {
            if (!wallpaperQuery.replaceFilterState(items(), filterLogics))
                wallpaperQuery.reload();
        }

        onApply: {
            doQuery();
            root._persistGlobalChange(g => {
                g.wallpaperFilters = items();
                g.wallpaperFilterLogics = filterLogics;
            });
        }
        onReset: {
            replaceState(filterSettingsGet.global.wallpaperFilters || [], filterSettingsGet.global.wallpaperFilterLogics || []);
            doQuery();
        }
    }
    Component {
        id: filterDialogComponent

        W.WallpaperFilterDialog {
            filterTags: wallpaperQuery.filterTags
            hiddenFilter: wallpaperQuery.hiddenFilter
            model: wallpaperFilterModel
            popupWindow: root.Window.window
            skipContentRatings: wallpaperQuery.skipContentRatings
            skipTypes: wallpaperQuery.skipTypes
            supportedTypes: pluginQuery.supportedTypes || []
            typeLabels: pluginQuery.typeLabels || ({})
            valueLabels: root.sourceValueLabels

            onApplyFilterTags: function (tags) {
                wallpaperQuery.filterTags = tags;
                root._persistGlobalChange(g => {
                    g.wallpaperFilterTags = tags;
                });
            }
            onHiddenFilterSelected: function (value) {
                if (wallpaperQuery.hiddenFilter === value)
                    return;
                wallpaperQuery.hiddenFilter = value;
                root._persistGlobalChange(g => {
                    g.wallpaperHiddenFilter = value;
                });
            }
            onToggleSkip: function (ty) {
                const next = (wallpaperQuery.skipTypes || []).slice();
                const i = next.indexOf(ty);
                if (i >= 0)
                    next.splice(i, 1);
                else
                    next.push(ty);
                wallpaperQuery.skipTypes = next;
                root._persistGlobalChange(g => {
                    g.wallpaperSkipTypes = next;
                });
            }
            onToggleSkipRating: function (rating) {
                const next = (wallpaperQuery.skipContentRatings || []).slice();
                const i = next.indexOf(rating);
                if (i >= 0)
                    next.splice(i, 1);
                else
                    next.push(rating);
                wallpaperQuery.skipContentRatings = next;
                root._persistGlobalChange(g => {
                    g.wallpaperSkipContentRatings = next;
                });
            }
        }
    }
    Connections {
        function onSettingsChanged() {
            filterSettingsGet.reload();
        }

        target: W.Notify
    }
    Connections {
        function onWallpaperSelectEntered(storage) {
            root.adoptWallpaperSelect(storage);
        }

        target: W.Action
    }
    Connections {
        function onActiveChanged() {
            root.syncWallpaperSelectSheet();
        }

        target: root.currentWallpaperSelect
    }
    MD.AdaptivePresenter {
        id: detailPresenter

        content: root.detailPanel
        destination: root.selectionActive || !(root.requestedWallpaperId || root.selectedWallpaper?.id_proto) ? null : detailSplit.width >= MD.Token.window_class.expanded.min_width ? sideSite : sheetSite
        enabled: root.visible

        onAboutToRelocate: root.detailPanel.dismissAnchoredPopups()
        onDismissed: {
            root.selectedWallpaper = null;
            root.requestedWallpaperId = "";
        }
    }
    MD.BottomSheet {
        id: detailSheet

        maxSheetWidth: 400
        nestedScrollEnabled: true
        parent: root
        preferredContentHeight: root.height
        wideSideMargin: 24

        onAboutToHide: root.detailPanel.dismissAnchoredPopups()

        MD.PresentationSite {
            id: sheetSite

            height: detailSheet.contentViewportHeight
            popup: detailSheet
            presenter: detailPresenter
            width: detailSheet.contentViewportWidth
        }
    }
    Component {
        id: wallpaperSelectSheetComponent

        W.SelectSheet {
            currentWallpaperSelect: root.currentWallpaperSelect
            popupParent: root
            relay: wallpaperSelectSheetRelay
        }
    }
    Component {
        id: wallpaperTweakSheetComponent

        W.TweakSheet {
            popupParent: root
            tweak: wallpaperTweakState
        }
    }
    Component {
        id: playlistListSheetComponent

        W.PlaylistListSheet {
            popupParent: root
            sheetState: playlistListSheetState

            onHiddenWallpapersRequested: root.openHiddenWallpapers()
        }
    }
    Component {
        id: newPlaylistSheetComponent

        W.NewPlaylistSheetContent {
            sheetState: selectSheetContentState
        }
    }
    Component {
        id: addToPlaylistSheetComponent

        W.AddToPlaylistSheetContent {
            sheetState: selectSheetContentState
        }
    }
}
