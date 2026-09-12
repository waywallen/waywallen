pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQml as Qml
import QtQuick.Layouts
import QtQuick.Templates as T
import Qcm.Material as MD
import waywallen.control as WC
import waywallen.ui as W

MD.Page {
    id: root

    W.WallpaperListQuery {
        id: wallpaperQuery
    }

    W.WallpaperSelectStorage {
        id: userWallpaperSelect
        model: wallpaperQuery.data
        property list<MD.Action> actions: [removeSelectionAction, createPlaylistFromSelectionAction, addToPlaylistAction]
    }

    W.PlaylistItemSelectStorage {
        id: playlistWallpaperSelect
        model: wallpaperQuery.data
        property list<MD.Action> actions: [applyPlaylistSelectionAction]
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

    W.PlaylistListQuery {
        id: playlistListQuery
    }

    property bool playlistListReady: false
    property string playlistMutationSuccessMessage: ""
    property string playlistMutationPendingMessage: ""
    readonly property int defaultPlaylistIntervalSecs: 300
    readonly property bool playlistListLoading: playlistListQuery.querying && !root.playlistListReady

    Connections {
        target: playlistListQuery
        function onPlaylistsChanged() {
            root.playlistListReady = true;
        }
        function onStatusChanged(status) {
            if (status !== 1)
                root.playlistListReady = true;
        }
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
        playlistListQuery: playlistListQuery
        playlistMutation: playlistMutation
        playlistCreateMutation: playlistSheetCreateMutation
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
        property Component defaultComponent: null
        readonly property Component currentComponent: activeComponent ? activeComponent : defaultComponent

        signal newPlaylistRequested
        signal addToPlaylistRequested

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

        function requestNewPlaylist() {
            if (toggle(createPlaylistFromSelectionAction, newPlaylistSheetComponent))
                newPlaylistRequested();
        }

        function requestAddToPlaylist() {
            if (toggle(addToPlaylistAction, addToPlaylistSheetComponent)) {
                playlistListQuery.reload();
                addToPlaylistRequested();
            }
        }
    }

    // Daemon-driven syncs (manual click, LibraryAdd/Remove, startup)
    // all reach the UI through `Notify` (mirrors the daemon's
    // `GlobalEvent` broadcasts). Toast UX is handled here via
    // `Action.toast`; Notify itself is intentionally toast-free.
    Connections {
        target: W.Notify
        function onWallpaperSyncFinished(count, error) {
            if (error && error.length > 0) {
                W.Action.toast(qsTr("Sync failed: %1").arg(error));
            } else {
                W.Action.toast(qsTr("Scanned %n wallpaper(s)", "", count));
            }
            wallpaperQuery.reload();
        }
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
    }

    function reloadAll() {
        pluginQuery.reload();
        playlistListQuery.reload();
        filterSettingsGet.reload();
        sourceFilterQuery.reload();
    }

    Component.onCompleted: {
        applySort();
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready)
            reloadAll();
    }

    MD.Action {
        id: createPlaylistFromSelectionAction
        text: qsTr("New playlist")
        icon.name: MD.Token.icon.playlist_add
        busy: playlistMutation.querying
        checked: wallpaperSelectSheetRelay.activeAction === createPlaylistFromSelectionAction
        enabled: root.selectedWallpaperCount > 0
        onTriggered: wallpaperSelectSheetRelay.requestNewPlaylist()
    }

    MD.Action {
        id: addToPlaylistAction
        text: qsTr("Add to playlist")
        icon.name: MD.Token.icon.playlist_add
        checked: wallpaperSelectSheetRelay.activeAction === addToPlaylistAction
        enabled: root.selectedWallpaperCount > 0 && (playlistListQuery.playlists || []).length > 0 && !playlistMutation.querying
        onTriggered: wallpaperSelectSheetRelay.requestAddToPlaylist()
    }

    MD.Action {
        id: applyPlaylistSelectionAction
        text: qsTr("Apply")
        icon.name: MD.Token.icon.check
        busy: playlistItemsMutation.querying
        enabled: playlistWallpaperSelect.playlistId > 0 && !playlistItemsMutation.querying
        onTriggered: root.applyPlaylistSelection()
    }

    MD.Action {
        id: removeSelectionAction
        text: qsTr("Remove %1").arg(root.removableSelectedWallpaperCount)
        icon.name: MD.Token.icon.delete
        busy: selectionRemoveQuery.querying
        enabled: root.removableSelectedWallpaperCount > 0 && !selectionRemoveQuery.querying
        onTriggered: root.removeSelectedWallpapers()
    }

    MD.Action {
        id: playlistListAction
        text: qsTr("Playlists")
        icon.name: MD.Token.icon.playlist_play
        checked: W.App.displayManager.hasActivePlaylistDisplays
        onTriggered: root.togglePlaylistListSheet()
    }

    MD.Action {
        id: tweakAction
        text: qsTr("Tweak")
        icon.name: MD.Token.icon.tune
        checked: root.isSheetActive(root.wallpaperTweakSheet)
        onTriggered: root.toggleWallpaperTweakSheet()
    }

    MD.Action {
        id: filterAction
        icon.name: MD.Token.icon.filter_list
        text: qsTr("Filters")
        checked: wallpaperQuery.hasActiveFilters
        onTriggered: {
            if (root.filterPresentation?.active)
                return;
            root.filterPresentation = root.Window.window.presentPopup(filterDialogComponent);
        }
    }

    MD.Action {
        id: sourcesAction
        icon.name: MD.Token.icon.hard_drive
        text: qsTr("Library Manager")
        property var presentation: null
        onTriggered: {
            if (presentation?.active)
                return;
            presentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
                source: 'waywallen.ui/SourceManagePage'
            });
        }
    }

    MD.Action {
        id: refreshAction
        icon.name: MD.Token.icon.refresh
        text: qsTr("Refresh")
        enabled: !W.Notify.scanInProgress
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

    readonly property var sourceValueLabels: root.buildSourceValueLabels(sourceFilterQuery.sources)

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

    W.LibraryAutoDetectQuery {
        id: autoDetectQuery
    }

    // Quick filters (skip-types, tag filter) are seeded from settings
    // once; after that the local selection is authoritative. Re-adopting
    // them on every settings echo would revert a just-applied toggle
    // whenever the round-trip lags.
    property bool _quickFiltersSeeded: false
    property bool _filterStateSeeded: false

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
            popupWindow: root.Window.window
            model: wallpaperFilterModel
            supportedTypes: pluginQuery.supportedTypes || []
            valueLabels: root.sourceValueLabels
            skipTypes: wallpaperQuery.skipTypes
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
            filterTags: wallpaperQuery.filterTags
            onApplyFilterTags: function (tags) {
                wallpaperQuery.filterTags = tags;
                root._persistGlobalChange(g => {
                    g.wallpaperFilterTags = tags;
                });
            }
            skipContentRatings: wallpaperQuery.skipContentRatings
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
        target: W.Notify
        function onSettingsChanged() {
            filterSettingsGet.reload();
        }
    }

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
    property int sortIndex: 0
    property bool sortAsc: true
    property WC.wallpaperSortRule emptySortRule

    Connections {
        target: wallpaperTweakState
        function onItemSizeChanged() {
            root.forceWallpaperGridLayout();
        }
        function onItemAspectRatioChanged() {
            root.forceWallpaperGridLayout();
        }
        function onLayoutModeChanged() {
            root.forceWallpaperGridLayout();
        }
    }

    function _buildSortRule() {
        const rule = emptySortRule;
        rule.key = sortOptions[sortIndex].key;
        rule.direction = sortAsc ? WC.SortDirection.SORT_DIRECTION_ASC : WC.SortDirection.SORT_DIRECTION_DESC;
        return rule;
    }
    function applySort() {
        wallpaperQuery.sorts = [_buildSortRule()];
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
        filterSettingsSet.global = nextGlobal;
        filterSettingsSet.plugins = filterSettingsGet.plugins;
        filterSettingsSet.reload();
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

    function forceWallpaperGridLayout() {
        if (m_grid_view)
            m_grid_view.forceLayout();
    }

    property var selectedWallpaper: null
    property var currentWallpaperSelect: null
    property var wallpaperSelectSheet: null
    property var wallpaperTweakSheet: null
    property var playlistListSheet: null
    property var playlistEditorPresentation: null
    property double playlistEditorId: 0
    property bool playlistEditorReturnsToList: true
    property var filterPresentation: null
    Component.onDestruction: {
        root.filterPresentation?.cancel();
        root.playlistEditorReturnsToList = false;
        root.playlistEditorPresentation?.cancel();
    }
    readonly property int selectionSheetReserve: wallpaperSelectSheetRelay.currentComponent ? 360 : 160
    readonly property int selectedWallpaperCount: root.currentWallpaperSelect ? root.currentWallpaperSelect.selectedCount : 0
    readonly property int removableSelectedWallpaperCount: root.currentWallpaperSelect ? root.currentWallpaperSelect.removableSelectedCount : 0
    readonly property bool selectionActive: root.currentWallpaperSelect ? root.currentWallpaperSelect.active : false
    readonly property bool selectionActionSheetActive: root.selectionActive && root.currentWallpaperSelect && (root.currentWallpaperSelect.actions || []).length > 0

    onSelectionActiveChanged: {
        if (selectionActive) {
            selectedWallpaper = null;
            if (m_grid_view)
                m_grid_view.currentIndex = -1;
        } else {
            wallpaperSelectSheetRelay.reset();
        }
        root.syncWallpaperSelectSheet();
    }

    Connections {
        target: W.Action
        function onWallpaperSelectEntered(storage) {
            root.adoptWallpaperSelect(storage);
        }
    }

    Connections {
        target: root.currentWallpaperSelect
        function onActiveChanged() {
            root.syncWallpaperSelectSheet();
        }
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

    function releaseWallpaperSelectSheet(sheet) {
        const target = sheet || root.wallpaperSelectSheet;
        if (!target)
            return;
        if (root.wallpaperSelectSheet === target)
            root.wallpaperSelectSheet = null;
    }

    function isSheetActive(sheet) {
        return !!sheet && (sheet.status === MD.PopupPresentation.Opening || sheet.status === MD.PopupPresentation.Open);
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

    function releaseWallpaperTweakSheet(sheet) {
        if (root.wallpaperTweakSheet === sheet)
            root.wallpaperTweakSheet = null;
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

    function releasePlaylistListSheet(sheet) {
        if (root.playlistListSheet === sheet)
            root.playlistListSheet = null;
    }

    function syncWallpaperSelectSheet() {
        if (root.selectionActionSheetActive) {
            root.ensureWallpaperSelectSheet();
            return;
        }

        if (root.wallpaperSelectSheet?.active)
            root.wallpaperSelectSheet.close();
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

    function enterWallpaperSelect(storage) {
        if (!storage)
            return;
        root.adoptWallpaperSelect(storage);
        W.Action.enterWallpaperSelect(storage);
    }

    function interactionWallpaperSelect() {
        return root.currentWallpaperSelect && root.currentWallpaperSelect.active ? root.currentWallpaperSelect : userWallpaperSelect;
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

    function clearWallpaperSelection() {
        if (root.currentWallpaperSelect)
            root.currentWallpaperSelect.clear();
        root.currentWallpaperSelect = null;
        wallpaperSelectSheetRelay.reset();
        root.syncWallpaperSelectSheet();
    }

    function selectedWallpaperIds() {
        return root.currentWallpaperSelect ? root.currentWallpaperSelect.selectedWallpaperIds() : [];
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

    function openPlaylistEditor(playlist) {
        const playlistId = Number(typeof playlist === "object" ? playlist?.id ?? 0 : playlist ?? 0);
        if (!(playlistId > 0))
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
                playlistId: playlistId
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

    function toggleWallpaperTweakSheet() {
        if (root.wallpaperTweakSheet?.active) {
            root.wallpaperTweakSheet.close();
            return;
        }
        if (root.playlistListSheet?.active)
            root.playlistListSheet.close();
        root.ensureWallpaperTweakSheet();
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

    function showPlaylistListSheet() {
        playlistListQuery.reload();
        Qt.callLater(function () {
            if (!root.selectionActive && !root.playlistEditorPresentation?.active)
                root.ensurePlaylistListSheet();
        });
    }

    function finishPlaylistItemSelection() {
        root.clearWallpaperSelection();
        root.showPlaylistListSheet();
    }

    function cancelWallpaperSelection() {
        if (root.currentWallpaperSelect === playlistWallpaperSelect) {
            root.finishPlaylistItemSelection();
            return;
        }
        root.clearWallpaperSelection();
    }

    function applyPlaylistSelection() {
        if (playlistWallpaperSelect.playlistId <= 0 || playlistItemsMutation.querying)
            return;
        playlistItemsMutation.setItems(playlistWallpaperSelect.playlistId, playlistWallpaperSelect.orderedWallpaperIds(), playlistWallpaperSelect.revision);
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
        root.selectedWallpaper = model.item(index);
    }

    function requestWallpaperSelection(index) {
        const model = wallpaperQuery.data;
        if (!model)
            return;

        root.beginWallpaperSelection(index);
    }

    function createPlaylistFromSelection(name) {
        const ids = root.selectedWallpaperIds();
        if (ids.length === 0 || playlistMutation.querying)
            return;

        const title = String(name || "").trim();
        playlistMutation.create(title.length > 0 ? title : qsTr("New playlist"), WC.PlaylistMode.SEQUENTIAL, root.defaultPlaylistIntervalSecs, true, ids);
    }

    function createEmptyPlaylist() {
        if (playlistSheetCreateMutation.querying)
            return;

        playlistSheetCreateMutation.create(qsTr("New playlist"), WC.PlaylistMode.SEQUENTIAL, root.defaultPlaylistIntervalSecs, true, []);
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

    function removeSelectedWallpapers() {
        const select = root.currentWallpaperSelect;
        if (!select || selectionRemoveQuery.querying)
            return;

        const ids = select.removableSelectedWallpaperIds();
        if (ids.length === 0)
            return;

        selectionRemoveQuery.remove(ids);
    }

    function deletePlaylist(playlist) {
        if (!playlist || playlistMutation.querying)
            return;

        root.playlistMutationSuccessMessage = qsTr("Playlist deleted");
        playlistMutation.remove(playlist.id);
    }

    showBackground: false
    padding: MD.MProp.size.isCompact ? 0 : 12

    contentItem: RowLayout {
        spacing: 12

        // --- Left: wallpaper grid ---
        MD.Pane {
            Layout.fillWidth: true
            Layout.fillHeight: true
            radius: root.MD.MProp.page.backgroundRadius
            padding: 0
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
                        text: root.sortOptions[root.sortIndex].name
                        trailingIconName: root.sortAsc ? MD.Token.icon.arrow_downward : MD.Token.icon.arrow_upward
                        mdState.borderWidth: 1
                        onClicked: sortMenu.open()

                        MD.Menu {
                            id: sortMenu
                            parent: sortChip
                            y: parent.height
                            model: root.sortOptions
                            contentDelegate: MD.MenuItem {
                                required property var modelData
                                required property int index
                                text: modelData.name
                                icon.name: index === root.sortIndex ? (root.sortAsc ? MD.Token.icon.arrow_downward : MD.Token.icon.arrow_upward) : ' '
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
                    visible: m_grid_view.count > 0 && W.Notify.scanInProgress
                    running: visible
                }

                // Grid + centered empty-state overlay
                Item {
                    Layout.fillWidth: true
                    Layout.fillHeight: true

                    MD.VerticalGridView {
                        id: m_grid_view
                        anchors.fill: parent
                        clip: true
                        focus: true
                        focusPolicy: Qt.StrongFocus
                        keyNavigationEnabled: true
                        keyNavigationWraps: true
                        currentIndex: -1
                        highlightRangeMode: GridView.NoHighlightRange
                        cacheBuffer: 300
                        displayMarginBeginning: 300
                        displayMarginEnd: 300
                        topMargin: 2
                        bottomMargin: root.selectionActionSheetActive ? root.selectionSheetReserve : 8
                        leftMargin: 8
                        rightMargin: 8
                        visible: m_grid_view.count > 0

                        readonly property real _availableWidth: Math.max(0, width - leftMargin - rightMargin)
                        readonly property int _cols: Math.max(1, Math.floor(_availableWidth / wallpaperTweakState.itemSize))
                        readonly property real _stretchedItemWidth: _availableWidth / _cols
                        readonly property bool _fillCell: wallpaperTweakState.layoutMode === wallpaperTweakState.layoutFillCell
                        readonly property real _displayItemWidth: _fillCell ? _stretchedItemWidth : Math.min(wallpaperTweakState.itemSize, _stretchedItemWidth)
                        readonly property real _displayItemHeight: _displayItemWidth / Math.max(wallpaperTweakState.itemAspectRatio, 0.1)
                        cellWidth: _stretchedItemWidth
                        cellHeight: _fillCell ? _displayItemHeight : wallpaperTweakState.itemHeight

                        model: wallpaperQuery.data

                        delegate: WallpaperCard {
                            selected: model.selected ?? false
                            itemWidth: m_grid_view._displayItemWidth
                            itemHeight: m_grid_view._displayItemHeight
                            onClicked: modifiers => root.handleWallpaperClick(index, modifiers)
                            onSelectionRequested: modifiers => root.requestWallpaperSelection(index)
                        }

                        Keys.onEscapePressed: event => {
                            if (root.selectionActive) {
                                root.cancelWallpaperSelection();
                                event.accepted = true;
                            }
                        }

                        highlightFollowsCurrentItem: true
                        highlight: Component {
                            Item {
                                visible: m_grid_view.currentItem !== null
                                z: 2
                                // Inset 2 = 6 (card margin) − 4 (ring outset),
                                // so the ring sits 4px outside the image
                                // control with the same concentric radius.
                                Rectangle {
                                    anchors.fill: parent
                                    anchors.margins: 2
                                    color: "transparent"
                                    border.color: MD.Token.color.primary
                                    border.width: 3
                                    radius: MD.Token.shape.corner.extra_small + 4
                                }
                            }
                        }
                    }

                    MD.Button {
                        id: cancelSelectionButton
                        anchors.left: parent.left
                        anchors.top: parent.top
                        anchors.leftMargin: 16
                        anchors.topMargin: 12
                        z: 10
                        visible: root.selectionActive
                        checked: true
                        text: String(root.selectedWallpaperCount)
                        icon.name: MD.Token.icon.close
                        mdState.type: MD.Enum.BtElevated
                        onClicked: root.cancelWallpaperSelection()
                    }

                    MD.Loader {
                        anchors.centerIn: parent
                        active: m_grid_view.count === 0
                        sourceComponent: m_load_comp
                    }

                    Component {
                        id: m_load_comp

                        ColumnLayout {
                            spacing: 16
                            readonly property bool showLibraryHint: !wallpaperQuery.querying && !wallpaperQuery.hasActiveFilters && wallpaperQuery.searchText.trim().length === 0 && W.App.libraryManager.count === 0
                            readonly property int libraryHintSize: 22
                            readonly property string libraryHintIcon: '<font face="' + MD.Token.font.icon_family + '" style="font-size: ' + libraryHintSize + 'px;">' + sourcesAction.icon.name + '</font>'

                            MD.BusyIndicator {
                                Layout.alignment: Qt.AlignHCenter
                                running: wallpaperQuery.querying
                            }

                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                visible: !wallpaperQuery.querying
                                text: qsTr("No wallpapers found")
                                typescale: MD.Token.typescale.body_large
                                color: MD.Token.color.on_surface_variant
                            }

                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                Layout.preferredWidth: Math.max(0, Math.min(420, m_grid_view.width - 32))
                                visible: parent.showLibraryHint
                                text: qsTr("Click the %1 button in the top right to add a library.").arg(parent.libraryHintIcon)
                                textFormat: Text.RichText
                                typescale: MD.Token.typescale.body_medium
                                color: MD.Token.color.on_surface_variant
                                wrapMode: Text.WordWrap
                                horizontalAlignment: Text.AlignHCenter
                            }

                            MD.BusyButton {
                                Layout.alignment: Qt.AlignHCenter
                                // Only offer auto-detect when the empty grid is
                                // genuinely "fresh user, nothing configured" —
                                // not when filters are excluding existing rows
                                // and not when libraries are already registered
                                // (in that case the user wants Refresh, not a
                                // second round of auto-detection).
                                visible: parent.showLibraryHint
                                text: qsTr("Auto detect libraries")
                                busy: autoDetectQuery.querying
                                mdState.type: MD.Enum.BtFilledTonal
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
            Layout.preferredWidth: root.selectedWallpaper !== null && !root.selectionActive ? 280 : 0
            Layout.fillHeight: true
            Layout.maximumWidth: 280
            visible: root.selectedWallpaper !== null && !root.selectionActive
            radius: root.MD.MProp.page.backgroundRadius
            padding: 0
            showBackground: true

            contentItem: WallpaperDetailPanel {
                wallpaperId: root.selectedWallpaper?.id_proto ?? ""
                fallbackWallpaper: root.selectedWallpaper
                valueLabels: root.sourceValueLabels
                showApply: true
                onBack: root.selectedWallpaper = null
            }
        }
    }

    Component {
        id: wallpaperSelectSheetComponent

        W.SelectSheet {
            popupParent: root
            relay: wallpaperSelectSheetRelay
            currentWallpaperSelect: root.currentWallpaperSelect
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
