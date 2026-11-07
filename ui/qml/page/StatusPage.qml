pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    padding: 0
    showHeader: MD.MProp.size.isCompact
    showBackground: false
    title: qsTr('Status')

    actions: [
        MD.Action {
            icon.name: MD.Token.icon.extension
            text: qsTr("Plugins")
            property var presentation: null
            onTriggered: {
                if (!presentation?.active)
                    presentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
                        source: 'waywallen.ui/PluginManagePage'
                    });
            }
        },
        MD.Action {
            icon.name: MD.Token.icon.settings
            text: qsTr("Settings")
            property var presentation: null
            onTriggered: {
                if (!presentation?.active)
                    presentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
                        source: 'waywallen.ui/SettingsPage'
                    });
            }
        },
        MD.Action {
            icon.name: MD.Token.icon.info
            text: qsTr("About")
            property var presentation: null
            onTriggered: {
                if (!presentation?.active)
                    presentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
                        source: 'waywallen.ui/AboutPage'
                    });
            }
        }
    ]

    component SectionTitle: MD.Text {
        typescale: MD.Token.typescale.title_medium
        color: MD.Token.color.on_surface
    }

    component SectionHint: MD.Text {
        Layout.fillWidth: true
        typescale: MD.Token.typescale.body_medium
        color: MD.Token.color.on_surface_variant
        wrapMode: Text.WordWrap
    }

    component SectionPane: MD.Pane {
        Layout.fillWidth: true
        radius: 16
        padding: 16
        backgroundColor: MD.MProp.color.surface
    }

    W.HealthQuery {
        id: healthQuery
    }

    W.GlobalPauseSetQuery {
        id: globalPauseSetQuery
    }

    W.GlobalMuteSetQuery {
        id: globalMuteSetQuery
    }

    W.GlobalStopSetQuery {
        id: globalStopSetQuery
    }

    W.RendererListQuery {
        id: rendererQuery
    }

    W.RendererPluginListQuery {
        id: pluginQuery
    }

    W.SettingsGetQuery {
        id: settingsQuery
    }

    MD.Action {
        id: logAction
        icon.name: MD.Token.icon.receipt_long
        text: qsTr("Logs")
        displayHint: MD.ToolBarLayout.KeepVisible
        enabled: W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready
        property var presentation: null
        onTriggered: {
            if (!presentation?.active)
                presentation = root.Window.window.presentPopup('waywallen.ui/DaemonLogDialog');
        }
    }

    MD.Action {
        id: exitAction
        icon.name: MD.Token.icon.exit_to_app
        text: qsTr("Exit")
        displayHint: MD.ToolBarLayout.KeepVisible
        enabled: W.DaemonDBusClient.daemonAvailable
        onTriggered: {
            if (!W.DaemonDBusClient.quitDaemon())
                W.Global.toastError(qsTr("Failed to stop daemon"));
        }
    }

    // Queries fan out only after the daemon is Ready (avoid hitting
    // a half-booted daemon at UI startup). `daemonReady` is edge-
    // triggered, so pages constructed AFTER ready also need the level
    // check in `Component.onCompleted`.
    Connections {
        target: W.Notify
        function onDaemonReady() {
            root.reloadAll();
        }
        function onSettingsChanged() {
            settingsQuery.reload();
        }
        function onPluginChanged() {
            pluginQuery.reload();
        }
    }

    Component.onCompleted: {
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready)
            reloadAll();
    }

    function reloadAll() {
        healthQuery.reload();
        rendererQuery.reload();
        pluginQuery.reload();
        settingsQuery.reload();
    }

    function rendererLabel(d) {
        const name = (d && d.name && d.name.length) ? d.name : "renderer";
        const pid = (d && d.pid) ? d.pid : 0;
        const identity = pid > 0 ? String(pid) : String(d && d.id || "").slice(0, 8);
        return identity.length > 0 ? name + "-" + identity : name;
    }

    function desktopLabel(value) {
        const raw = (value || "").trim();
        if (!raw.length)
            return "";
        const key = raw.toLowerCase();
        if (key === "cosmic")
            return "COSMIC";
        if (key === "gnome")
            return "GNOME";
        if (key === "kde")
            return "KDE";
        if (key === "hyprland")
            return "Hyprland";
        if (key === "niri")
            return "Niri";
        if (key === "river")
            return "River";
        if (key === "sway")
            return "Sway";
        return raw.charAt(0).toUpperCase() + raw.slice(1);
    }

    function healthLabel(state) {
        if (!state)
            return qsTr("unknown");
        if (state === "healthy")
            return qsTr("healthy");
        return state;
    }

    function rendererStatusLabel(status) {
        if (status === "starting")
            return qsTr("starting");
        if (status === "playing")
            return qsTr("playing");
        if (status === "paused")
            return qsTr("paused");
        if (status === "muted")
            return qsTr("muted");
        if (status === "stopping")
            return qsTr("stopping");
        if (status === "stopped")
            return qsTr("stopped");
        if (status === "killed")
            return qsTr("killed");
        if (status === "failed")
            return qsTr("failed");
        return status || "";
    }

    W.RendererKillQuery {
        id: killQuery
        onStatusChanged: {
            if (status === 3) {
                rendererQuery.reload();
                healthQuery.reload();
            }
        }
    }

    MD.Dialog {
        id: killDialog
        property string rendererId: ""
        property string label: ""
        title: qsTr("Kill renderer?")
        parent: MD.Overlay.overlay
        standardButtons: MD.Dialog.Cancel | MD.Dialog.Ok

        contentItem: MD.Text {
            text: qsTr("Stop the renderer process\n\"%1\"?\nUnsaved frame state may be lost.").arg(killDialog.label)
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.on_surface_variant
            wrapMode: Text.WordWrap
        }

        onAccepted: {
            killQuery.rendererId = killDialog.rendererId;
            killQuery.reload();
        }
    }

    contentItem: MD.VerticalFlickable {
        id: m_flick
        topMargin: 12
        leftMargin: 12
        rightMargin: 12
        bottomMargin: 12

        ColumnLayout {
            width: m_flick.contentWidth
            spacing: 12

            // --- Daemon ---
            SectionPane {
                contentItem: ColumnLayout {
                    spacing: 8

                    Item {
                        Layout.fillWidth: true
                        implicitHeight: daemonTitle.implicitHeight

                        SectionTitle {
                            id: daemonTitle

                            anchors.left: parent.left
                            anchors.right: daemonActions.left
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Daemon")
                            elide: Text.ElideRight
                            maximumLineCount: 1
                        }

                        MD.ActionToolBar {
                            id: daemonActions

                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            actions: [logAction, exitAction]
                        }
                    }

                    RowLayout {
                        spacing: 8
                        MD.Text {
                            text: qsTr("Service:")
                            typescale: MD.Token.typescale.label_medium
                            color: MD.Token.color.on_surface_variant
                        }
                        MD.Text {
                            text: healthQuery.service || "—"
                            typescale: MD.Token.typescale.body_medium
                            color: MD.Token.color.on_surface
                        }
                        W.Tag {
                            Layout.alignment: Qt.AlignVCenter
                            visible: healthQuery.osName.length > 0
                            text: healthQuery.osName
                        }
                        W.Tag {
                            readonly property string label: root.desktopLabel(W.Notify.displayBackend.desktop)
                            Layout.alignment: Qt.AlignVCenter
                            visible: label.length > 0
                            text: label
                            bgColor: MD.Token.color.secondary_container
                            fgColor: MD.Token.color.on_secondary_container
                        }
                        W.Tag {
                            Layout.alignment: Qt.AlignVCenter
                            visible: W.Notify.displayBackend.flatpakId.length > 0
                            text: "Flatpak"
                            bgColor: MD.Token.color.tertiary_container
                            fgColor: MD.Token.color.on_tertiary_container
                        }
                    }

                    RowLayout {
                        spacing: 8
                        MD.Text {
                            text: qsTr("State:")
                            typescale: MD.Token.typescale.label_medium
                            color: MD.Token.color.on_surface_variant
                        }

                        Rectangle {
                            Layout.preferredWidth: 8
                            Layout.preferredHeight: 8
                            radius: 4
                            color: healthQuery.state === "healthy" ? MD.Token.color.primary : MD.Token.color.error
                        }

                        MD.Text {
                            text: root.healthLabel(healthQuery.state)
                            typescale: MD.Token.typescale.body_medium
                            color: MD.Token.color.on_surface
                        }
                    }
                }
            }

            // --- Renderers ---
            SectionPane {
                contentItem: ColumnLayout {
                    spacing: 8

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        SectionTitle {
                            text: qsTr("Renderers")
                        }

                        Item {
                            Layout.fillWidth: true
                        }

                        MD.FilterChip {
                            text: qsTr("Mute all")
                            selected: W.Notify.globalMuted
                            enabled: W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready && !globalMuteSetQuery.querying
                            onSelectionRequested: {
                                globalMuteSetQuery.muted = !W.Notify.globalMuted;
                                globalMuteSetQuery.reload();
                            }
                        }

                        MD.FilterChip {
                            text: qsTr("Pause all")
                            selected: W.Notify.globalPaused
                            enabled: W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready && !globalPauseSetQuery.querying
                            onSelectionRequested: {
                                globalPauseSetQuery.paused = !W.Notify.globalPaused;
                                globalPauseSetQuery.reload();
                            }
                        }

                        MD.FilterChip {
                            text: qsTr("Stop all")
                            selected: W.Notify.globalStopped
                            enabled: W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready && !globalStopSetQuery.querying
                            onSelectionRequested: {
                                globalStopSetQuery.stopped = !W.Notify.globalStopped;
                                globalStopSetQuery.reload();
                            }
                        }
                    }

                    SectionHint {
                        readonly property var liveRenderers: W.App.rendererManager.renderers
                        visible: !liveRenderers || liveRenderers.length === 0
                        text: qsTr("No renderers")
                    }

                    MD.VerticalLazyList {
                        Layout.fillWidth: true
                        expand: true
                        interactive: false
                        spacing: 4

                        // Push-updated logical renderer slots remain visible while
                        // their process is retained in a stopped state.
                        source: MD.ListSnapshotSource {
                            items: W.App.rendererManager.renderers
                        }

                        delegate: MD.ListItem {
                            id: rendererItem
                            required property MD.LazyRow row
                            readonly property var modelData: row.value
                            itemRow: row

                            radius: 12
                            text: root.rendererLabel(modelData)
                            font.family: "monospace"
                            leader: MD.Icon {
                                name: modelData.running ? (modelData.status === "paused" ? MD.Token.icon.pause : MD.Token.icon.play_arrow) : MD.Token.icon.stop
                                size: 24
                                color: modelData.running && modelData.status !== "paused" ? MD.Token.color.primary : MD.Token.color.on_surface_variant
                            }
                            trailing: RowLayout {
                                spacing: 6
                                Repeater {
                                    model: modelData.runtimeConditions || []
                                    delegate: W.RuntimeConditionTag {
                                        required property var modelData
                                        Layout.alignment: Qt.AlignVCenter
                                        condition: modelData
                                    }
                                }
                                MD.IconButton {
                                    icon.name: MD.Token.icon.close
                                    onClicked: {
                                        killDialog.rendererId = modelData.id;
                                        killDialog.label = root.rendererLabel(modelData);
                                        killDialog.open();
                                    }
                                }
                            }
                            supporting: Flow {
                                spacing: 6
                                topPadding: 4

                                W.Tag {
                                    visible: !!rendererItem.modelData.status
                                    text: root.rendererStatusLabel(rendererItem.modelData.status)
                                }
                                W.Tag {
                                    visible: rendererItem.modelData.keep
                                    text: qsTr("keep")
                                }
                                W.Tag {
                                    visible: rendererItem.modelData.running
                                    text: (rendererItem.modelData.fps || 0) + " fps"
                                }
                                W.Tag {
                                    visible: rendererItem.modelData.textureWidth > 0 && rendererItem.modelData.textureHeight > 0
                                    text: rendererItem.modelData.textureWidth + "×" + rendererItem.modelData.textureHeight
                                }
                                Repeater {
                                    model: rendererItem.modelData.runtimeTags || []
                                    delegate: W.RendererRuntimeTag {
                                        required property var modelData
                                        runtimeTag: modelData
                                    }
                                }
                                W.GpuTag {
                                    drmRenderMajor: rendererItem.modelData.drmRenderMajor || 0
                                    drmRenderMinor: rendererItem.modelData.drmRenderMinor || 0
                                }
                            }
                        }
                    }
                }
            }

            // --- Components ---
            SectionPane {
                contentItem: ColumnLayout {
                    spacing: 8

                    SectionTitle {
                        text: qsTr("Components")
                    }

                    SectionHint {
                        typescale: MD.Token.typescale.label_medium
                        visible: pluginQuery.supportedTypes && pluginQuery.supportedTypes.length > 0
                        text: qsTr("Supported types: %1").arg((pluginQuery.supportedTypes ?? []).map(t => W.I18n.valueLabel(pluginQuery.typeLabels, t)).join(", "))
                    }

                    SectionHint {
                        visible: !pluginQuery.renderers || pluginQuery.renderers.length === 0
                        text: qsTr("No components")
                    }

                    MD.VerticalLazyList {
                        Layout.fillWidth: true
                        expand: true
                        interactive: false
                        spacing: 4

                        source: MD.ListSnapshotSource {
                            items: pluginQuery.renderers
                        }

                        delegate: MD.ListItem {
                            id: componentItem
                            required property MD.LazyRow row
                            readonly property var modelData: row.value
                            itemRow: row

                            readonly property bool hasSettings: (modelData.settings && modelData.settings.length > 0) === true
                            property var settingsPresentation: null

                            Connections {
                                target: componentItem.row
                                function onReused() {
                                    componentItem.settingsPresentation = null;
                                }
                            }

                            radius: 12
                            text: modelData.name || ""
                            supportText: (modelData.types ? modelData.types.map(t => W.I18n.valueLabel(pluginQuery.typeLabels, t)).join(", ") : "")
                            leader: MD.Icon {
                                name: MD.Token.icon.widgets
                                size: 24
                                color: MD.Token.color.on_surface_variant
                            }
                            trailing: RowLayout {
                                spacing: 4
                                W.Tag {
                                    Layout.alignment: Qt.AlignVCenter
                                    text: "v" + (componentItem.modelData.version || "0.0.0")
                                }
                                MD.IconButton {
                                    visible: componentItem.hasSettings
                                    icon.name: MD.Token.icon.settings
                                    onClicked: {
                                        if (componentItem.settingsPresentation?.active)
                                            return;
                                        const name = componentItem.modelData.name;
                                        const p = settingsQuery.plugins ? settingsQuery.plugins[name] : undefined;
                                        componentItem.settingsPresentation = root.Window.window.presentPopup('waywallen.ui/PagePopup', {
                                            source: 'waywallen.ui/PluginSettingsPage',
                                            props: {
                                                pluginName: name,
                                                schemaList: componentItem.modelData.settings || [],
                                                allCurrentPlugins: settingsQuery.plugins || ({}),
                                                currentGlobal: settingsQuery.global || ({}),
                                                currentValues: p || ({})
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
