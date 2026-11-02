pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root

    property bool detailOpen: false
    property RemoteDetailPanel detailPanel: RemoteDetailPanel {
        details: detailsQuery
        downloadState: Number(root.detailRow?.acquisitionState ?? 0)
        item: root.detailRow
        nestedScrollEnabled: sheetSite.current
        parent: null
        remoteCapability: root.detailRow ? root.sourceCapability(root.detailRow.sourceId) : 0
        remoteHint: root.detailRow ? root.sourceRemoteHint(root.detailRow.sourceId) : ""
        subscriptionState: Number(root.detailRow?.acquisitionState ?? 0)
        tagOptions: root.detailRow ? root.sourceFilterOptions(root.detailRow.sourceId) : []

        onBack: detailPresenter.dismiss()
        onDownloadRequested: {
            if (root.detailRow)
                dlQuery.start(root.detailRow.sourceId, root.detailRow.itemId);
        }
        onRemoveRequested: {
            if (root.detailRow)
                dlQuery.remove(root.detailRow.sourceId, root.detailRow.itemId);
        }
        onShowInfo: root.openInfo()
        onSubscriptionChangeRequested: subscribed => root.setDetailSubscription(subscribed)
        onSubscriptionRefreshRequested: root.refreshDetailSubscription()
    }
    readonly property var detailRow: detailOpen ? detailStore.item : null
    property var discoverTweakSheet: null
    property W.TweakState discoverTweakState: discoverTweakStateComponent.createObject(root, {
        settingsCategory: "DiscoverView"
    })
    property var infoPresentation: null
    property var managePresentation: null
    property int sortIndex: 0
    property var sortOptions: []
    property string sourceId: ""

    function closeDetail() {
        detailOpen = false;
        detailsQuery.itemId = "";
        m_grid.currentIndex = -1;
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
    function isSheetActive(sheet) {
        return !!sheet && (sheet.status === MD.PopupPresentation.Opening || sheet.status === MD.PopupPresentation.Open);
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
                tagOptions: root.sourceFilterOptions(root.detailRow.sourceId)
            }
        });
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
    function pickSort(idx) {
        if (idx < 0 || idx >= sortOptions.length)
            return;
        sortIndex = idx;
        searchQuery.sortKey = sortOptions[idx].key;
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
    function refreshDetailSubscription() {
        if (!root.detailRow || root.sourceCapability(root.detailRow.sourceId) !== 2)
            return;
        if (root.currentSourceLoginAction() !== null)
            return;
        subscriptionQuery.refresh(root.detailRow.sourceId, root.detailRow.itemId);
    }
    function releaseDiscoverTweakSheet(sheet) {
        if (root.discoverTweakSheet === sheet)
            root.discoverTweakSheet = null;
    }
    function reloadAll() {
        availabilityQuery.reload();
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
    function selectItem(index) {
        detailStore.item = searchQuery.model.item(index);
        detailOpen = true;
        detailPresenter.present();
        detailsQuery.sourceId = detailRow.sourceId;
        detailsQuery.itemId = detailRow.itemId;
        if (root.sourceCapability(detailRow.sourceId) === 2)
            refreshDetailSubscription();
    }
    function setDetailSubscription(subscribed) {
        if (!root.detailRow)
            return;
        const sourceId = root.detailRow.sourceId;
        const itemId = root.detailRow.itemId;
        subscriptionQuery.setSubscribed(sourceId, itemId, subscribed);
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
    function sortLabel() {
        if (sortOptions.length === 0)
            return qsTr("Sort");
        return W.I18n.tr(sortOptions[Math.max(0, Math.min(sortIndex, sortOptions.length - 1))].labelText);
    }
    function sourceCapability(id) {
        const s = sourceInfo(id);
        return s ? Number(s.remoteCapability ?? 0) : 0;
    }
    function sourceFilterOptions(id) {
        const options = [];
        for (const filter of sourceFilters(id)) {
            for (const option of filter.options ?? [])
                options.push(option);
        }
        return options;
    }
    function sourceFilters(id) {
        const s = sourceInfo(id);
        return s && s.filters ? s.filters : [];
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
    function sourceRemoteHint(id) {
        const s = sourceInfo(id);
        return s ? W.I18n.tr(s.remoteHintText) : "";
    }
    function toggleDiscoverTweakSheet() {
        if (root.discoverTweakSheet?.active) {
            root.discoverTweakSheet.close();
            return;
        }
        root.ensureDiscoverTweakSheet();
    }
    function tweakSettingsCategory(id) {
        const source = String(id ?? "");
        return source.length > 0 ? "DiscoverView/" + encodeURIComponent(source) + "/Tweak" : "DiscoverView";
    }

    padding: MD.MProp.size.isCompact ? 0 : 12
    showBackground: false

    contentItem: MD.SplitView {
        id: detailSplit

        handle: null
        orientation: Qt.Horizontal
        spacing: 24

        MD.Pane {
            MD.SplitViewBase.fillWidth: true
            padding: 0
            radius: root.MD.MProp.page.backgroundRadius
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

                        mdState.borderWidth: 1
                        text: root.sourceName(root.sourceId)
                        trailingIconName: MD.Token.icon.arrow_drop_down
                        visible: availabilityQuery.sources.length > 1

                        onClicked: sourceMenu.open()

                        MD.Menu {
                            id: sourceMenu

                            model: availabilityQuery.sources
                            parent: sourceChip
                            y: parent.height

                            contentDelegate: MD.MenuItem {
                                required property var modelData

                                icon.name: modelData.id === root.sourceId ? MD.Token.icon.check : ' '
                                text: W.I18n.tr(modelData.displayNameText) || modelData.name

                                onClicked: {
                                    root.setSource(modelData.id);
                                    sourceMenu.close();
                                }
                            }
                        }
                    }
                    MD.EmbedChip {
                        id: sortChip

                        mdState.borderWidth: 1
                        text: root.sortLabel()
                        trailingIconName: MD.Token.icon.arrow_drop_down
                        visible: root.sortOptions.length > 0

                        onClicked: sortMenu.open()

                        MD.Menu {
                            id: sortMenu

                            model: root.sortOptions
                            parent: sortChip
                            y: parent.height

                            contentDelegate: MD.MenuItem {
                                required property int index
                                required property var modelData

                                icon.name: index === root.sortIndex ? MD.Token.icon.check : ' '
                                text: W.I18n.tr(modelData.labelText)

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
                    running: visible
                    visible: searchQuery.querying && searchQuery.model.count > 0
                }
                Item {
                    Layout.fillHeight: true
                    Layout.fillWidth: true

                    W.WallpaperView {
                        id: m_grid

                        anchors.fill: parent
                        model: searchQuery.model
                        tweak: root.discoverTweakState

                        delegate: RemoteCard {
                            fallbackAspectRatio: root.discoverTweakState.itemAspectRatio
                            itemHeight: masonry ? height : m_grid.displayItemHeight
                            itemWidth: m_grid.displayItemWidth
                            masonry: m_grid.masonry
                            remoteCapability: root.sourceCapability(root.sourceId)
                            shimmer: m_grid.shimmer
                            spacing: m_grid.itemSpacing

                            onClicked: {
                                m_grid.currentIndex = index;
                                root.selectItem(index);
                            }
                        }

                        onActivated: index => root.selectItem(index)
                    }
                    MD.Button {
                        anchors.bottom: parent.bottom
                        anchors.bottomMargin: 16
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: qsTr("Retry")
                        visible: searchQuery.status === 3 && m_grid.count > 0

                        onClicked: searchQuery.retry()
                    }
                    ColumnLayout {
                        readonly property bool hasError: !searchQuery.querying && searchQuery.errorText.length > 0
                        readonly property var loginAction: root.currentSourceLoginAction()
                        readonly property string loginButtonLabel: {
                            const label = loginAction ? W.I18n.tr(loginAction.browseButtonLabelText).trim() : "";
                            return label.length > 0 ? label : qsTr("Log in");
                        }
                        readonly property string loginDescription: {
                            const browseDescription = loginAction ? W.I18n.tr(loginAction.browseDescriptionText).trim() : "";
                            if (browseDescription.length > 0)
                                return browseDescription;
                            const description = loginAction ? W.I18n.tr(loginAction.descriptionText).trim() : "";
                            return description.length > 0 ? description : qsTr("Log in to start browsing.");
                        }
                        readonly property string loginLabel: {
                            const label = loginAction ? W.I18n.tr(loginAction.labelText).trim() : "";
                            return label.length > 0 ? label : qsTr("Log in to %1").arg(root.sourceName(root.sourceId));
                        }
                        readonly property bool needsLogin: loginAction !== null

                        anchors.centerIn: parent
                        spacing: 12
                        visible: m_grid.count === 0
                        width: Math.min(parent.width - 48, 420)

                        MD.BusyIndicator {
                            Layout.alignment: Qt.AlignHCenter
                            running: searchQuery.querying
                            visible: running
                        }
                        MD.Icon {
                            Layout.alignment: Qt.AlignHCenter
                            color: MD.Token.color.on_surface_variant
                            name: MD.Token.icon.person
                            size: 40
                            visible: parent.needsLogin
                        }
                        MD.Label {
                            Layout.fillWidth: true
                            color: MD.Token.color.on_surface
                            horizontalAlignment: Text.AlignHCenter
                            text: parent.needsLogin ? parent.loginLabel : qsTr("Couldn't load this source")
                            typescale: MD.Token.typescale.title_medium
                            visible: parent.needsLogin || parent.hasError
                            wrapMode: Text.WordWrap
                        }
                        MD.Label {
                            Layout.fillWidth: true
                            color: MD.Token.color.on_surface_variant
                            elide: Text.ElideRight
                            font.capitalization: Font.MixedCase
                            horizontalAlignment: Text.AlignHCenter
                            maximumLineCount: 4
                            text: parent.needsLogin ? parent.loginDescription : searchQuery.errorText
                            typescale: MD.Token.typescale.body_medium
                            visible: parent.needsLogin || parent.hasError
                            wrapMode: Text.WordWrap
                        }
                        MD.Button {
                            Layout.alignment: Qt.AlignHCenter
                            mdState.type: MD.Enum.BtFilledTonal
                            text: parent.loginButtonLabel
                            visible: parent.needsLogin

                            onClicked: root.openRemoteManagement()
                        }
                        MD.Button {
                            Layout.alignment: Qt.AlignHCenter
                            text: qsTr("Retry")
                            visible: parent.hasError && !parent.needsLogin

                            onClicked: searchQuery.retry()
                        }
                        MD.Label {
                            Layout.alignment: Qt.AlignHCenter
                            color: MD.Token.color.on_surface_variant
                            text: qsTr("No wallpapers found")
                            typescale: MD.Token.typescale.body_large
                            visible: !searchQuery.querying && !parent.needsLogin && searchQuery.errorText.length === 0
                        }
                    }
                }
            }
        }
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
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready)
            reloadAll();
    }

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
    MD.Action {
        id: manageAction

        enabled: root.currentSourceConfig() !== null
        icon.name: MD.Token.icon.manage_accounts
        text: qsTr("Manage")

        onTriggered: root.openRemoteManagement()
    }
    MD.Action {
        id: tweakAction

        checked: root.isSheetActive(root.discoverTweakSheet)
        icon.name: MD.Token.icon.tune
        text: qsTr("Tweak")

        onTriggered: root.toggleDiscoverTweakSheet()
    }
    MD.Action {
        id: filterAction

        checked: searchQuery.tags.length > 0
        enabled: m_filter_dialog.filters.length > 0
        icon.name: MD.Token.icon.filter_alt
        text: qsTr("Filters")

        onTriggered: m_filter_dialog.open()
    }
    MD.Action {
        id: refreshAction

        enabled: searchQuery.browsingEnabled && !searchQuery.querying
        icon.name: MD.Token.icon.refresh
        text: qsTr("Refresh")

        onTriggered: searchQuery.reload()
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
    W.RemoteSearchQuery {
        id: searchQuery
    }
    W.RemoteFilterDialog {
        id: m_filter_dialog

        filters: root.sourceFilters(root.sourceId)
        parent: root.Window.window ? root.Window.window.contentItem : null
        popupWindow: root.Window.window
        positioningItem: overlayItem
        selectedValues: searchQuery.tags
        x: (overlayWidth - width) / 2
        y: (overlayHeight - height) / 2

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

        function onRejected(sourceId, id, error) {
            W.Action.toast(qsTr("Download rejected: ") + error);
        }
        function onRemoveFailed(sourceId, id, error) {
            W.Action.toast(qsTr("Remove failed: ") + error);
        }
        function onRemoved(sourceId, id) {
            W.Action.toast(qsTr("Removed"));
        }
    }
    W.RemoteSubscriptionQuery {
        id: subscriptionQuery

        forwardError: false
    }
    Connections {
        function onSetFinished(sourceId, id, subscribed, accepted, error) {
            if (accepted) {
                const message = subscribed ? qsTr("Subscription request accepted") : qsTr("Unsubscribe request accepted");
                const hint = root.sourceRemoteHint(sourceId);
                W.Action.toast(hint.length > 0 ? message + "\n" + hint : message);
            } else {
                W.Action.toast(qsTr("Subscription update failed: ") + W.I18n.tr(error));
            }
        }
        function onStateLoaded(sourceId, id, state, error) {
            const message = W.I18n.tr(error);
            if (message.length > 0)
                W.Action.toast(qsTr("Couldn't load subscription status: ") + message);
        }

        target: subscriptionQuery
    }
    Connections {
        function onRemoteDownloadProgress(sourceId, id, state, error) {
            if (state === 5 && error.length > 0)
                W.Action.toast(qsTr("Download failed: ") + error);
        }

        target: W.Notify
    }
    Connections {
        function onDaemonReady() {
            root.reloadAll();
        }
        function onPluginStateChanged() {
            availabilityQuery.reload();
        }
        function onSettingsChanged() {
            availabilityQuery.reload();
        }

        target: W.Notify
    }
    MD.AdaptivePresenter {
        id: detailPresenter

        content: root.detailPanel
        destination: !root.detailRow ? null : detailSplit.width >= MD.Token.window_class.expanded.min_width ? sideSite : sheetSite
        enabled: root.visible

        onDismissed: root.closeDetail()
    }
    MD.BottomSheet {
        id: detailSheet

        maxSheetWidth: 400
        nestedScrollEnabled: true
        parent: root
        preferredContentHeight: root.height
        wideSideMargin: 24

        MD.PresentationSite {
            id: sheetSite

            height: detailSheet.contentViewportHeight
            popup: detailSheet
            presenter: detailPresenter
            width: detailSheet.contentViewportWidth
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
