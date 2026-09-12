pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    title: qsTr('Plugins')
    scrolling: !pluginListView.atYBeginning
    readonly property int inactivePluginCount: (pluginListQuery.inactiveSystem ? pluginListQuery.inactiveSystem.length : 0) + (pluginListQuery.inactiveUser ? pluginListQuery.inactiveUser.length : 0)
    readonly property int pluginUpdateStateUnknown: 1
    readonly property int pluginUpdateStateNoUrl: 2
    readonly property int pluginUpdateStateChecking: 3
    readonly property int pluginUpdateStateUpToDate: 4
    readonly property int pluginUpdateStateAvailable: 5
    readonly property int pluginUpdateStateFailed: 6
    readonly property int pluginUpdateStateUnsupported: 7
    property var inactivePresentation: null

    function openInactiveDialog() {
        if (root.inactivePresentation?.active)
            return;
        root.inactivePresentation = root.Window.window.presentPopup(inactiveDialogComponent);
    }

    function updateState(info) {
        return info && info.state !== undefined ? info.state : pluginUpdateStateUnknown;
    }

    function updateTagVisible(info) {
        const state = updateState(info);
        return state !== pluginUpdateStateUnknown && state !== pluginUpdateStateNoUrl;
    }

    function updateActionVisible(info) {
        return updateState(info) === pluginUpdateStateAvailable && !!info && !!info.zipUrl && info.zipUrl.length > 0;
    }

    function installUpdate(pluginId, info) {
        if (!root.updateActionVisible(info) || !pluginId || pluginId.length === 0)
            return;
        updateInstallQuery.install(pluginId);
    }

    function updateTagText(info) {
        const state = updateState(info);
        if (state === pluginUpdateStateChecking)
            return qsTr("Checking");
        if (state === pluginUpdateStateUpToDate)
            return qsTr("Up to date");
        if (state === pluginUpdateStateAvailable) {
            const latest = info.latestVersion || "";
            if (latest.length === 0)
                return qsTr("Update available");
            return latest.startsWith("v") || latest.startsWith("V") ? qsTr("New %1").arg(latest) : qsTr("New v%1").arg(latest);
        }
        if (state === pluginUpdateStateFailed)
            return qsTr("Check failed");
        if (state === pluginUpdateStateUnsupported)
            return qsTr("Unsupported update");
        return "";
    }

    // The daemon already reports why a check failed; surface it instead of
    // leaving the tag unexplained. The message is backend text and is shown as
    // sent, only folded so a long manifest URL cannot stretch the tooltip past
    // the window.
    function updateTagTooltip(info) {
        const state = updateState(info);
        if (state !== pluginUpdateStateFailed && state !== pluginUpdateStateUnsupported)
            return "";
        return foldMessage(info && info.error ? String(info.error) : "", 64);
    }

    function foldMessage(message, limit) {
        const lines = [];
        for (let word of message.split(" ")) {
            while (word.length > limit) {
                lines.push(word.substring(0, limit));
                word = word.substring(limit);
            }
            if (word.length === 0)
                continue;
            const last = lines.length - 1;
            if (last >= 0 && lines[last].length + 1 + word.length <= limit)
                lines[last] += " " + word;
            else
                lines.push(word);
        }
        return lines.join("\n");
    }

    function updateTagBgColor(info) {
        const state = updateState(info);
        if (state === pluginUpdateStateAvailable)
            return MD.Token.color.primary_container;
        if (state === pluginUpdateStateFailed || state === pluginUpdateStateUnsupported)
            return MD.Token.color.error_container;
        return MD.Token.color.secondary_container;
    }

    function updateTagFgColor(info) {
        const state = updateState(info);
        if (state === pluginUpdateStateFailed || state === pluginUpdateStateUnsupported)
            return MD.Token.color.on_error_container;
        if (state === pluginUpdateStateAvailable)
            return MD.Token.color.on_primary_container;
        return MD.Token.color.on_secondary_container;
    }

    actions: [
        MD.Action {
            icon.name: MD.Token.icon.warning
            text: qsTr("Inactive plugins")
            visible: root.inactivePluginCount > 0
            onTriggered: root.openInactiveDialog()
        },
        MD.Action {
            icon.name: "update"
            text: qsTr("Check updates")
            enabled: !updateCheckQuery.querying && !updateInstallQuery.querying
            onTriggered: updateCheckQuery.check()
        },
        MD.Action {
            icon.name: MD.Token.icon.add
            text: qsTr("Install from .zip")
            enabled: !installQuery.querying && !inspectQuery.querying && !updateInstallQuery.querying
            onTriggered: zipDialog.open()
        }
    ]

    W.PluginListQuery {
        id: pluginListQuery
    }

    W.PluginInstallQuery {
        id: installQuery
        forwardError: false
    }

    W.PluginInspectQuery {
        id: inspectQuery
        forwardError: false
    }

    W.PluginDeleteQuery {
        id: deleteQuery
    }

    W.PluginUpdateCheckQuery {
        id: updateCheckQuery
    }

    W.PluginUpdateInstallQuery {
        id: updateInstallQuery
        forwardError: false
    }

    Connections {
        target: W.Notify
        function onDaemonReady() {
            pluginListQuery.reload();
        }
        function onPluginUpdateChanged() {
            pluginListQuery.reload();
        }
        function onPluginChanged() {
            pluginListQuery.reload();
        }
    }

    Connections {
        target: deleteQuery
        function onDeleted(pluginId, needsRestart) {
            W.Action.toast(needsRestart ? qsTr("Deleted \"%1\" — restart waywallen to unload it").arg(pluginId) : qsTr("Deleted \"%1\"").arg(pluginId));
            pluginListQuery.reload();
        }
    }

    Connections {
        target: installQuery
        function onInstalled(pluginId, needsRestart) {
            W.Action.toast(needsRestart ? qsTr("Installed \"%1\" — restart waywallen to load it").arg(pluginId) : qsTr("Installed \"%1\"").arg(pluginId));
            pluginListQuery.reload();
        }
        function onStatusChanged(status) {
            if (status !== 3)
                return;
            const message = installQuery.error && installQuery.error.length > 0 ? installQuery.error : qsTr("Plugin install failed");
            W.Action.toast(message, 6000, 1, null);
        }
    }

    Connections {
        target: updateInstallQuery
        function onInstalled(pluginId) {
            W.Action.toast(qsTr("Updated \"%1\"").arg(pluginId));
            pluginListQuery.reload();
        }
        function onStatusChanged(status) {
            if (status !== 3)
                return;
            const message = updateInstallQuery.error && updateInstallQuery.error.length > 0 ? updateInstallQuery.error : qsTr("Plugin update failed");
            W.Action.toast(message, 6000, 1, null);
        }
    }

    Connections {
        target: inspectQuery
        function onInspected() {
            installDialog.open();
        }
        function onStatusChanged(status) {
            if (status !== 3)
                return;
            const message = inspectQuery.error && inspectQuery.error.length > 0 ? inspectQuery.error : qsTr("Plugin package inspect failed");
            W.Action.toast(message, 6000, 1, null);
        }
    }

    Component.onCompleted: {
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready)
            pluginListQuery.reload();
    }
    Component.onDestruction: root.inactivePresentation?.cancel()

    Component {
        id: inactiveDialogComponent

        MD.Dialog {
            title: qsTr("Inactive plugins")
            horizontalPadding: 16
            implicitWidth: Math.min(440, parent ? parent.width - 48 : 440)
            standardButtons: T.Dialog.Close

            contentItem: ColumnLayout {
                spacing: 12

                MD.Text {
                    Layout.fillWidth: true
                    text: qsTr("These plugins were skipped because another installed plugin with the same id was selected. Higher versions win; when versions match, user plugins win over system plugins.")
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                    wrapMode: Text.WordWrap
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    visible: pluginListQuery.inactiveUser && pluginListQuery.inactiveUser.length > 0

                    MD.Text {
                        Layout.fillWidth: true
                        text: qsTr("User")
                        typescale: MD.Token.typescale.title_small
                        color: MD.Token.color.on_surface
                    }

                    Flow {
                        Layout.fillWidth: true
                        spacing: 6
                        Repeater {
                            model: pluginListQuery.inactiveUser
                            delegate: W.Tag {
                                required property var modelData
                                text: modelData
                            }
                        }
                    }
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 6
                    visible: pluginListQuery.inactiveSystem && pluginListQuery.inactiveSystem.length > 0

                    MD.Text {
                        Layout.fillWidth: true
                        text: qsTr("System")
                        typescale: MD.Token.typescale.title_small
                        color: MD.Token.color.on_surface
                    }

                    Flow {
                        Layout.fillWidth: true
                        spacing: 6
                        Repeater {
                            model: pluginListQuery.inactiveSystem
                            delegate: W.Tag {
                                required property var modelData
                                text: modelData
                            }
                        }
                    }
                }
            }
        }
    }

    MD.FileDialog {
        id: zipDialog
        title: qsTr("Choose plugin package")
        fileMode: MD.FileDialog.OpenFile
        nameFilters: [qsTr("Plugin package (*.zip)"), qsTr("All files (*)")]
        onAccepted: {
            inspectQuery.zipPath = selectedFile.toString().replace(/^file:\/\//, "");
            inspectQuery.reload();
        }
    }

    MD.Dialog {
        id: installDialog
        title: inspectQuery.overwrite ? qsTr("Update plugin?") : qsTr("Install plugin?")
        parent: T.Overlay.overlay
        horizontalPadding: 16
        implicitWidth: Math.min(440, parent ? parent.width - 48 : 440)
        standardButtons: T.Dialog.Cancel | T.Dialog.Ok

        contentItem: ColumnLayout {
            spacing: 12

            GridLayout {
                Layout.fillWidth: true
                columns: 2
                columnSpacing: 12
                rowSpacing: 8

                MD.Text {
                    text: qsTr("Name")
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                }
                MD.Text {
                    Layout.fillWidth: true
                    text: inspectQuery.name || inspectQuery.pluginId
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface
                    wrapMode: Text.WordWrap
                }

                MD.Text {
                    text: qsTr("Id")
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                }
                MD.Text {
                    Layout.fillWidth: true
                    text: inspectQuery.pluginId
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface
                    wrapMode: Text.WrapAnywhere
                }

                MD.Text {
                    text: qsTr("Version")
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                }
                MD.Text {
                    Layout.fillWidth: true
                    text: inspectQuery.overwrite ? qsTr("%1 -> %2").arg(inspectQuery.existingVersion || qsTr("unknown")).arg(inspectQuery.version || qsTr("unknown")) : ("v" + (inspectQuery.version || "0.0.0"))
                    typescale: MD.Token.typescale.body_medium
                    color: inspectQuery.overwrite ? MD.Token.color.primary : MD.Token.color.on_surface
                    wrapMode: Text.WordWrap
                }

                MD.Text {
                    text: qsTr("Source")
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                }
                MD.Text {
                    Layout.fillWidth: true
                    text: inspectQuery.hasSource ? qsTr("Yes") : qsTr("No")
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface
                }
            }

            Flow {
                Layout.fillWidth: true
                spacing: 6
                visible: inspectQuery.renderers && inspectQuery.renderers.length > 0

                Repeater {
                    model: inspectQuery.renderers
                    delegate: W.Tag {
                        required property var modelData
                        text: modelData
                    }
                }
            }

            MD.Text {
                Layout.fillWidth: true
                visible: !inspectQuery.overwrite && inspectQuery.existingSystem && inspectQuery.existingVersion.length > 0
                text: qsTr("A system plugin with the same id is active. Installing this package may replace it with the user plugin version %1.").arg(inspectQuery.version || qsTr("unknown"))
                typescale: MD.Token.typescale.body_medium
                color: MD.Token.color.on_surface_variant
                wrapMode: Text.WordWrap
            }
        }

        onAccepted: {
            installQuery.zipPath = inspectQuery.zipPath;
            installQuery.reload();
        }
    }

    contentItem: MD.VerticalListView {
        id: pluginListView
        expand: true
        leftMargin: 12
        rightMargin: 12
        bottomMargin: 12
        spacing: 4

        model: pluginListQuery.plugins

        header: MD.Text {
            width: pluginListView.contentWidth
            visible: !pluginListQuery.plugins || pluginListQuery.plugins.length === 0
            height: visible ? implicitHeight : 0
            text: qsTr("No plugins installed")
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.on_surface_variant
            wrapMode: Text.WordWrap
        }

        section.property: "section"
        section.criteria: ViewSection.FullString
        section.delegate: MD.Text {
            required property string section
            width: pluginListView.contentWidth
            text: section === "user" ? qsTr("User") : qsTr("System")
            typescale: MD.Token.typescale.title_small
            color: MD.Token.color.on_surface_variant
            topPadding: 4
            bottomPadding: 4
            leftPadding: 4
        }

        delegate: MD.ListItem {
            id: pluginItem
            required property var modelData

            width: pluginListView.contentWidth
            radius: 12
            mdState.backgroundColor: MD.Token.color.surface_container
            text: modelData.name || modelData.id || ""
            supportText: modelData.id
            leader: MD.Icon {
                name: MD.Token.icon.extension
                size: 24
                color: MD.Token.color.on_surface_variant
            }
            trailing: Item {
                readonly property real actionButtonWidth: 40
                readonly property int visibleActionCount: (pluginUpdateAction.visible ? 1 : 0) + (pluginDeleteAction.visible ? 1 : 0)
                readonly property bool hasOverflow: visibleActionCount >= 2
                readonly property real actionAreaWidth: visibleActionCount > 0 ? actionButtonWidth * (hasOverflow ? 2 : 1) : 0
                readonly property real actionAreaHeight: visibleActionCount > 0 ? pluginFloatingTags.implicitHeight + 4 + pluginActionToolBar.implicitHeight : 0

                implicitWidth: actionAreaWidth
                implicitHeight: actionAreaHeight

                MD.ActionToolBar {
                    id: pluginActionToolBar
                    anchors.right: parent.right
                    y: pluginFloatingTags.implicitHeight + 4
                    visible: pluginUpdateAction.visible || pluginDeleteAction.visible
                    width: parent.actionAreaWidth
                    actions: [pluginUpdateAction, pluginDeleteAction]
                    iconDelegate: MD.BusyIconButton {
                        action: MD.ToolBarLayout.action
                        mdState.size: MD.Enum.XS
                    }
                    moreDelegate: MD.IconButton {
                        action: pluginActionToolBar.moreAction
                        mdState.size: MD.Enum.XS
                    }
                }

                MD.Action {
                    id: pluginUpdateAction
                    text: updateInstallQuery.pluginId === pluginItem.modelData.id && updateInstallQuery.querying ? qsTr("Updating") : qsTr("Update")
                    icon.name: "download"
                    visible: root.updateActionVisible(pluginItem.modelData.updateInfo)
                    displayHint: MD.ToolBarLayout.KeepVisible
                    busy: updateInstallQuery.pluginId === pluginItem.modelData.id && updateInstallQuery.querying ? (updateInstallQuery.progressing ? MD.Enum.Progress : MD.Enum.Busy) : MD.Enum.Idle
                    progress: updateInstallQuery.pluginId === pluginItem.modelData.id ? updateInstallQuery.progress : 0
                    onTriggered: {
                        if (updateInstallQuery.querying || deleteQuery.querying)
                            return;
                        root.installUpdate(pluginItem.modelData.id, pluginItem.modelData.updateInfo);
                    }
                }

                MD.Action {
                    id: pluginDeleteAction
                    text: qsTr("Delete")
                    icon.name: MD.Token.icon.delete
                    visible: pluginItem.modelData.system !== true
                    displayHint: pluginUpdateAction.visible ? MD.ToolBarLayout.AlwaysHide : MD.ToolBarLayout.KeepVisible
                    enabled: !deleteQuery.querying && !updateInstallQuery.querying
                    onTriggered: deleteQuery.remove(pluginItem.modelData.id)
                }
            }
            Flow {
                id: pluginFloatingTags
                anchors.top: parent.top
                anchors.topMargin: 8
                anchors.right: parent.right
                anchors.rightMargin: 16
                spacing: 6
                z: 2

                W.Tag {
                    id: pluginCompatTag
                    readonly property var compat: pluginItem.modelData.compat
                    readonly property string reason: root.foldMessage(pluginCompatTag.compat && pluginCompatTag.compat.reason ? String(pluginCompatTag.compat.reason) : "", 64)

                    // The daemon reports a plugin it cannot serve; say so on the
                    // plugin itself rather than leaving it looking installed and
                    // working. The reason is backend text, shown as sent.
                    visible: !!pluginCompatTag.compat && pluginCompatTag.compat.compatible === false
                    text: qsTr("Incompatible")
                    bgColor: MD.Token.color.error_container
                    fgColor: MD.Token.color.on_error_container

                    HoverHandler {
                        id: pluginCompatTagHover
                    }

                    MD.ToolTip.visible: pluginCompatTagHover.hovered && pluginCompatTag.reason.length > 0
                    MD.ToolTip.delay: 300
                    MD.ToolTip.text: pluginCompatTag.reason
                }
                W.Tag {
                    id: pluginUpdateTag
                    readonly property string reason: root.updateTagTooltip(pluginItem.modelData.updateInfo)

                    visible: root.updateTagVisible(pluginItem.modelData.updateInfo)
                    text: root.updateTagText(pluginItem.modelData.updateInfo)
                    bgColor: root.updateTagBgColor(pluginItem.modelData.updateInfo)
                    fgColor: root.updateTagFgColor(pluginItem.modelData.updateInfo)

                    HoverHandler {
                        id: pluginUpdateTagHover
                    }

                    MD.ToolTip.visible: pluginUpdateTagHover.hovered && pluginUpdateTag.reason.length > 0
                    MD.ToolTip.delay: 300
                    MD.ToolTip.text: pluginUpdateTag.reason
                }
                W.Tag {
                    text: "v" + (pluginItem.modelData.version || "0.0.0")
                }
            }
            below: Flow {
                spacing: 6
                bottomPadding: 8
                Repeater {
                    model: pluginItem.modelData.renderers
                    delegate: W.Tag {
                        required property var modelData
                        text: modelData.name || ""
                    }
                }
            }
        }
    }
}
