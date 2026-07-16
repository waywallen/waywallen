pragma ComponentBehavior: Bound
import QtQuick
import QtQml as Qml
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

Item {
    id: root

    property string wallpaperId: ""
    property var fallbackWallpaper: null
    property bool showApply: true

    signal back

    readonly property var wp: (wallpaperGetQuery.wallpaper?.id_proto ?? "") !== ""
                              ? wallpaperGetQuery.wallpaper
                              : root.fallbackWallpaper

    property var applyTargetIds: []
    property int rendererIndex: 0
    property string applyTarget: "desktop"
    readonly property bool showLockTarget: root.lockTargetIds.length > 0
    // Apply targets, plus the lock-capable subset used for the lock-screen target.
    readonly property var lockTargetIds: {
        const all = W.App.displayManager.displays || [];
        const targets = root.applyTargetIds.length === 0
            ? all
            : all.filter(d => root.applyTargetIds.indexOf(d.id) >= 0);
        return targets.filter(d => !!d && d.hasLockScreen === true).map(d => d.id);
    }

    function isTargetAll() { return root.applyTargetIds.length === 0; }
    function toggleTarget(id) {
        const next = root.applyTargetIds.slice();
        const i = next.indexOf(id);
        if (i >= 0) next.splice(i, 1);
        else next.push(id);
        root.applyTargetIds = next;
    }
    function infoSizeOf(w) {
        return m_list.data && w ? m_list.data.sizeOf(w) : 0;
    }
    function openInfo() {
        if (!root.wp)
            return;
        MD.Util.showPopup('waywallen.ui/PagePopup', {
            source: 'waywallen.ui/WallpaperInfoPage',
            props: {
                wallpaper: root.wp,
                sizeBytes: root.infoSizeOf(root.wp)
            }
        }, root.Window.window);
    }
    function containerFolderUrl(resource) {
        let path = String(resource || "");
        if (path.length === 0)
            return "";
        if (path.indexOf("file://") === 0)
            path = path.slice(7);
        const i = path.lastIndexOf("/");
        if (i <= 0)
            return "";
        return "file://" + path.slice(0, i).split("/").map(encodeURIComponent).join("/");
    }
    function openContainerFolder() {
        const url = root.containerFolderUrl(root.wp?.resource);
        if (url.length > 0)
            MD.Util.openFolderExternally(url);
    }

    readonly property var rendererCandidates: {
        const w = root.wp;
        if (!w) return [];
        const t = w.wpType || "";
        if (!t) return [];
        const list = (pluginQuery.renderers || []).filter(r => (r.types || []).indexOf(t) >= 0);
        list.sort((a, b) => (b.priority || 0) - (a.priority || 0));
        return list;
    }
    onRendererCandidatesChanged: root.rendererIndex = 0

    W.WallpaperGetQuery {
        id: wallpaperGetQuery
        wallpaperId: root.wallpaperId
    }

    W.WallpaperListQuery { id: m_list }
    W.RendererPluginListQuery { id: pluginQuery }
    W.WallpaperApplyQuery { id: applyQuery }
    W.WallpaperApplyViaPortalQuery { id: portalApplyQuery }
    W.DisplayLockWallpaperSetQuery { id: lockWallpaperQuery }
    W.WallpaperRemoveQuery {
        id: removeQuery
        wallpaperId: root.wallpaperId
    }

    Connections {
        target: lockWallpaperQuery
        function onStatusChanged() {
            if (lockWallpaperQuery.status === 2) {
                wallpaperGetQuery.reload();
            } else if (lockWallpaperQuery.status === 3) {
                const message = lockWallpaperQuery.error && lockWallpaperQuery.error.length > 0
                    ? lockWallpaperQuery.error
                    : qsTr("Apply failed");
                W.Action.toast(message, 6000, 1, null);
            }
        }
    }

    Connections {
        target: applyQuery
        function onStatusChanged() {
            if (applyQuery.status === 2) {
                wallpaperGetQuery.reload();
            } else if (applyQuery.status === 3) {
                const message = applyQuery.error && applyQuery.error.length > 0
                    ? applyQuery.error
                    : qsTr("Apply failed");
                W.Action.toast(message, 6000, 1, null);
            }
        }
    }

    Connections {
        target: portalApplyQuery
        function onStatusChanged() {
            if (portalApplyQuery.status === 3)
                W.Action.toast("Portal apply failed");
            else if (portalApplyQuery.status === 2)
                W.Action.toast("Wallpaper sent to desktop portal");
        }
    }

    Connections {
        target: removeQuery
        function onRemoved() {
            W.Action.toast("Wallpaper removed");
            root.back();
        }
        function onStatusChanged() {
            if (removeQuery.status === 3) {
                const message = removeQuery.error && removeQuery.error.length > 0
                    ? removeQuery.error
                    : qsTr("Remove failed");
                W.Action.toast(message, 6000, 1, null);
            }
        }
    }

    W.WallpaperPropertySetQuery {
        id: setQuery
    }

    Connections {
        target: W.Notify
        function onPluginChanged() {
            pluginQuery.reload();
        }
    }

    W.UserPropertyListModel {
        id: propertyModel
        schemaJson: wallpaperGetQuery.wallpaper?.userPropertiesSchema ?? ""
        overridesJson: wallpaperGetQuery.wallpaper?.userPropertyOverrides ?? ""
    }

    Component.onCompleted: pluginQuery.reload()

    QtObject {
        id: m_pending_writes
        property string wallpaperId: ""
        property var entries: ({})

        function queue(key, value, reset) {
            const e = entries;
            e[key] = { value: value, reset: reset };
            entries = e;
            if (!restartFlush())
                entries = {};
        }

        function restartFlush() {
            wallpaperId = root.wallpaperId;
            if (wallpaperId.length === 0)
                return false;
            m_flush_timer.restart();
            return true;
        }
    }

    Qml.Timer {
        id: m_flush_timer
        interval: 200
        repeat: false
        onTriggered: {
            const wallpaperId = m_pending_writes.wallpaperId;
            const e = m_pending_writes.entries;
            for (const k in e) {
                const operation = e[k];
                setQuery.wallpaperId = wallpaperId;
                setQuery.propertyKey = k;
                setQuery.propertyValue = operation.value;
                setQuery.reset = operation.reset;
                setQuery.reload();
            }
            m_pending_writes.entries = {};
        }
    }

    Connections {
        target: propertyModel
        function onValueChanged(key, value) {
            m_pending_writes.queue(key, value, false);
        }
        function onResetRequested(key) {
            m_pending_writes.queue(key, "", true);
        }
    }

    MD.Action {
        id: applyAction
        text: "Apply"
        busy: applyQuery.querying || lockWallpaperQuery.querying
        enabled: (W.App.displayManager.displays || []).length > 0
        onTriggered: {
            if (busy) return;
            if (!root.wp) return;
            const target = root.showLockTarget ? root.applyTarget : "desktop";
            if (target === "desktop" || target === "both") {
                applyQuery.wallpaper = root.wp;
                applyQuery.displayIds = root.applyTargetIds;
                if (root.rendererCandidates.length >= 2) {
                    const pick = root.rendererCandidates[root.rendererIndex];
                    applyQuery.rendererName = pick ? (pick.name || "") : "";
                } else {
                    applyQuery.rendererName = "";
                }
                applyQuery.reload();
            }
            if ((target === "lock" || target === "both") && root.lockTargetIds.length > 0) {
                // Only the lock-capable subset of the targeted displays.
                lockWallpaperQuery.displayIds = root.lockTargetIds;
                lockWallpaperQuery.wallpaperId = root.wp.id_proto;
                lockWallpaperQuery.reload();
            }
        }
    }

    MD.Action {
        id: applyViaPortalAction
        text: "Apply via desktop portal"
        busy: portalApplyQuery.querying
        onTriggered: {
            if (busy) return;
            if (!root.wp) return;
            portalApplyQuery.wallpaperId = root.wallpaperId;
            portalApplyQuery.reload();
        }
    }

    MD.Action {
        id: closeAction
        text: "Close"
        icon.name: MD.Token.icon.close
        onTriggered: root.back()
    }

    MD.Action {
        id: infoAction
        text: "Info"
        icon.name: MD.Token.icon.info
        enabled: (root.wp?.id_proto ?? "") !== ""
        onTriggered: root.openInfo()
    }

    MD.Action {
        id: openContainerFolderAction
        text: "Open containing folder"
        icon.name: MD.Token.icon.folder_open
        enabled: root.containerFolderUrl(root.wp?.resource).length > 0
        onTriggered: root.openContainerFolder()
    }

    MD.Action {
        id: removeAction
        text: "Remove"
        icon.name: MD.Token.icon.delete
        busy: removeQuery.querying
        enabled: (root.wp?.supportsItemRemove ?? false) && (root.wp?.id_proto ?? "") !== ""
        onTriggered: {
            if (busy)
                return;
            removeQuery.reload();
        }
    }

    readonly property MD.Action activeApplyAction:
        ((root.wp?.wpType ?? "") === "image"
            && (W.App.displayManager.displays || []).length === 0)
        ? applyViaPortalAction : applyAction

    readonly property list<MD.Action> detailActions:
        (root.wp?.supportsItemRemove ?? false)
        ? [removeAction, openContainerFolderAction, infoAction, closeAction]
        : [openContainerFolderAction, infoAction, closeAction]

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        MD.VerticalListView {
            id: m_detail_view
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    model: propertyModel
                    spacing: 8
                    leftMargin: 16
                    rightMargin: 16
            topMargin: 0
            bottomMargin: 8

            header: ColumnLayout {
                width: m_detail_view.contentWidth
                spacing: 12

                W.ThumbnailImage {
                    Layout.fillWidth: true
                    Layout.preferredHeight: visible ? 200 : 0
                    Layout.topMargin: 4
                    visible: (root.wp?.preview ?? "") !== ""
                             || (["video", "image"].indexOf(root.wp?.wpType ?? "") >= 0
                                 && (root.wp?.resource ?? "") !== "")
                    source: root.wp?.preview ?? ""
                    resource: root.wp?.resource ?? ""
                    wpType: root.wp?.wpType ?? ""
                    fillMode: Image.PreserveAspectFit
                }

                MD.Text {
                    Layout.fillWidth: true
                    text: root.wp?.name || "Untitled"
                    typescale: MD.Token.typescale.title_large
                    color: MD.Token.color.on_surface
                    wrapMode: Text.Wrap
                    maximumLineCount: 2
                    elide: Text.ElideRight
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    MD.Text {
                        Layout.fillWidth: true
                        text: root.wp?.wpType || ""
                        typescale: MD.Token.typescale.label_large
                        color: MD.Token.color.on_surface_variant
                        elide: Text.ElideRight
                        maximumLineCount: 1
                    }

                    Item {
                        id: detailActionBarHost
                        readonly property real targetWidth: Math.ceil(detailActionToolBar.maximumContentWidth) + 2
                        Layout.minimumWidth: targetWidth
                        Layout.preferredWidth: targetWidth
                        Layout.maximumWidth: targetWidth
                        Layout.preferredHeight: detailActionToolBar.implicitHeight
                        Layout.alignment: Qt.AlignVCenter

                        MD.ActionToolBar {
                            id: detailActionToolBar
                            anchors.fill: parent
                            actions: root.detailActions
                            iconDelegate: MD.SmallIconButton {
                                action: MD.ToolBarLayout.action
                            }
                        }
                    }
                }

                GridLayout {
                    id: m_meta
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 12
                    rowSpacing: 4

                    readonly property real sizeBytes: m_list.data && root.wp
                                                      ? m_list.data.sizeOf(root.wp)
                                                      : 0
                    readonly property bool hasPath: (root.wp?.resource ?? "") !== ""
                    readonly property bool hasResolution: Number(root.wp?.width ?? 0) > 0 && Number(root.wp?.height ?? 0) > 0
                    readonly property bool hasSize: sizeBytes > 0
                    readonly property bool hasFormat: (root.wp?.format ?? "") !== ""

                    function shortPath(p) {
                        const parts = (p || "").split("/").filter(s => s.length > 0);
                        return parts.slice(-2).join("/");
                    }
                    function formatSize(b) {
                        let v = Number(b ?? 0);
                        if (!(v > 0)) return "";
                        const u = ["B", "KB", "MB", "GB", "TB"];
                        let i = 0;
                        while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
                        return v.toFixed(i === 0 ? 0 : 1) + " " + u[i];
                    }

                    MD.Text {
                        visible: m_meta.hasPath
                        text: "Path"
                        typescale: MD.Token.typescale.label_medium
                        color: MD.Token.color.on_surface_variant
                    }
                    MD.Text {
                        visible: m_meta.hasPath
                        Layout.fillWidth: true
                        text: m_meta.shortPath(root.wp?.resource)
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface
                        elide: Text.ElideMiddle
                        maximumLineCount: 1
                        wrapMode: Text.NoWrap
                    }

                    MD.Text {
                        visible: m_meta.hasResolution
                        text: "Resolution"
                        typescale: MD.Token.typescale.label_medium
                        color: MD.Token.color.on_surface_variant
                    }
                    MD.Text {
                        visible: m_meta.hasResolution
                        text: (root.wp?.width ?? 0) + "×" + (root.wp?.height ?? 0)
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface
                    }

                    MD.Text {
                        visible: m_meta.hasSize
                        text: "Size"
                        typescale: MD.Token.typescale.label_medium
                        color: MD.Token.color.on_surface_variant
                    }
                    MD.Text {
                        visible: m_meta.hasSize
                        text: m_meta.formatSize(m_meta.sizeBytes)
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface
                    }

                    MD.Text {
                        visible: m_meta.hasFormat
                        text: "Format"
                        typescale: MD.Token.typescale.label_medium
                        color: MD.Token.color.on_surface_variant
                    }
                    MD.Text {
                        visible: m_meta.hasFormat
                        text: (root.wp?.format ?? "").toLowerCase()
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface
                    }
                }

                Flow {
                    Layout.fillWidth: true
                    spacing: 6
                    visible: (root.wp?.tags?.length ?? 0) > 0
                    Repeater {
                        model: root.wp?.tags ?? []
                        delegate: W.Tag {
                            required property string modelData
                            text: modelData
                        }
                    }
                }

                ColumnLayout {
                    id: m_description
                    Layout.fillWidth: true
                    spacing: 4
                    visible: (root.wp?.description ?? "") !== ""

                    property bool expanded: false
                    readonly property int collapsedLines: 3

                    MD.Divider { Layout.fillWidth: true }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        MD.Text {
                            Layout.fillWidth: true
                            text: "Description"
                            typescale: MD.Token.typescale.label_large
                            color: MD.Token.color.on_surface_variant
                        }
                        MD.IconButton {
                            icon.name: m_description.expanded ? MD.Token.icon.expand_less : MD.Token.icon.expand_more
                            visible: m_descText.lineCount > m_description.collapsedLines || m_description.expanded
                            onClicked: m_description.expanded = !m_description.expanded
                        }
                    }

                    MD.Text {
                        id: m_descText
                        Layout.fillWidth: true
                        text: W.Util.bbcodeToHtml(root.wp?.description ?? "")
                        textFormat: Text.StyledText
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface
                        wrapMode: Text.WordWrap
                        maximumLineCount: m_description.expanded ? Number.MAX_SAFE_INTEGER : m_description.collapsedLines
                        elide: m_description.expanded ? Text.ElideNone : Text.ElideRight
                        onLinkActivated: link => MD.Util.openUrlExternally(link)
                    }
                }

            }

            section.property: "section"
            section.criteria: ViewSection.FullString
            section.delegate: ColumnLayout {
                id: m_prop_section
                required property string section

                width: m_detail_view.contentWidth
                spacing: 4

                MD.Divider {
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 4

                    MD.Text {
                        Layout.fillWidth: true
                        text: m_prop_section.section
                        typescale: MD.Token.typescale.label_large
                        color: MD.Token.color.on_surface_variant
                    }

                    MD.IconButton {
                        icon.name: MD.Token.icon.restart_alt
                        mdState.size: MD.Enum.XS
                        enabled: m_prop_section.section === "Properties"
                            ? propertyModel.hasPredefinedPropertyOverrides
                            : propertyModel.hasUserPropertyOverrides
                        onClicked: {
                            if (m_prop_section.section === "Properties")
                                propertyModel.resetPredefinedProperties();
                            else
                                propertyModel.resetUserProperties();
                        }

                        MD.ToolTip {
                            visible: parent.hovered
                            text: "Reset to defaults"
                        }
                    }
                }
            }

            delegate: ColumnLayout {
                id: m_prop_delegate
                required property string key
                required property string label
                required property string type
                required property string section
                required property string kind
                required property bool   supported
                required property real   minVal
                required property real   maxVal
                required property string currentValue
                required property bool   hasAlpha
                required property var    optionLabels
                required property var    optionValues

                width: ListView.view ? (ListView.view.width - ListView.view.leftMargin - ListView.view.rightMargin) : 0
                spacing: 2

                function optionIndex(value) {
                    const values = m_prop_delegate.optionValues || [];
                    for (let i = 0; i < values.length; ++i) {
                        if (String(values[i]) === String(value))
                            return i;
                    }
                    return 0;
                }

                MD.TextEdit {
                    text: m_prop_delegate.label
                    textFormat: TextEdit.RichText
                    typescale: MD.Token.typescale.label_medium
                    color: MD.Token.color.on_surface
                    Layout.fillWidth: true
                    Layout.preferredHeight: Math.max(MD.Token.typescale.label_medium.line_height, contentHeight)
                    readOnly: true
                    selectByMouse: false
                    activeFocusOnPress: false
                    wrapMode: TextEdit.WordWrap
                    onLinkActivated: link => MD.Util.openUrlExternally(link)
                }

                MD.Switch {
                    id: m_switch
                    visible: m_prop_delegate.type === "bool"
                    onToggled: propertyModel.setValue(m_prop_delegate.key, checked ? "true" : "false")
                }
                Binding {
                    target: m_switch
                    property: "checked"
                    value: m_prop_delegate.currentValue === "true"
                }

                W.ValueSlider {
                    id: m_slider
                    visible: m_prop_delegate.type === "slider"
                    Layout.fillWidth: true
                    from: m_prop_delegate.minVal
                    to: m_prop_delegate.maxVal
                    stepSize: m_prop_delegate.maxVal > 10 ? 1 : 0
                    valueText: displayValue(value)
                    valueMaxText: {
                        const minText = displayValue(from);
                        const maxText = displayValue(to);
                        return minText.length > maxText.length ? minText : maxText;
                    }
                    valueTypescale: MD.Token.typescale.body_small
                    function displayValue(v) {
                        return Number(v).toFixed(m_prop_delegate.maxVal > 10 ? 0 : 3);
                    }
                    onMoved: propertyModel.setValue(m_prop_delegate.key, String(value))
                }
                Binding {
                    target: m_slider
                    property: "value"
                    value: Number(m_prop_delegate.currentValue)
                }

                MD.ColorPickerButton {
                    id: m_color
                    visible: m_prop_delegate.type === "color"
                    Layout.preferredWidth: 80
                    Layout.preferredHeight: 32
                    showAlpha: m_prop_delegate.hasAlpha
                    onAccepted: c => propertyModel.setValue(m_prop_delegate.key, W.Util.colorToWire(c, showAlpha))
                }
                Binding {
                    target: m_color
                    property: "color"
                    value: W.Util.colorFromWire(m_prop_delegate.currentValue)
                }

                MD.ComboBox {
                    id: m_combo
                    visible: m_prop_delegate.type === "combo" && m_prop_delegate.supported
                    Layout.fillWidth: true
                    model: m_prop_delegate.optionLabels || []
                    onActivated: idx => {
                        const values = m_prop_delegate.optionValues || [];
                        if (idx >= 0 && idx < values.length)
                            propertyModel.setValue(m_prop_delegate.key, String(values[idx]));
                    }
                }
                Binding {
                    target: m_combo
                    property: "currentIndex"
                    value: m_prop_delegate.optionIndex(m_prop_delegate.currentValue)
                }

                MD.TextField {
                    id: m_text_input
                    visible: m_prop_delegate.type === "textinput"
                    Layout.fillWidth: true
                    text: m_prop_delegate.currentValue
                    mdState.dense: true
                    onAccepted: submit()

                    function submit() {
                        if (text === m_prop_delegate.currentValue)
                            return;
                        propertyModel.setValue(m_prop_delegate.key, text);
                    }

                    trailing: MD.SmallIconButton {
                        anchors.right: parent?.right
                        anchors.verticalCenter: parent?.verticalCenter
                        anchors.rightMargin: 8
                        icon.name: MD.Token.icon.check
                        enabled: m_text_input.text !== m_prop_delegate.currentValue
                        onClicked: m_text_input.submit()

                        MD.ToolTip {
                            visible: parent.hovered
                            text: "Apply"
                        }
                    }
                }

                MD.Text {
                    visible: !m_prop_delegate.supported
                    text: "(" + m_prop_delegate.type + " — not yet supported)"
                    typescale: MD.Token.typescale.body_small
                    color: MD.Token.color.on_surface_variant
                }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            Layout.topMargin: 8
            Layout.bottomMargin: 8
            spacing: 8
            visible: root.showApply

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4
                visible: (W.App.displayManager.displays || []).length > 0

                MD.Text {
                    text: "Apply to"
                    typescale: MD.Token.typescale.label_medium
                    color: MD.Token.color.on_surface_variant
                }

                Flow {
                    Layout.fillWidth: true
                    spacing: 6

                    MD.FilterChip {
                        text: "All"
                        checked: root.isTargetAll()
                        onClicked: root.applyTargetIds = []
                    }

                    Repeater {
                        model: W.App.displayManager.displays
                        MD.FilterChip {
                            required property var modelData
                            text: (modelData?.displayLabel ?? "") || (modelData?.name ?? "").replace(/^waywallen-[a-z]+-[a-z]+-/, "") || ("Display " + modelData?.id)
                            checked: root.applyTargetIds.indexOf(modelData?.id) >= 0
                            onClicked: root.toggleTarget(modelData?.id)
                        }
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4
                visible: root.showLockTarget

                MD.Text {
                    text: "Set as"
                    typescale: MD.Token.typescale.label_medium
                    color: MD.Token.color.on_surface_variant
                }

                Flow {
                    Layout.fillWidth: true
                    spacing: 6

                    MD.FilterChip {
                        checkable: false
                        text: "Both"
                        checked: root.applyTarget === "both"
                        onClicked: root.applyTarget = "both"
                    }
                    MD.FilterChip {
                        checkable: false
                        text: "Desktop"
                        checked: root.applyTarget === "desktop"
                        onClicked: root.applyTarget = "desktop"
                    }
                    MD.FilterChip {
                        checkable: false
                        text: "Lock Screen"
                        checked: root.applyTarget === "lock"
                        onClicked: root.applyTarget = "lock"
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4
                visible: root.rendererCandidates.length >= 2

                MD.Text {
                    text: "Renderer"
                    typescale: MD.Token.typescale.label_medium
                    color: MD.Token.color.on_surface_variant
                }

                Flow {
                    Layout.fillWidth: true
                    spacing: 6
                    Repeater {
                        model: root.rendererCandidates
                        MD.FilterChip {
                            required property var modelData
                            required property int index
                            text: modelData?.name || ""
                            checked: root.rendererIndex === index
                            onClicked: root.rendererIndex = index
                        }
                    }
                }
            }

            MD.BusyButton {
                id: applyBtn
                Layout.fillWidth: true
                action: root.activeApplyAction
                mdState.type: MD.Enum.BtFilled

                MD.ToolTip {
                    visible: applyBtn.hovered && !applyBtn.enabled
                    text: "No display connected"
                }
            }
        }
    }
}
