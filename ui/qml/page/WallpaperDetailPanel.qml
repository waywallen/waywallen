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
    property var valueLabels: ({})
    property bool unsubscribeAccepted: false
    property var infoPresentation: null

    onWallpaperIdChanged: unsubscribeAccepted = false

    signal back

    readonly property var wp: (wallpaperGetQuery.wallpaper?.id_proto ?? "") !== "" ? wallpaperGetQuery.wallpaper : root.fallbackWallpaper

    property int rendererIndex: 0
    readonly property var kFillModeValues: [1, 2, 3, 7]
    readonly property var kFillModeLabels: [qsTr("Stretch"), qsTr("Fit"), qsTr("Crop"), qsTr("Center")]
    readonly property var kRotationValues: [1, 2, 3, 4]
    readonly property var kRotationLabels: ["0°", "90°", "180°", "270°"]
    readonly property bool wallpaperLayoutOverrideSet: root.wp?.wallpaperLayoutOverrideSet ?? false
    readonly property var wallpaperLayout: wallpaperLayoutOverrideSet ? (root.wp?.wallpaperLayoutOverride ?? ({})) : ({
            fillmode: 3,
            locationX: 50,
            locationY: 50,
            rotation: 1,
            locationSet: true
        })

    function fillmodeIndex(value) {
        const i = root.kFillModeValues.indexOf(value);
        return i < 0 ? 2 : i;
    }
    function clampPercent(value) {
        return Math.max(0, Math.min(100, Math.round(Number(value) || 0)));
    }
    function applyWallpaperLayout(fillmode, x, y, rotation) {
        if (!root.wp)
            return;
        layoutSetQuery.wallpaperId = root.wallpaperId;
        layoutSetQuery.clear = false;
        layoutSetQuery.fillmode = fillmode;
        layoutSetQuery.locationX = root.clampPercent(x);
        layoutSetQuery.locationY = root.clampPercent(y);
        layoutSetQuery.rotation = rotation;
        layoutSetQuery.reload();
    }
    function resetWallpaperLayout() {
        if (!root.wp)
            return;
        layoutSetQuery.wallpaperId = root.wallpaperId;
        layoutSetQuery.clear = true;
        layoutSetQuery.reload();
    }
    function displayLabelForId(id) {
        const display = W.App.displayManager.get(id);
        return display ? display.displayLabel : qsTr("Display #%1").arg(id);
    }
    function toastStoppedPlaylists() {
        const playlists = applyQuery.stoppedPlaylists || [];
        for (let i = 0; i < playlists.length; ++i) {
            const playlist = playlists[i];
            const name = playlist.playlistName || qsTr("Playlist #%1").arg(playlist.playlistId);
            if (playlist.allDisplays) {
                W.Action.toast(qsTr("Playlist \"%1\" stopped on all displays").arg(name));
                continue;
            }
            const labels = [];
            const ids = playlist.displayIds || [];
            for (let j = 0; j < ids.length; ++j)
                labels.push(root.displayLabelForId(ids[j]));
            W.Action.toast(qsTr("Playlist \"%1\" stopped on %2").arg(name).arg(labels.join(", ")));
        }
    }
    function infoSizeOf(w) {
        return m_list.data && w ? m_list.data.sizeOf(w) : 0;
    }
    function openInfo() {
        if (!root.wp)
            return;
        if (root.infoPresentation?.active)
            return;
        root.infoPresentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
            source: 'waywallen.ui/WallpaperInfoPage',
            props: {
                wallpaper: root.wp,
                sizeBytes: root.infoSizeOf(root.wp),
                valueLabels: root.valueLabels
            }
        });
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
        if (!w)
            return [];
        const t = w.wpType || "";
        if (!t)
            return [];
        const list = (pluginQuery.renderers || []).filter(r => (r.types || []).indexOf(t) >= 0);
        list.sort((a, b) => (b.priority || 0) - (a.priority || 0));
        return list;
    }
    onRendererCandidatesChanged: root.rendererIndex = 0

    W.WallpaperGetQuery {
        id: wallpaperGetQuery
        wallpaperId: root.wallpaperId
    }

    W.PresentationTargetState {
        id: applyTargetState
    }

    W.WallpaperListQuery {
        id: m_list
    }
    W.RendererPluginListQuery {
        id: pluginQuery
    }
    W.WallpaperApplyQuery {
        id: applyQuery
        forwardError: false
    }
    W.WallpaperApplyViaPortalQuery {
        id: portalApplyQuery
        forwardError: false
    }
    W.WallpaperRemoveQuery {
        id: removeQuery
        forwardError: false
        wallpaperId: root.wallpaperId
    }
    W.WallpaperUnsubscribeQuery {
        id: unsubscribeQuery
        forwardError: false
        wallpaperId: root.wallpaperId
    }

    Connections {
        target: applyQuery
        function onStatusChanged() {
            if (applyQuery.status === 2) {
                root.toastStoppedPlaylists();
                wallpaperGetQuery.reload();
            } else if (applyQuery.status === 3) {
                const message = applyQuery.error && applyQuery.error.length > 0 ? applyQuery.error : qsTr("Apply failed");
                W.Global.toastError(message);
            }
        }
    }

    Connections {
        target: portalApplyQuery
        function onStatusChanged() {
            if (portalApplyQuery.status === 3)
                W.Action.toast(qsTr("Portal apply failed"));
            else if (portalApplyQuery.status === 2)
                W.Action.toast(qsTr("Wallpaper sent to desktop portal"));
        }
    }

    Connections {
        target: removeQuery
        function onRemoved() {
            W.Action.toast(qsTr("Wallpaper removed"));
            root.back();
        }
        function onStatusChanged() {
            if (removeQuery.status === 3) {
                const message = removeQuery.error && removeQuery.error.length > 0 ? removeQuery.error : qsTr("Remove failed");
                W.Action.toast(message, 6000, 1, null);
            }
        }
    }

    Connections {
        target: unsubscribeQuery
        function onUnsubscribed() {
            root.unsubscribeAccepted = true;
            W.Action.toast(qsTr("Unsubscribed"));
        }
        function onStatusChanged() {
            if (unsubscribeQuery.status === 3) {
                const message = unsubscribeQuery.error && unsubscribeQuery.error.length > 0 ? unsubscribeQuery.error : qsTr("Unsubscribe failed");
                W.Action.toast(message, 6000, 1, null);
            }
        }
    }

    W.WallpaperPropertySetQuery {
        id: setQuery
    }

    W.WallpaperLayoutSetQuery {
        id: layoutSetQuery
        forwardError: false
        wallpaperId: root.wallpaperId
    }

    Connections {
        target: layoutSetQuery
        function onStatusChanged() {
            if (layoutSetQuery.status === 2)
                wallpaperGetQuery.reload();
            else if (layoutSetQuery.status === 3)
                W.Action.toast(qsTr("Layout update failed"));
        }
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
            e[key] = {
                value: value,
                reset: reset
            };
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
        text: qsTr("Apply")
        busy: applyQuery.querying
        enabled: applyTargetState.hasSelection
        onTriggered: {
            if (busy)
                return;
            if (!root.wp)
                return;
            applyQuery.wallpaper = root.wp;
            applyQuery.targets = applyTargetState.wireTargets;
            if (root.rendererCandidates.length >= 2) {
                const pick = root.rendererCandidates[root.rendererIndex];
                applyQuery.rendererName = pick ? (pick.name || "") : "";
            } else {
                applyQuery.rendererName = "";
            }
            applyQuery.reload();
        }
    }

    MD.Action {
        id: applyViaPortalAction
        text: qsTr("Apply via desktop portal")
        busy: portalApplyQuery.querying
        onTriggered: {
            if (busy)
                return;
            if (!root.wp)
                return;
            portalApplyQuery.wallpaperId = root.wallpaperId;
            portalApplyQuery.reload();
        }
    }

    MD.Action {
        id: linkAction
        text: qsTr("Open web page")
        icon.name: MD.Token.icon.web
        visible: String(root.wp?.webUrl ?? "").length > 0
        onTriggered: MD.Util.openUrlExternally(root.wp.webUrl)
    }

    MD.Action {
        id: infoAction
        text: qsTr("Info")
        icon.name: MD.Token.icon.info
        enabled: (root.wp?.id_proto ?? "") !== ""
        onTriggered: root.openInfo()
    }

    MD.Action {
        id: openContainerFolderAction
        text: qsTr("Open containing folder")
        icon.name: MD.Token.icon.folder_open
        enabled: root.containerFolderUrl(root.wp?.resource).length > 0
        onTriggered: root.openContainerFolder()
    }

    MD.Action {
        id: removeAction
        text: qsTr("Remove")
        icon.name: MD.Token.icon.delete
        busy: removeQuery.querying
        enabled: (root.wp?.supportsItemRemove ?? false) && (root.wp?.id_proto ?? "") !== ""
        onTriggered: {
            if (busy)
                return;
            removeQuery.reload();
        }
    }

    MD.Action {
        id: unsubscribeAction
        text: root.unsubscribeAccepted ? qsTr("Unsubscribed") : qsTr("Unsubscribe")
        icon.name: "unsubscribe"
        busy: unsubscribeQuery.querying
        enabled: (root.wp?.supportsItemUnsubscribe ?? false) && !root.unsubscribeAccepted && (root.wp?.id_proto ?? "") !== ""
        onTriggered: {
            if (busy)
                return;
            unsubscribeQuery.reload();
        }
    }

    readonly property MD.Action activeApplyAction: ((root.wp?.wpType ?? "") === "image" && (W.App.displayManager.displays || []).length === 0) ? applyViaPortalAction : applyAction

    readonly property list<MD.Action> detailActions: (root.wp?.supportsItemUnsubscribe ?? false) ? [unsubscribeAction, linkAction, openContainerFolderAction, infoAction] : (root.wp?.supportsItemRemove ?? false) ? [removeAction, linkAction, openContainerFolderAction, infoAction] : [linkAction, openContainerFolderAction, infoAction]

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
                    visible: (root.wp?.preview ?? "") !== "" || (["video", "image"].indexOf(root.wp?.wpType ?? "") >= 0 && (root.wp?.resource ?? "") !== "")
                    source: root.wp?.preview ?? ""
                    resource: root.wp?.resource ?? ""
                    wpType: root.wp?.wpType ?? ""
                    fillMode: Image.PreserveAspectFit
                }

                MD.Text {
                    Layout.fillWidth: true
                    text: root.wp?.name || qsTr("Untitled")
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

                    RowLayout {
                        spacing: 0

                        W.DetailActionBar {
                            actions: root.detailActions
                        }

                        MD.SmallIconButton {
                            icon.name: MD.Token.icon.close
                            hoverEnabled: true
                            MD.ToolTip.text: qsTr("Close")
                            MD.ToolTip.visible: hovered && !pressed
                            onClicked: root.back()
                        }
                    }
                }

                GridLayout {
                    id: m_meta
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 12
                    rowSpacing: 4

                    readonly property real sizeBytes: m_list.data && root.wp ? m_list.data.sizeOf(root.wp) : 0
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
                        if (!(v > 0))
                            return "";
                        const u = ["B", "KB", "MB", "GB", "TB"];
                        let i = 0;
                        while (v >= 1024 && i < u.length - 1) {
                            v /= 1024;
                            i++;
                        }
                        return v.toFixed(i === 0 ? 0 : 1) + " " + u[i];
                    }

                    MD.Text {
                        visible: m_meta.hasPath
                        text: qsTr("Path")
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
                        text: qsTr("Resolution")
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
                        text: qsTr("Size")
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
                        text: qsTr("Format")
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
                            text: W.I18n.valueLabel(root.valueLabels, modelData)
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

                    MD.Divider {
                        Layout.fillWidth: true
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 4
                        MD.Text {
                            Layout.fillWidth: true
                            text: qsTr("Description")
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

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    visible: (root.wp?.id_proto ?? "") !== ""

                    MD.Divider {
                        Layout.fillWidth: true
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 4

                        MD.Text {
                            Layout.fillWidth: true
                            text: qsTr("Layout override")
                            typescale: MD.Token.typescale.label_large
                            color: MD.Token.color.on_surface_variant
                        }

                        MD.IconButton {
                            icon.name: MD.Token.icon.restart_alt
                            mdState.size: MD.Enum.XS
                            enabled: root.wallpaperLayoutOverrideSet
                            onClicked: root.resetWallpaperLayout()
                            MD.ToolTip.visible: hovered
                            MD.ToolTip.text: qsTr("Reset to display layout")
                        }
                    }

                    Flow {
                        id: m_wallpaper_layout_flow
                        Layout.fillWidth: true
                        spacing: 12

                        readonly property var layout: root.wallpaperLayout || ({})
                        readonly property int currentFillmode: Number(layout.fillmode ?? 3)
                        readonly property int currentRotation: Number(layout.rotation ?? 1)
                        readonly property int currentX: root.clampPercent(layout.locationX ?? 50)
                        readonly property int currentY: root.clampPercent(layout.locationY ?? 50)
                        readonly property bool locationEnabled: currentFillmode !== 1

                        ColumnLayout {
                            width: Math.min(m_wallpaper_layout_flow.width, 220)
                            spacing: 4

                            MD.Text {
                                text: qsTr("Fill mode")
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }

                            MD.ComboBox {
                                Layout.fillWidth: true
                                mdState.size: MD.Enum.S
                                model: root.kFillModeLabels
                                currentIndex: root.fillmodeIndex(m_wallpaper_layout_flow.currentFillmode)
                                onActivated: idx => {
                                    root.applyWallpaperLayout(root.kFillModeValues[idx], m_wallpaper_layout_flow.currentX, m_wallpaper_layout_flow.currentY, m_wallpaper_layout_flow.currentRotation);
                                }
                            }
                        }

                        ColumnLayout {
                            width: Math.min(m_wallpaper_layout_flow.width, 260)
                            spacing: 4
                            enabled: m_wallpaper_layout_flow.locationEnabled
                            opacity: enabled ? 1.0 : 0.4

                            MD.Text {
                                text: qsTr("Horizontal")
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }

                            W.ValueSlider {
                                id: wallpaperHorizontalLocation
                                Layout.fillWidth: true
                                from: 0
                                to: 100
                                stepSize: 1
                                value: m_wallpaper_layout_flow.currentX
                                valueText: root.clampPercent(value)
                                valueMaxText: root.clampPercent(to).toString()
                                valueHorizontalAlignment: Text.AlignLeft
                                onMoved: root.applyWallpaperLayout(m_wallpaper_layout_flow.currentFillmode, value, wallpaperVerticalLocation.value, m_wallpaper_layout_flow.currentRotation)
                            }
                        }

                        ColumnLayout {
                            width: Math.min(m_wallpaper_layout_flow.width, 260)
                            spacing: 4
                            enabled: m_wallpaper_layout_flow.locationEnabled
                            opacity: enabled ? 1.0 : 0.4

                            MD.Text {
                                text: qsTr("Vertical")
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }

                            W.ValueSlider {
                                id: wallpaperVerticalLocation
                                Layout.fillWidth: true
                                from: 0
                                to: 100
                                stepSize: 1
                                value: m_wallpaper_layout_flow.currentY
                                valueText: root.clampPercent(value)
                                valueMaxText: root.clampPercent(to).toString()
                                valueHorizontalAlignment: Text.AlignLeft
                                onMoved: root.applyWallpaperLayout(m_wallpaper_layout_flow.currentFillmode, wallpaperHorizontalLocation.value, value, m_wallpaper_layout_flow.currentRotation)
                            }
                        }

                        ColumnLayout {
                            width: Math.min(m_wallpaper_layout_flow.width, implicitWidth)
                            spacing: 4

                            MD.Text {
                                text: qsTr("Rotation")
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }

                            MD.SegmentedButtonGroup {
                                id: wallpaperRotationGroup
                                size: MD.Enum.XS

                                function applyRotation(rotationValue) {
                                    root.applyWallpaperLayout(m_wallpaper_layout_flow.currentFillmode, m_wallpaper_layout_flow.currentX, m_wallpaper_layout_flow.currentY, rotationValue);
                                }
                                function isChecked(rotationValue) {
                                    return m_wallpaper_layout_flow.currentRotation === rotationValue;
                                }

                                MD.SegmentedButton {
                                    text: root.kRotationLabels[0]
                                    checked: wallpaperRotationGroup.isChecked(root.kRotationValues[0])
                                    onClicked: wallpaperRotationGroup.applyRotation(root.kRotationValues[0])
                                }
                                MD.SegmentedButton {
                                    text: root.kRotationLabels[1]
                                    checked: wallpaperRotationGroup.isChecked(root.kRotationValues[1])
                                    onClicked: wallpaperRotationGroup.applyRotation(root.kRotationValues[1])
                                }
                                MD.SegmentedButton {
                                    text: root.kRotationLabels[2]
                                    checked: wallpaperRotationGroup.isChecked(root.kRotationValues[2])
                                    onClicked: wallpaperRotationGroup.applyRotation(root.kRotationValues[2])
                                }
                                MD.SegmentedButton {
                                    text: root.kRotationLabels[3]
                                    checked: wallpaperRotationGroup.isChecked(root.kRotationValues[3])
                                    onClicked: wallpaperRotationGroup.applyRotation(root.kRotationValues[3])
                                }
                            }
                        }
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
                        text: m_prop_section.section === "property" ? qsTr("Properties") : qsTr("User properties")
                        typescale: MD.Token.typescale.label_large
                        color: MD.Token.color.on_surface_variant
                    }

                    MD.IconButton {
                        icon.name: MD.Token.icon.restart_alt
                        mdState.size: MD.Enum.XS
                        enabled: m_prop_section.section === "property" ? propertyModel.hasPredefinedPropertyOverrides : propertyModel.hasUserPropertyOverrides
                        onClicked: {
                            if (m_prop_section.section === "property")
                                propertyModel.resetPredefinedProperties();
                            else
                                propertyModel.resetUserProperties();
                        }
                        MD.ToolTip.visible: hovered
                        MD.ToolTip.text: qsTr("Reset to defaults")
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
                required property bool supported
                required property real minVal
                required property real maxVal
                required property real stepVal
                required property string valueSuffix
                required property string currentValue
                required property bool hasAlpha
                required property var optionLabels
                required property var optionValues

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
                    stepSize: m_prop_delegate.stepVal > 0 ? m_prop_delegate.stepVal : (m_prop_delegate.maxVal > 10 ? 1 : 0)
                    valueText: displayValue(value)
                    valueMaxText: {
                        const minText = displayValue(from);
                        const maxText = displayValue(to);
                        return minText.length > maxText.length ? minText : maxText;
                    }
                    valueTypescale: MD.Token.typescale.body_small
                    function displayValue(v) {
                        return Number(v).toFixed(m_prop_delegate.maxVal > 10 ? 0 : 3) + m_prop_delegate.valueSuffix;
                    }
                    onMoved: propertyModel.setValue(m_prop_delegate.key, String(value))
                }
                Binding {
                    target: m_slider
                    property: "value"
                    value: {
                        const minimum = Math.min(m_slider.from, m_slider.to);
                        const maximum = Math.max(m_slider.from, m_slider.to);
                        const current = Number(m_prop_delegate.currentValue);
                        return Number.isFinite(current) ? Math.max(minimum, Math.min(maximum, current)) : minimum;
                    }
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
                    mdState.size: MD.Enum.S
                    popupMaximumHeight: Math.min(320, (Window.window?.height ?? 480) * 0.45)
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
                    mdState.size: MD.Enum.S
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
                        MD.ToolTip.visible: hovered
                        MD.ToolTip.text: qsTr("Apply")
                    }
                }

                MD.Text {
                    visible: !m_prop_delegate.supported
                    text: qsTr("(%1 — not yet supported)").arg(m_prop_delegate.type)
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
                visible: applyTargetState.hasTargets

                MD.Text {
                    text: qsTr("Apply to")
                    typescale: MD.Token.typescale.label_medium
                    color: MD.Token.color.on_surface_variant
                }

                W.PresentationTargetFlow {
                    Layout.fillWidth: true
                    enabled: !applyQuery.querying
                    targetState: applyTargetState
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4
                visible: root.rendererCandidates.length >= 2

                MD.Text {
                    text: qsTr("Renderer")
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
                MD.ToolTip.visible: applyBtn.hovered && !applyBtn.enabled
                MD.ToolTip.text: qsTr("No display connected")
            }
        }
    }
}
