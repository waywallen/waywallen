pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    showBackground: false
    padding: MD.MProp.size.isCompact ? 0 : 12

    property bool detailOpen: false
    readonly property var detailRow: detailOpen ? detailStore.item : null

    property string sourceId: ""
    property var sortOptions: []
    property int sortIndex: 0
    property var discoverTweakSheet: null
    property var infoPresentation: null
    property var managePresentation: null
    property W.TweakState discoverTweakState: discoverTweakStateComponent.createObject(root, {
        settingsCategory: "DiscoverView"
    })

    Component {
        id: discoverTweakStateComponent

        W.TweakState {}
    }

    W.DiscoverState {
        id: discoverState
    }

    W.RemoteStoreItem {
        id: detailStore
    }

    function sourceInfo(id) {
        for (const s of availabilityQuery.sources) {
            if (s.id === id)
                return s;
        }
        return null;
    }

    function sourceName(id) {
        const s = sourceInfo(id);
        return s ? (W.I18n.tr(s.displayNameText) || s.name) : "";
    }

    function sourceFilters(id) {
        const s = sourceInfo(id);
        return s && s.filters ? s.filters : [];
    }

    function sourceFilterOptions(id) {
        const options = [];
        for (const filter of sourceFilters(id)) {
            for (const option of filter.options ?? [])
                options.push(option);
        }
        return options;
    }

    function sourceCapability(id) {
        const s = sourceInfo(id);
        return s ? Number(s.remoteCapability ?? 0) : 0;
    }

    function sourceRemoteHint(id) {
        const s = sourceInfo(id);
        return s ? W.I18n.tr(s.remoteHintText) : "";
    }

    function sameList(a, b) {
        const left = a ?? [];
        const right = b ?? [];
        if (left.length !== right.length)
            return false;
        for (let i = 0; i < left.length; ++i) {
            if (left[i] !== right[i])
                return false;
        }
        return true;
    }

    function pruneTags(tags, filters) {
        const allowed = {};
        for (const filter of filters ?? []) {
            for (const option of filter.options ?? [])
                allowed[String(option.value)] = true;
        }
        let out = [];
        for (const tag of tags ?? []) {
            const value = String(tag);
            if (allowed[value] === true)
                out.push(value);
        }
        return out;
    }

    function sortLabel() {
        if (sortOptions.length === 0)
            return qsTr("Sort");
        return W.I18n.tr(sortOptions[Math.max(0, Math.min(sortIndex, sortOptions.length - 1))].labelText);
    }

    function tweakSettingsCategory(id) {
        const source = String(id ?? "");
        return source.length > 0 ? "DiscoverView/" + encodeURIComponent(source) + "/Tweak" : "DiscoverView";
    }

    function loadDiscoverTweakState(id) {
        const category = tweakSettingsCategory(id);
        if (discoverTweakState.settingsCategory === category)
            return;
        const next = discoverTweakStateComponent.createObject(root, {
            settingsCategory: category
        });
        if (!next)
            return;
        if (isSheetActive(discoverTweakSheet))
            discoverTweakSheet.close();
        const previous = discoverTweakState;
        discoverTweakState = next;
        previous.destroy();
    }

    function setSource(id) {
        const s = sourceInfo(id);
        if (!s)
            return;
        const sourceChanged = sourceId !== id;
        const currentSortKey = searchQuery.sortKey;
        sourceId = id;
        discoverState.setSelectedSourceId(id);
        if (sourceChanged)
            loadDiscoverTweakState(id);
        sortOptions = s.sorts ?? [];
        sortIndex = 0;
        if (!sourceChanged && currentSortKey.length > 0) {
            for (let i = 0; i < sortOptions.length; ++i) {
                if (sortOptions[i].key === currentSortKey) {
                    sortIndex = i;
                    break;
                }
            }
        }
        const canBrowse = currentSourceLoginAction() === null;
        searchQuery.browsingEnabled = canBrowse;
        const candidateTags = sourceChanged ? discoverState.filterValuesFor(id) : searchQuery.tags;
        const nextTags = pruneTags(candidateTags, s.filters ?? []);
        if (!sameList(searchQuery.tags, nextTags))
            searchQuery.tags = nextTags;
        discoverState.setFilterValuesFor(id, nextTags);
        searchQuery.sourceId = id;
        detailsQuery.sourceId = id;
        searchQuery.sortKey = sortOptions.length > 0 ? sortOptions[sortIndex].key : "";
        if (sourceChanged || !canBrowse) {
            detailOpen = false;
            detailsQuery.itemId = "";
            m_grid.currentIndex = -1;
        }
    }

    function pickSort(idx) {
        if (idx < 0 || idx >= sortOptions.length)
            return;
        sortIndex = idx;
        searchQuery.sortKey = sortOptions[idx].key;
    }

    function selectItem(index) {
        detailStore.item = searchQuery.model.item(index);
        detailOpen = true;
        detailPresenter.present();
        detailsQuery.sourceId = detailRow.sourceId;
        detailsQuery.itemId = detailRow.itemId;
        if (root.sourceCapability(detailRow.sourceId) === 2)
            refreshDetailSubscription();
    }

    function closeDetail() {
        detailOpen = false;
        detailsQuery.itemId = "";
        m_grid.currentIndex = -1;
    }

    function openInfo() {
        if (!root.detailRow)
            return;
        if (root.infoPresentation?.active)
            return;
        root.infoPresentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
            source: 'waywallen.ui/RemoteInfoPage',
            props: {
                itemStore: detailStore,
                details: detailsQuery,
                sourceName: root.sourceName(root.detailRow.sourceId),
                remoteCapability: root.sourceCapability(root.detailRow.sourceId),
                remoteHint: root.sourceRemoteHint(root.detailRow.sourceId),
                tagOptions: root.sourceFilterOptions(root.detailRow.sourceId),
                typeLabels: pluginQuery.typeLabels || ({})
            }
        });
    }

    function setDetailSubscription(subscribed) {
        if (!root.detailRow)
            return;
        const sourceId = root.detailRow.sourceId;
        const itemId = root.detailRow.itemId;
        subscriptionQuery.setSubscribed(sourceId, itemId, subscribed);
    }

    function refreshDetailSubscription() {
        if (!root.detailRow || root.sourceCapability(root.detailRow.sourceId) !== 2)
            return;
        if (root.currentSourceLoginAction() !== null)
            return;
        subscriptionQuery.refresh(root.detailRow.sourceId, root.detailRow.itemId);
    }

    function isSheetActive(sheet) {
        return !!sheet && (sheet.status === MD.PopupPresentation.Opening || sheet.status === MD.PopupPresentation.Open);
    }

    function ensureDiscoverTweakSheet() {
        if (!root.discoverTweakState)
            return null;
        if (root.discoverTweakSheet?.active)
            return root.discoverTweakSheet;

        const sheet = root.Window.window.presentPopup(discoverTweakSheetComponent);
        if (sheet.active) {
            root.discoverTweakSheet = sheet;
            sheet.activeChanged.connect(sheet, function () {
                if (!sheet.active)
                    root.releaseDiscoverTweakSheet(sheet);
            });
        }
        return sheet;
    }

    function releaseDiscoverTweakSheet(sheet) {
        if (root.discoverTweakSheet === sheet)
            root.discoverTweakSheet = null;
    }

    function toggleDiscoverTweakSheet() {
        if (root.discoverTweakSheet?.active) {
            root.discoverTweakSheet.close();
            return;
        }
        root.ensureDiscoverTweakSheet();
    }

    MD.Action {
        id: manageAction
        icon.name: MD.Token.icon.manage_accounts
        text: qsTr("Manage")
        enabled: root.currentSourceConfig() !== null
        onTriggered: root.openRemoteManagement()
    }

    MD.Action {
        id: tweakAction
        text: qsTr("Tweak")
        icon.name: MD.Token.icon.tune
        checked: root.isSheetActive(root.discoverTweakSheet)
        onTriggered: root.toggleDiscoverTweakSheet()
    }

    MD.Action {
        id: filterAction
        icon.name: MD.Token.icon.filter_alt
        text: qsTr("Filters")
        enabled: m_filter_dialog.filters.length > 0
        checked: searchQuery.tags.length > 0
        onTriggered: m_filter_dialog.open()
    }

    MD.Action {
        id: refreshAction
        icon.name: MD.Token.icon.refresh
        text: qsTr("Refresh")
        enabled: searchQuery.browsingEnabled && !searchQuery.querying
        onTriggered: searchQuery.reload()
    }

    // Renderer-declared labels for the wallpaper type keys of remote items.
    W.RendererPluginListQuery {
        id: pluginQuery
    }

    W.RemoteAvailabilityQuery {
        id: availabilityQuery
        onSourcesChanged: {
            if (sources.length === 0) {
                searchQuery.browsingEnabled = false;
                return;
            }
            let nextSourceId = root.sourceId;
            if (!root.sourceInfo(nextSourceId)) {
                const savedSourceId = discoverState.selectedSourceId();
                if (root.sourceInfo(savedSourceId))
                    nextSourceId = savedSourceId;
                else if (root.sourceInfo(defaultSourceId))
                    nextSourceId = defaultSourceId;
                else
                    nextSourceId = sources[0].id;
            }
            root.setSource(nextSourceId);
            root.refreshDetailSubscription();
        }
    }

    function currentSourceConfig() {
        const id = root.sourceId;
        const list = availabilityQuery.sources || [];
        for (let i = 0; i < list.length; ++i) {
            if (list[i].id === id)
                return list[i];
        }
        return null;
    }

    function currentSourceLoginAction() {
        const c = root.currentSourceConfig();
        const actions = c && c.actions ? c.actions : [];
        for (let i = 0; i < actions.length; ++i) {
            const kind = Number(actions[i].kind);
            if ((kind === 2 || kind === 3) && actions[i].requiredForBrowsing === true && (actions[i].visible === undefined || actions[i].visible))
                return actions[i];
        }
        return null;
    }

    function openRemoteManagement() {
        const c = root.currentSourceConfig();
        if (!c)
            return;
        if (root.managePresentation?.active)
            return;
        root.managePresentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
            source: 'waywallen.ui/RemoteManagePage',
            props: {
                sourceId: c.id,
                displayName: W.I18n.tr(c.displayNameText) || c.name || c.id
            }
        });
    }

    W.RemoteSearchQuery {
        id: searchQuery
    }

    W.RemoteFilterDialog {
        id: m_filter_dialog
        parent: root.Window.window ? root.Window.window.contentItem : null
        positioningItem: overlayItem
        x: (overlayWidth - width) / 2
        y: (overlayHeight - height) / 2
        popupWindow: root.Window.window
        filters: root.sourceFilters(root.sourceId)
        selectedValues: searchQuery.tags
        onApply: function (values) {
            searchQuery.tags = values;
            discoverState.setFilterValuesFor(root.sourceId, values);
        }
    }

    W.RemoteDetailsQuery {
        id: detailsQuery
    }

    W.RemoteDownloadQuery {
        id: dlQuery
        function onRemoved(sourceId, id) {
            W.Action.toast(qsTr("Removed"));
        }
        function onRemoveFailed(sourceId, id, error) {
            W.Action.toast(qsTr("Remove failed: ") + error);
        }
        function onRejected(sourceId, id, error) {
            W.Action.toast(qsTr("Download rejected: ") + error);
        }
    }

    W.RemoteSubscriptionQuery {
        id: subscriptionQuery
        forwardError: false
    }

    Connections {
        target: subscriptionQuery
        function onStateLoaded(sourceId, id, state, error) {
            const message = W.I18n.tr(error);
            if (message.length > 0)
                W.Action.toast(qsTr("Couldn't load subscription status: ") + message);
        }
        function onSetFinished(sourceId, id, subscribed, accepted, error) {
            if (accepted) {
                const message = subscribed ? qsTr("Subscription request accepted") : qsTr("Unsubscribe request accepted");
                const hint = root.sourceRemoteHint(sourceId);
                W.Action.toast(hint.length > 0 ? message + "\n" + hint : message);
            } else {
                W.Action.toast(qsTr("Subscription update failed: ") + W.I18n.tr(error));
            }
        }
    }

    Connections {
        target: W.Notify
        function onRemoteDownloadProgress(sourceId, id, state, error) {
            if (state === 5 && error.length > 0)
                W.Action.toast(qsTr("Download failed: ") + error);
        }
    }

    function reloadAll() {
        availabilityQuery.reload();
        pluginQuery.reload();
    }

    Connections {
        target: W.Notify
        function onDaemonReady() {
            root.reloadAll();
        }
        function onSettingsChanged() {
            availabilityQuery.reload();
        }
        function onPluginStateChanged() {
            availabilityQuery.reload();
        }
        function onPluginChanged() {
            pluginQuery.reload();
        }
    }

    Component.onCompleted: {
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready)
            reloadAll();
    }

    property RemoteDetailPanel detailPanel: RemoteDetailPanel {
        parent: null
        nestedScrollEnabled: sheetSite.current
        item: root.detailRow
        details: detailsQuery
        remoteCapability: root.detailRow ? root.sourceCapability(root.detailRow.sourceId) : 0
        remoteHint: root.detailRow ? root.sourceRemoteHint(root.detailRow.sourceId) : ""
        tagOptions: root.detailRow ? root.sourceFilterOptions(root.detailRow.sourceId) : []
        typeLabels: pluginQuery.typeLabels || ({})
        downloadState: Number(root.detailRow?.acquisitionState ?? 0)
        subscriptionState: Number(root.detailRow?.acquisitionState ?? 0)
        onBack: detailPresenter.dismiss()
        onShowInfo: root.openInfo()
        onDownloadRequested: {
            if (root.detailRow)
                dlQuery.start(root.detailRow.sourceId, root.detailRow.itemId);
        }
        onRemoveRequested: {
            if (root.detailRow)
                dlQuery.remove(root.detailRow.sourceId, root.detailRow.itemId);
        }
        onSubscriptionRefreshRequested: root.refreshDetailSubscription()
        onSubscriptionChangeRequested: subscribed => root.setDetailSubscription(subscribed)
    }

    MD.AdaptivePresenter {
        id: detailPresenter
        content: root.detailPanel
        enabled: root.visible
        destination: !root.detailRow ? null : detailSplit.width >= MD.Token.window_class.expanded.min_width ? sideSite : sheetSite
        onDismissed: root.closeDetail()
    }

    MD.BottomSheet {
        id: detailSheet
        parent: root
        nestedScrollEnabled: true
        maxSheetWidth: 400
        wideSideMargin: 24
        preferredContentHeight: root.height

        MD.PresentationSite {
            id: sheetSite
            presenter: detailPresenter
            popup: detailSheet
            width: detailSheet.contentViewportWidth
            height: detailSheet.contentViewportHeight
        }
    }

    contentItem: MD.SplitView {
        id: detailSplit
        orientation: Qt.Horizontal
        handle: null
        spacing: 24

        MD.Pane {
            MD.SplitViewBase.fillWidth: true
            radius: root.MD.MProp.page.backgroundRadius
            padding: 0
            showBackground: true

            contentItem: ColumnLayout {
                spacing: 0

                RowLayout {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    Layout.topMargin: 4
                    spacing: 8

                    MD.EmbedChip {
                        id: sourceChip
                        visible: availabilityQuery.sources.length > 1
                        text: root.sourceName(root.sourceId)
                        trailingIconName: MD.Token.icon.arrow_drop_down
                        mdState.borderWidth: 1
                        onClicked: sourceMenu.open()

                        MD.Menu {
                            id: sourceMenu
                            parent: sourceChip
                            y: parent.height
                            model: availabilityQuery.sources
                            contentDelegate: MD.MenuItem {
                                required property var modelData
                                text: W.I18n.tr(modelData.displayNameText) || modelData.name
                                icon.name: modelData.id === root.sourceId ? MD.Token.icon.check : ' '
                                onClicked: {
                                    root.setSource(modelData.id);
                                    sourceMenu.close();
                                }
                            }
                        }
                    }

                    MD.EmbedChip {
                        id: sortChip
                        visible: root.sortOptions.length > 0
                        text: root.sortLabel()
                        trailingIconName: MD.Token.icon.arrow_drop_down
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
                                text: W.I18n.tr(modelData.labelText)
                                icon.name: index === root.sortIndex ? MD.Token.icon.check : ' '
                                onClicked: {
                                    root.pickSort(index);
                                    sortMenu.close();
                                }
                            }
                        }
                    }

                    W.SearchChip {
                        id: m_search_field
                        Layout.preferredWidth: 120
                        placeholderText: qsTr("Search")
                        onTextEdited: searchQuery.query = text
                    }

                    MD.ActionToolBar {
                        Layout.fillWidth: true
                        actions: [manageAction, tweakAction, filterAction, refreshAction]
                    }
                }

                MD.LinearIndicator {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    visible: searchQuery.querying && searchQuery.model.count > 0
                    running: visible
                }

                Item {
                    Layout.fillWidth: true
                    Layout.fillHeight: true

                    MD.VerticalGridView {
                        id: m_grid
                        anchors.fill: parent
                        clip: true
                        cacheBuffer: 300
                        displayMarginBeginning: 300
                        displayMarginEnd: 300
                        currentIndex: -1
                        topMargin: 2
                        bottomMargin: 8
                        leftMargin: 8
                        rightMargin: 8

                        readonly property real _availableWidth: Math.max(0, width - leftMargin - rightMargin)
                        readonly property int _cols: Math.max(1, Math.floor(_availableWidth / root.discoverTweakState.itemSize))
                        readonly property real _stretchedItemWidth: _availableWidth / _cols
                        readonly property bool _fillCell: root.discoverTweakState.layoutMode === root.discoverTweakState.layoutFillCell
                        readonly property real _displayItemWidth: _fillCell ? _stretchedItemWidth : Math.min(root.discoverTweakState.itemSize, _stretchedItemWidth)
                        readonly property real _displayItemHeight: _displayItemWidth / Math.max(root.discoverTweakState.itemAspectRatio, 0.1)
                        cellWidth: _stretchedItemWidth
                        cellHeight: _fillCell ? _displayItemHeight : root.discoverTweakState.itemHeight

                        model: searchQuery.model

                        delegate: RemoteCard {
                            remoteCapability: root.sourceCapability(root.sourceId)
                            itemWidth: m_grid._displayItemWidth
                            itemHeight: m_grid._displayItemHeight
                            onClicked: {
                                m_grid.currentIndex = index;
                                root.selectItem(index);
                            }
                        }

                        highlightFollowsCurrentItem: true
                        highlight: Component {
                            Item {
                                visible: m_grid.currentItem !== null
                                z: 2
                                Rectangle {
                                    anchors.fill: parent
                                    anchors.margins: 4
                                    color: "transparent"
                                    border.color: MD.Token.color.primary
                                    border.width: 3
                                    radius: MD.Token.shape.corner.small + 2
                                }
                            }
                        }
                    }

                    ColumnLayout {
                        anchors.centerIn: parent
                        width: Math.min(parent.width - 48, 420)
                        visible: m_grid.count === 0
                        spacing: 12

                        readonly property bool hasError: !searchQuery.querying && searchQuery.errorText.length > 0
                        readonly property var loginAction: root.currentSourceLoginAction()
                        readonly property bool needsLogin: loginAction !== null
                        readonly property string loginLabel: {
                            const label = loginAction ? W.I18n.tr(loginAction.labelText).trim() : "";
                            return label.length > 0 ? label : qsTr("Log in to %1").arg(root.sourceName(root.sourceId));
                        }
                        readonly property string loginDescription: {
                            const browseDescription = loginAction ? W.I18n.tr(loginAction.browseDescriptionText).trim() : "";
                            if (browseDescription.length > 0)
                                return browseDescription;
                            const description = loginAction ? W.I18n.tr(loginAction.descriptionText).trim() : "";
                            return description.length > 0 ? description : qsTr("Log in to start browsing.");
                        }
                        readonly property string loginButtonLabel: {
                            const label = loginAction ? W.I18n.tr(loginAction.browseButtonLabelText).trim() : "";
                            return label.length > 0 ? label : qsTr("Log in");
                        }

                        MD.BusyIndicator {
                            Layout.alignment: Qt.AlignHCenter
                            running: searchQuery.querying
                            visible: running
                        }

                        MD.Icon {
                            Layout.alignment: Qt.AlignHCenter
                            visible: parent.needsLogin
                            name: MD.Token.icon.person
                            size: 40
                            color: MD.Token.color.on_surface_variant
                        }
                        MD.Label {
                            Layout.fillWidth: true
                            horizontalAlignment: Text.AlignHCenter
                            visible: parent.needsLogin || parent.hasError
                            text: parent.needsLogin ? parent.loginLabel : qsTr("Couldn't load this source")
                            typescale: MD.Token.typescale.title_medium
                            color: MD.Token.color.on_surface
                            wrapMode: Text.WordWrap
                        }
                        MD.Label {
                            Layout.fillWidth: true
                            horizontalAlignment: Text.AlignHCenter
                            visible: parent.needsLogin || parent.hasError
                            text: parent.needsLogin ? parent.loginDescription : searchQuery.errorText
                            typescale: MD.Token.typescale.body_medium
                            color: MD.Token.color.on_surface_variant
                            font.capitalization: Font.MixedCase
                            wrapMode: Text.WordWrap
                            maximumLineCount: 4
                            elide: Text.ElideRight
                        }
                        MD.Button {
                            Layout.alignment: Qt.AlignHCenter
                            visible: parent.needsLogin
                            text: parent.loginButtonLabel
                            mdState.type: MD.Enum.BtFilledTonal
                            onClicked: root.openRemoteManagement()
                        }

                        MD.Label {
                            Layout.alignment: Qt.AlignHCenter
                            visible: !searchQuery.querying && !parent.needsLogin && searchQuery.errorText.length === 0
                            text: qsTr("No wallpapers found")
                            typescale: MD.Token.typescale.body_large
                            color: MD.Token.color.on_surface_variant
                        }
                    }
                }
            }
        }

        MD.Pane {
            MD.SplitViewBase.minimumWidth: 280
            MD.SplitViewBase.preferredWidth: 280
            MD.SplitViewBase.maximumWidth: 280
            visible: sideSite.current
            radius: root.MD.MProp.page.backgroundRadius
            padding: 0
            showBackground: true

            contentItem: MD.PresentationSite {
                id: sideSite
                presenter: detailPresenter
            }
        }
    }

    Component {
        id: discoverTweakSheetComponent

        W.TweakSheet {
            popupParent: root
            tweak: root.discoverTweakState
        }
    }
}
