pragma ComponentBehavior: Bound
import QtQuick
import QtQml as Qml
import QtQuick.Templates as T
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.control as WC
import waywallen.ui as W

MD.Page {
    id: root
    padding: 0
    showHeader: true
    showBackground: false
    title: qsTr('Settings')
    scrolling: !m_flick.atYBeginning

    actions: [
        MD.Action {
            icon.name: MD.Token.icon.settings_backup_restore
            text: qsTr("Reset")
            enabled: Object.keys(getQ.global).length > 0
            onTriggered: resetSettingsDialog.open()
        }
    ]

    MD.Dialog {
        id: resetSettingsDialog
        parent: T.Overlay.overlay
        modal: true
        title: qsTr("Reset settings?")
        standardButtons: T.Dialog.Cancel | T.Dialog.Reset
        contentItem: MD.Text {
            text: qsTr("All settings will be reset to their defaults. Your custom configuration will be lost.")
            wrapMode: Text.Wrap
            color: MD.Token.color.on_surface_variant
        }
        onReset: {
            root.resetSettings()
            resetSettingsDialog.close()
        }
    }

    component FieldLabel: MD.Text {
        typescale: MD.Token.typescale.label_large
        color: MD.Token.color.on_surface
    }

    component SettingHeader: MD.Text {
        Layout.fillWidth: true
        typescale: MD.Token.typescale.title_small
        color: MD.Token.color.on_surface_variant
        topPadding: 16
        bottomPadding: 6
        leftPadding: 4
    }

    component SettingItem: Rectangle {
        id: settingItem
        default property alias content: settingContent.data
        property bool first: true
        property bool last: true

        Layout.fillWidth: true
        implicitHeight: settingContent.implicitHeight + 16
        color: MD.Token.color.surface_container

        readonly property real radiusBig: 16

        topLeftRadius: first ? radiusBig : 0
        topRightRadius: first ? radiusBig : 0
        bottomLeftRadius: last ? radiusBig : 0
        bottomRightRadius: last ? radiusBig : 0

        ColumnLayout {
            id: settingContent
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: 16
            anchors.rightMargin: 16
        }
    }

    function formatBytes(bytes) {
        let value = Math.max(0, Number(bytes ?? 0));
        const units = ["B", "KiB", "MiB", "GiB", "TiB"];
        let index = 0;
        while (value >= 1024 && index < units.length - 1) {
            value /= 1024;
            ++index;
        }
        return value.toFixed(index === 0 ? 0 : 1) + " " + units[index];
    }

    W.SettingsGetQuery {
        id: getQ
        onGlobalChanged: root._maybeClearSubmittedGlobal()
    }

    W.SettingsSetQuery {
        id: setQ
        onStatusChanged: {
            if (status === 3)
                m_pending.submittedGlobal = null;
        }
    }

    W.AutostartGetQuery {
        id: autostartGetQ
    }

    W.AutostartSetQuery {
        id: autostartSetQ
        forwardError: false
        onStatusChanged: {
            if (status === 2) {
                autostartGetQ.reload();
            } else if (status === 3) {
                const message = error && error.length > 0
                    ? error
                    : qsTr("Failed to update login startup");
                W.Action.toast(message, 6000, 1, null);
            }
        }
    }

    Connections {
        target: W.Notify
        function onDaemonReady() {
            getQ.reload();
            if (W.Util.isFlatpak)
                autostartGetQ.reload();
        }
        function onSettingsChanged() {
            getQ.reload();
        }
    }

    Component.onCompleted: {
        W.App.refreshNetworkCacheSize();
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready) {
            getQ.reload();
            if (W.Util.isFlatpak)
                autostartGetQ.reload();
        }
    }

    onVisibleChanged: if (visible) W.App.refreshNetworkCacheSize()

    // Same pattern as WallpaperPage._persistGlobalChange but routed
    // through a 200ms debounce — slider drags would otherwise flood
    // the daemon with one RPC per pixel.
    QtObject {
        id: m_pending
        property var nextGlobal: null
        property var submittedGlobal: null
    }

    Qml.Timer {
        id: m_flush
        interval: 200
        repeat: false
        onTriggered: {
            const g = m_pending.nextGlobal;
            if (!g) return;
            setQ.global = g;
            setQ.plugins = getQ.plugins;
            setQ.reload();
            m_pending.submittedGlobal = g;
            m_pending.nextGlobal = null;
        }
    }

    function _mut(fn) {
        if (Object.keys(getQ.global).length === 0)
            return;
        const base = m_pending.nextGlobal
                   ? m_pending.nextGlobal
                   : (m_pending.submittedGlobal
                      ? m_pending.submittedGlobal
                      : Object.assign({}, getQ.global));
        fn(base);
        m_pending.nextGlobal = base;
        m_flush.restart();
    }

    function logFolderUrl() {
        let path = String(getQ.logDir || "");
        if (path.length === 0)
            return "";
        if (path.indexOf("file://") === 0)
            return path;
        return "file://" + path.split("/").map(encodeURIComponent).join("/");
    }

    function openLogFolder() {
        const url = root.logFolderUrl();
        if (url.length > 0)
            MD.Util.openFolderExternally(url);
    }

    property int autoReplayRevision: 0
    property int pauseEffectRevision: 0
    property int transitionRevision: 0

    readonly property var kAutoReplayRows: [
        { key: "anyWindow",       label: qsTr("Any window") },
        { key: "focused",         label: qsTr("Focused window") },
        { key: "maximized",       label: qsTr("Maximized window") },
        { key: "fullscreen",      label: qsTr("Fullscreen window") },
        { key: "sessionLocked",   label: qsTr("Session locked") },
        { key: "sessionInactive", label: qsTr("Session inactive") }
    ]

    readonly property var kAutoActions: [
        { value: WC.AutoAction.AUTO_ACTION_NONE,        label: qsTr("None") },
        { value: WC.AutoAction.AUTO_ACTION_MUTE,        label: qsTr("Mute") },
        { value: WC.AutoAction.AUTO_ACTION_PAUSE,       label: qsTr("Pause") },
        { value: WC.AutoAction.AUTO_ACTION_STOP,        label: qsTr("Stop") }
    ]

    function _listIndex(list, value) {
        for (let i = 0; i < list.length; ++i)
            if (list[i].value === value) return i;
        return 0;
    }

    function _uiLanguageIndex() {
        const languages = W.App.availableUiLanguages;
        for (let i = 0; i < languages.length; ++i)
            if (languages[i].code === W.App.uiLanguage) return i;
        return 0;
    }

    function _currentGlobal() {
        return m_pending.nextGlobal
            ? m_pending.nextGlobal
            : (m_pending.submittedGlobal
               ? m_pending.submittedGlobal
               : getQ.global);
    }

    function _defaultAutoReplay() {
        return {
            anyWindow: WC.AutoAction.AUTO_ACTION_NONE,
            focused: WC.AutoAction.AUTO_ACTION_NONE,
            maximized: WC.AutoAction.AUTO_ACTION_NONE,
            fullscreen: WC.AutoAction.AUTO_ACTION_PAUSE,
            sessionLocked: WC.AutoAction.AUTO_ACTION_STOP,
            sessionInactive: WC.AutoAction.AUTO_ACTION_STOP,
            resumeDelayMs: 250
        };
    }

    function _defaultGlobalPageSettings() {
        return {
            autoReplay: root._defaultAutoReplay(),
            pauseEffect: root._defaultPauseEffect(),
            transition: root._defaultTransition(),
            queueMode: "sequential",
            rotationSecs: 0,
            audioFadeMs: 500,
            muteWhenOtherAudio: false,
            audioCaptureEnabled: true,
            pointerForwardingEnabled: true,
            "renderer.enable_audio": true,
            "renderer.volume": 100,
            pluginUpdateNotifications: true,
            duplicateRenderers: false,
            hideTrayIcon: false,
            debugLoggingEnabled: false
        };
    }

    function resetSettings() {
        if (Object.keys(getQ.global).length === 0)
            return;

        const nextGlobal = Object.assign({}, getQ.global, root._defaultGlobalPageSettings());
        m_flush.stop();
        m_pending.nextGlobal = null;
        m_pending.submittedGlobal = nextGlobal;
        W.Global.sidebarAutoExpand = true;
        W.Global.singleUiEnabled = false;
        W.App.setSingleUiEnabled(false);
        W.DaemonDBusClient.quitOnDaemonShutdown = true;
        W.Global.networkCacheMaximumMiB = 1024;
        W.Global.setThemeMode("system");
        W.Global.accentColor = W.Global.defaultAccentColor;
        W.Global.setAccentMode("system");
        if (!W.App.setUiLanguage("system"))
            W.Global.toastError(qsTr("Failed to change language"));
        root.autoReplayRevision += 1;
        root.pauseEffectRevision += 1;
        root.transitionRevision += 1;
        setQ.global = nextGlobal;
        setQ.plugins = getQ.plugins;
        setQ.reload();
    }

    function _globalPageKey(g) {
        if (!g)
            return "";
        return JSON.stringify({
            autoReplay: root._normalizedAutoReplay(g.autoReplay || ({})),
            pauseEffect: root._normalizedPauseEffect(g.pauseEffect || ({})),
            transition: root._normalizedTransition(g.transition || ({})),
            queueMode: g.queueMode ?? "sequential",
            rotationSecs: Number(g.rotationSecs ?? 0),
            audioFadeMs: Number(g.audioFadeMs ?? 500),
            muteWhenOtherAudio: Boolean(g.muteWhenOtherAudio ?? false),
            audioCaptureEnabled: Boolean(g.audioCaptureEnabled),
            pointerForwardingEnabled: Boolean(g.pointerForwardingEnabled ?? true),
            rendererEnableAudio: root._rendererAudioEnabled(g),
            rendererVolume: root._rendererVolume(g),
            pluginUpdateNotifications: Boolean(g.pluginUpdateNotifications ?? true),
            duplicateRenderers: Boolean(g.duplicateRenderers ?? false),
            hideTrayIcon: Boolean(g.hideTrayIcon ?? false),
            debugLoggingEnabled: Boolean(g.debugLoggingEnabled ?? false)
        });
    }

    function _normalizedAutoReplay(policy) {
        return Object.assign(root._defaultAutoReplay(), policy || ({}));
    }

    function _defaultPauseEffect() {
        return {
            kind: WC.PauseEffectKind.PAUSE_EFFECT_KIND_NONE,
            blur: { radius: 30 }
        };
    }

    function _normalizedPauseEffect(config) {
        const normalized = Object.assign(root._defaultPauseEffect(), config || ({}));
        normalized.blur = Object.assign({ radius: 30 }, config?.blur || ({}));
        return normalized;
    }

    function _pauseEffect() {
        root.pauseEffectRevision;
        const g = root._currentGlobal();
        return root._normalizedPauseEffect(g?.pauseEffect || ({}));
    }

    function _mutPauseEffect(fn) {
        root._mut(g => {
            const config = root._normalizedPauseEffect(g.pauseEffect || ({}));
            fn(config);
            g.pauseEffect = config;
        });
        root.pauseEffectRevision += 1;
    }

    readonly property var kTransitionKinds: [
        { value: WC.TransitionKind.TRANSITION_KIND_NONE, label: qsTr("None") },
        { value: WC.TransitionKind.TRANSITION_KIND_FADE, label: qsTr("Fade") },
        { value: WC.TransitionKind.TRANSITION_KIND_WIPE, label: qsTr("Wipe") },
        { value: WC.TransitionKind.TRANSITION_KIND_GROW, label: qsTr("Grow") }
    ]

    // Angles are clockwise with 0 wiping left to right.
    readonly property var kWipeDirections: [
        { value: 0,   label: qsTr("Left to right") },
        { value: 45,  label: qsTr("Top left to bottom right") },
        { value: 90,  label: qsTr("Top to bottom") },
        { value: 135, label: qsTr("Top right to bottom left") },
        { value: 180, label: qsTr("Right to left") },
        { value: 225, label: qsTr("Bottom right to top left") },
        { value: 270, label: qsTr("Bottom to top") },
        { value: 315, label: qsTr("Bottom left to top right") }
    ]

    function _wipeDirectionIndex(angle) {
        const steps = Math.round((Number(angle) || 0) / 45);
        return ((steps % 8) + 8) % 8;
    }

    function _defaultTransition() {
        return {
            kind: WC.TransitionKind.TRANSITION_KIND_NONE,
            durationMs: 500,
            angle: 0,
            originX: 50,
            originY: 50
        };
    }

    function _normalizedTransition(config) {
        return Object.assign(root._defaultTransition(), config || ({}));
    }

    function _transition() {
        root.transitionRevision;
        const g = root._currentGlobal();
        return root._normalizedTransition(g?.transition || ({}));
    }

    function _mutTransition(fn) {
        root._mut(g => {
            const config = root._normalizedTransition(g.transition || ({}));
            fn(config);
            g.transition = config;
        });
        root.transitionRevision += 1;
    }

    function _rendererAudioEnabled(globalSettings) {
        const value = globalSettings?.["renderer.enable_audio"];
        return value === undefined ? true : Boolean(value);
    }

    function _rendererVolume(globalSettings) {
        const value = Number(globalSettings?.["renderer.volume"] ?? 100);
        return Math.max(0, Math.min(100, value));
    }

    function _maybeClearSubmittedGlobal() {
        if (!m_pending.submittedGlobal)
            return;
        if (root._globalPageKey(getQ.global) === root._globalPageKey(m_pending.submittedGlobal))
            m_pending.submittedGlobal = null;
    }

    function _autoReplay() {
        root.autoReplayRevision;
        const g = root._currentGlobal();
        return root._normalizedAutoReplay(g?.autoReplay || ({}));
    }

    function _mutAutoReplay(fn) {
        root._mut(g => {
            const policy = Object.assign(root._defaultAutoReplay(), g.autoReplay || ({}));
            fn(policy);
            g.autoReplay = policy;
        });
        root.autoReplayRevision += 1;
    }

    function _updateAutoReplayAction(key, action) {
        root._mutAutoReplay(policy => {
            policy[key] = action;
        });
    }

    readonly property var kQueueModes: [
        { value: "sequential", label: qsTr("Sequential") },
        { value: "shuffle",    label: qsTr("Shuffle") },
        { value: "random",     label: qsTr("Random") }
    ]

    function _queueIndex(v) {
        for (let i = 0; i < kQueueModes.length; ++i)
            if (kQueueModes[i].value === v) return i;
        return 0;
    }

    contentItem: MD.VerticalFlickable {
        id: m_flick
        leftMargin: 16
        rightMargin: 16
        bottomMargin: 12

        ColumnLayout {
            width: m_flick.contentWidth
            spacing: 2

            SettingHeader { text: qsTr("Appearance") }

            SettingItem {
                first: true
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Theme")
                    }

                    MD.SegmentedButtonGroup {
                        size: MD.Enum.XS

                        MD.SegmentedButton {
                            text: qsTr("Light")
                            checked: W.Global.themeMode === "light"
                            onClicked: W.Global.setThemeMode("light")
                        }

                        MD.SegmentedButton {
                            text: qsTr("Dark")
                            checked: W.Global.themeMode === "dark"
                            onClicked: W.Global.setThemeMode("dark")
                        }

                        MD.SegmentedButton {
                            text: qsTr("System")
                            checked: W.Global.themeMode === "system"
                            onClicked: W.Global.setThemeMode("system")
                        }
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Accent color")
                    }

                    MD.SegmentedButtonGroup {
                        size: MD.Enum.XS

                        MD.SegmentedButton {
                            text: qsTr("Custom")
                            checked: W.Global.accentMode === "custom"
                            onClicked: W.Global.setAccentMode("custom")
                        }

                        MD.SegmentedButton {
                            text: qsTr("System")
                            checked: W.Global.accentMode === "system"
                            onClicked: W.Global.setAccentMode("system")
                        }
                    }
                }

                MD.ColorPickerButton {
                    id: m_accent_color
                    visible: W.Global.accentMode === "custom"
                    Layout.alignment: Qt.AlignRight
                    Layout.topMargin: 4
                    Layout.preferredWidth: 96
                    Layout.preferredHeight: 32
                    showAlpha: false
                    onAccepted: color => W.Global.accentColor = color
                }

                Binding {
                    target: m_accent_color
                    property: "color"
                    value: W.Global.accentColor
                }
            }

            SettingItem {
                first: false
                last: true

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Language")
                    }

                    MD.ComboBox {
                        id: m_ui_language
                        Layout.preferredWidth: 180
                        mdState.size: MD.Enum.S
                        model: W.App.availableUiLanguages.map(language => language.label)
                        onActivated: index => {
                            const languages = W.App.availableUiLanguages;
                            if (index < 0 || index >= languages.length)
                                return;
                            if (!W.App.setUiLanguage(languages[index].code))
                                W.Global.toastError(qsTr("Failed to change language"));
                        }
                    }
                    Binding {
                        target: m_ui_language
                        property: "currentIndex"
                        value: root._uiLanguageIndex()
                    }
                }
            }

            SettingHeader { text: qsTr("Auto replay") }

            Repeater {
                model: root.kAutoReplayRows
                delegate: SettingItem {
                    id: autoReplayItem
                    required property int index
                    required property var modelData

                    first: autoReplayItem.index === 0
                    last: false

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        FieldLabel {
                            Layout.fillWidth: true
                            text: autoReplayItem.modelData.label
                        }

                        MD.ComboBox {
                            id: autoReplayActionBox
                            Layout.preferredWidth: 180
                            mdState.size: MD.Enum.S
                            model: root.kAutoActions.map(o => o.label)
                            onActivated: idx => root._updateAutoReplayAction(
                                autoReplayItem.modelData.key,
                                root.kAutoActions[idx].value)
                        }
                        Binding {
                            target: autoReplayActionBox
                            property: "currentIndex"
                            value: root._listIndex(
                                root.kAutoActions,
                                root._autoReplay()[autoReplayItem.modelData.key] ?? 0)
                        }
                    }
                }
            }

            SettingItem {
                first: false
                last: true

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Resume delay")
                    }

                    W.ValueSlider {
                        id: m_auto_replay_resume_delay
                        Layout.preferredWidth: 220
                        from: 0
                        to: 2000
                        stepSize: 50
                        snapMode: T.Slider.SnapAlways
                        maxVisibleStops: 10
                        valueText: Math.round(value).toString()
                        valueMaxText: "2000"
                        onMoved: root._mutAutoReplay(policy => {
                            policy.resumeDelayMs = Math.round(value);
                        })
                    }
                    Binding {
                        target: m_auto_replay_resume_delay
                        property: "value"
                        value: Number(root._autoReplay().resumeDelayMs ?? 250)
                    }

                    MD.Text {
                        text: qsTr("ms")
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }

            SettingHeader { text: qsTr("Behavior") }

            SettingItem {
                first: true
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        FieldLabel { text: qsTr("Auto-expand sidebar") }

                        MD.Text {
                            text: qsTr("Expand or collapse the sidebar with the window size.")
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }

                    MD.Switch {
                        id: m_sidebar_auto_expand
                        checked: W.Global.sidebarAutoExpand
                        onToggled: W.Global.sidebarAutoExpand = checked
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        FieldLabel { text: qsTr("Single UI instance") }

                        MD.Text {
                            text: qsTr("When enabled, launching Waywallen again focuses the existing window.")
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }

                    MD.Switch {
                        id: m_single_ui
                        checked: W.Global.singleUiEnabled
                        onToggled: {
                            W.Global.singleUiEnabled = checked;
                            W.App.setSingleUiEnabled(checked);
                        }
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        FieldLabel { text: qsTr("Quit UI with daemon") }

                        MD.Text {
                            text: qsTr("Close the UI when the daemon shuts down. Disable to keep it open for reconnection.")
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }

                    MD.Switch {
                        checked: W.DaemonDBusClient.quitOnDaemonShutdown
                        onToggled: W.DaemonDBusClient.quitOnDaemonShutdown = checked
                    }
                }
            }

            SettingItem {
                first: false
                last: false
                visible: W.Util.isFlatpak

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Start at login")
                    }

                    MD.Switch {
                        id: m_autostart
                        enabled: !autostartGetQ.querying && !autostartSetQ.querying
                        onClicked: {
                            autostartSetQ.enabled = checked;
                            autostartSetQ.reload();
                        }
                    }
                    Binding {
                        target: m_autostart
                        property: "checked"
                        value: autostartSetQ.querying || autostartSetQ.status === 2
                            ? autostartSetQ.enabled
                            : autostartGetQ.enabled
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        FieldLabel { text: qsTr("Hide tray icon") }

                        MD.Text {
                            text: qsTr("Remove the status-bar icon. Reopen this window by launching Waywallen again.")
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }

                    MD.Switch {
                        id: m_hide_tray_icon
                        onToggled: root._mut(g => {
                            g.hideTrayIcon = checked;
                        })
                    }
                    Binding {
                        target: m_hide_tray_icon
                        property: "checked"
                        value: Boolean(root._currentGlobal()?.hideTrayIcon ?? false)
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2

                        FieldLabel { text: qsTr("Debug logging") }

                        MD.Text {
                            text: getQ.wwLogActive
                                ? qsTr("Disabled because WW_LOG is set.")
                                : qsTr("Include debug-level messages in the log file. Takes effect immediately.")
                            typescale: MD.Token.typescale.body_small
                            color: MD.Token.color.on_surface_variant
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }

                    MD.Switch {
                        id: m_debug_logging
                        enabled: !getQ.wwLogActive
                        onToggled: root._mut(g => {
                            g.debugLoggingEnabled = checked;
                        })
                    }
                    Binding {
                        target: m_debug_logging
                        property: "checked"
                        value: Boolean(root._currentGlobal()?.debugLoggingEnabled ?? false)
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Log folder")
                    }

                    MD.Button {
                        text: qsTr("Open")
                        mdState.type: MD.Enum.BtText
                        enabled: String(getQ.logDir || "").length > 0
                        onClicked: root.openLogFolder()
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Allow duplicate renderers")
                    }

                    MD.Switch {
                        id: m_duplicate_renderers
                        onToggled: root._mut(g => {
                            g.duplicateRenderers = checked;
                        })
                    }
                    Binding {
                        target: m_duplicate_renderers
                        property: "checked"
                        value: Boolean(root._currentGlobal()?.duplicateRenderers ?? false)
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Mouse forwarding")
                    }

                    MD.Switch {
                        id: m_pointer_forwarding
                        onToggled: root._mut(g => {
                            g.pointerForwardingEnabled = checked;
                        })
                    }
                    Binding {
                        target: m_pointer_forwarding
                        property: "checked"
                        value: Boolean(root._currentGlobal()?.pointerForwardingEnabled ?? true)
                    }
                }
            }

            SettingItem {
                first: false
                last: true

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Plugin update notifications")
                    }

                    MD.Switch {
                        id: m_plugin_update_notifications
                        onToggled: root._mut(g => {
                            g.pluginUpdateNotifications = checked;
                        })
                    }
                    Binding {
                        target: m_plugin_update_notifications
                        property: "checked"
                        value: Boolean(root._currentGlobal()?.pluginUpdateNotifications ?? true)
                    }
                }
            }

            SettingHeader { text: qsTr("Effect") }

            SettingItem {
                first: true
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Blur while paused")
                    }

                    MD.Switch {
                        id: m_pause_effect_enabled
                        onToggled: root._mutPauseEffect(config => {
                            config.kind = checked
                                ? WC.PauseEffectKind.PAUSE_EFFECT_KIND_BLUR
                                : WC.PauseEffectKind.PAUSE_EFFECT_KIND_NONE;
                        })
                    }
                    Binding {
                        target: m_pause_effect_enabled
                        property: "checked"
                        value: root._pauseEffect().kind
                            === WC.PauseEffectKind.PAUSE_EFFECT_KIND_BLUR
                    }
                }
            }

            SettingItem {
                first: false
                last: true
                enabled: root._pauseEffect().kind
                    === WC.PauseEffectKind.PAUSE_EFFECT_KIND_BLUR

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Blur radius")
                    }

                    W.ValueSlider {
                        id: m_pause_effect_blur_radius
                        Layout.preferredWidth: 220
                        from: 1
                        to: 64
                        stepSize: 1
                        snapMode: T.Slider.SnapAlways
                        maxVisibleStops: 8
                        valueText: Math.round(value).toString()
                        valueMaxText: "64"
                        onMoved: root._mutPauseEffect(config => {
                            config.blur.radius = Math.round(value);
                        })
                    }
                    Binding {
                        target: m_pause_effect_blur_radius
                        property: "value"
                        value: root._pauseEffect().blur.radius
                    }

                    MD.Text {
                        text: qsTr("px")
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }

            SettingHeader { text: qsTr("Transition") }

            SettingItem {
                first: true
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Wallpaper transition")
                    }

                    MD.ComboBox {
                        id: m_transition_kind
                        Layout.preferredWidth: 180
                        mdState.size: MD.Enum.S
                        model: root.kTransitionKinds.map(o => o.label)
                        onActivated: idx => root._mutTransition(config => {
                            config.kind = root.kTransitionKinds[idx].value;
                        })
                    }
                    Binding {
                        target: m_transition_kind
                        property: "currentIndex"
                        value: root._listIndex(root.kTransitionKinds, root._transition().kind)
                    }
                }
            }

            SettingItem {
                readonly property int kind: root._transition().kind
                first: false
                last: kind !== WC.TransitionKind.TRANSITION_KIND_WIPE
                    && kind !== WC.TransitionKind.TRANSITION_KIND_GROW
                enabled: kind !== WC.TransitionKind.TRANSITION_KIND_NONE

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Transition duration")
                    }

                    W.ValueSlider {
                        id: m_transition_duration
                        Layout.preferredWidth: 220
                        from: 100
                        to: 3000
                        stepSize: 100
                        snapMode: T.Slider.SnapAlways
                        maxVisibleStops: 10
                        valueText: Math.round(value).toString()
                        valueMaxText: "3000"
                        onMoved: root._mutTransition(config => {
                            config.durationMs = Math.round(value);
                        })
                    }
                    Binding {
                        target: m_transition_duration
                        property: "value"
                        value: root._transition().durationMs
                    }

                    MD.Text {
                        text: qsTr("ms")
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }

            SettingItem {
                first: false
                last: true
                visible: root._transition().kind === WC.TransitionKind.TRANSITION_KIND_WIPE

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Wipe direction")
                    }

                    MD.ComboBox {
                        id: m_transition_wipe_direction
                        Layout.preferredWidth: 220
                        mdState.size: MD.Enum.S
                        model: root.kWipeDirections.map(o => o.label)
                        onActivated: idx => root._mutTransition(config => {
                            config.angle = root.kWipeDirections[idx].value;
                        })
                    }
                    Binding {
                        target: m_transition_wipe_direction
                        property: "currentIndex"
                        value: root._wipeDirectionIndex(root._transition().angle)
                    }
                }
            }

            SettingItem {
                first: false
                last: false
                visible: root._transition().kind === WC.TransitionKind.TRANSITION_KIND_GROW

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Grow center, horizontal")
                    }

                    W.ValueSlider {
                        id: m_transition_origin_x
                        Layout.preferredWidth: 220
                        from: 0
                        to: 100
                        stepSize: 5
                        snapMode: T.Slider.SnapAlways
                        maxVisibleStops: 10
                        valueText: Math.round(value).toString()
                        valueMaxText: "100"
                        onMoved: root._mutTransition(config => {
                            config.originX = Math.round(value);
                        })
                    }
                    Binding {
                        target: m_transition_origin_x
                        property: "value"
                        value: root._transition().originX
                    }

                    MD.Text {
                        text: qsTr("%")
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }

            SettingItem {
                first: false
                last: true
                visible: root._transition().kind === WC.TransitionKind.TRANSITION_KIND_GROW

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Grow center, vertical")
                    }

                    W.ValueSlider {
                        id: m_transition_origin_y
                        Layout.preferredWidth: 220
                        from: 0
                        to: 100
                        stepSize: 5
                        snapMode: T.Slider.SnapAlways
                        maxVisibleStops: 10
                        valueText: Math.round(value).toString()
                        valueMaxText: "100"
                        onMoved: root._mutTransition(config => {
                            config.originY = Math.round(value);
                        })
                    }
                    Binding {
                        target: m_transition_origin_y
                        property: "value"
                        value: root._transition().originY
                    }

                    MD.Text {
                        text: qsTr("%")
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }

            SettingHeader { text: qsTr("Audio") }

            SettingItem {
                first: true
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Enable audio")
                    }

                    MD.Switch {
                        id: m_renderer_enable_audio
                        onToggled: root._mut(g => {
                            g["renderer.enable_audio"] = checked;
                        })
                    }
                    Binding {
                        target: m_renderer_enable_audio
                        property: "checked"
                        value: root._rendererAudioEnabled(root._currentGlobal())
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Enable audio capture")
                    }

                    MD.Switch {
                        id: m_audio_capture_enabled
                        onToggled: root._mut(g => {
                            g.audioCaptureEnabled = checked;
                        })
                    }
                    Binding {
                        target: m_audio_capture_enabled
                        property: "checked"
                        value: Boolean(root._currentGlobal()?.audioCaptureEnabled)
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Volume")
                    }

                    W.ValueSlider {
                        id: m_renderer_volume_slider
                        Layout.preferredWidth: 220
                        from: 0
                        to: 100
                        stepSize: 1
                        snapMode: T.Slider.SnapAlways
                        maxVisibleStops: 10
                        valueText: Math.round(value).toString()
                        valueMaxText: "100"
                        onMoved: root._mut(g => {
                            g["renderer.volume"] = Math.round(value);
                        })
                    }
                    Binding {
                        target: m_renderer_volume_slider
                        property: "value"
                        value: root._rendererVolume(root._currentGlobal())
                    }

                    MD.Text {
                        text: qsTr("%")
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }

            SettingItem {
                first: false
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Mute for other active playback streams")
                    }

                    MD.Switch {
                        id: m_mute_when_other_audio
                        onToggled: root._mut(g => {
                            g.muteWhenOtherAudio = checked;
                        })
                    }
                    Binding {
                        target: m_mute_when_other_audio
                        property: "checked"
                        value: Boolean(root._currentGlobal()?.muteWhenOtherAudio ?? false)
                    }
                }
            }

            SettingItem {
                first: false
                last: true

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Mute fade")
                    }

                    W.ValueSlider {
                        id: m_audio_fade_slider
                        Layout.preferredWidth: 220
                        from: 0
                        to: 2000
                        stepSize: 100
                        snapMode: T.Slider.SnapAlways
                        maxVisibleStops: 10
                        valueText: Math.round(value).toString()
                        valueMaxText: "2000"
                        onMoved: root._mut(g => {
                            g.audioFadeMs = Math.round(value);
                        })
                    }
                    Binding {
                        target: m_audio_fade_slider
                        property: "value"
                        value: Number(root._currentGlobal()?.audioFadeMs ?? 500)
                    }

                    MD.Text {
                        text: qsTr("ms")
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }

            SettingHeader { text: qsTr("Rotation") }

            SettingItem {
                first: true
                last: false

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Queue mode")
                    }

                    MD.ComboBox {
                        id: m_queue_box
                        Layout.preferredWidth: 180
                        mdState.size: MD.Enum.S
                        model: root.kQueueModes.map(o => o.label)
                        onActivated: idx => root._mut(g => {
                            g.queueMode = root.kQueueModes[idx].value;
                        })
                    }
                    Binding {
                        target: m_queue_box
                        property: "currentIndex"
                        value: root._queueIndex(root._currentGlobal()?.queueMode ?? "sequential")
                    }
                }
            }

            SettingItem {
                first: false
                last: true

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    FieldLabel {
                        Layout.fillWidth: true
                        text: qsTr("Rotation interval")
                    }

                    MD.TextField {
                        id: m_rot_field
                        Layout.preferredWidth: 120
                        mdState.size: MD.Enum.S
                        placeholderText: qsTr("Interval")
                        inputMethodHints: Qt.ImhDigitsOnly
                        validator: RegularExpressionValidator { regularExpression: /^\d*$/ }
                        onTextEdited: root._mut(g => {
                            g.rotationSecs = Number(text) || 0;
                        })
                        onEditingFinished: {
                            if (text.length === 0) {
                                text = "0";
                            }
                            root._mut(g => {
                                g.rotationSecs = Number(text) || 0;
                            });
                        }
                    }
                    Binding {
                        target: m_rot_field
                        property: "text"
                        value: String(root._currentGlobal()?.rotationSecs ?? 0)
                        when: ! m_rot_field.activeFocus
                    }

                    MD.Text {
                        text: qsTr("s")
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }

            SettingHeader { text: qsTr("Cache") }

            MD.ListItem {
                Layout.fillWidth: true
                index: 0
                model: null
                count: 2
                showDivider: false
                text: qsTr("Network image cache")
                supportText: qsTr("%1 of %2 used")
                    .arg(root.formatBytes(W.App.networkCacheSize))
                    .arg(root.formatBytes(W.App.networkCacheMaximumSize))
                corners: MD.Util.listCorners(index, count, 16)
                mdState.backgroundColor: MD.Token.color.surface_container

                trailing: MD.Button {
                    text: qsTr("Clear")
                    mdState.type: MD.Enum.BtText
                    enabled: W.App.networkCacheSize > 0
                    onClicked: {
                        W.App.clearNetworkCache();
                        W.Action.toast(qsTr("Network image cache cleared"));
                    }
                }
            }

            MD.ListItem {
                Layout.fillWidth: true
                index: 1
                model: null
                count: 2
                showDivider: false
                text: qsTr("Maximum cache size")
                corners: MD.Util.listCorners(index, count, 16)
                mdState.backgroundColor: MD.Token.color.surface_container

                below: W.ValueSlider {
                    id: m_cache_maximum_slider
                    width: parent.width
                    from: 256
                    to: 6144
                    stepSize: 256
                    snapMode: T.Slider.SnapAlways
                    maxVisibleStops: 8
                    valueText: root.formatBytes(Math.round(value) * 1024 * 1024)
                    valueMaxText: root.formatBytes(to * 1024 * 1024)
                    onMoved: {
                        if (!pressed)
                            W.Global.networkCacheMaximumMiB = Math.round(value);
                    }
                    onPressedChanged: {
                        if (!pressed)
                            W.Global.networkCacheMaximumMiB = Math.round(value);
                    }

                    Binding {
                        target: m_cache_maximum_slider
                        property: "value"
                        value: W.Global.networkCacheMaximumMiB
                    }
                }
            }
        }
    }
}
