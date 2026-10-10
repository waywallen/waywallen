pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQml
import QtQuick.Window
import QtQuick.Layouts

import Qcm.Material as MD
import waywallen.ui as W

MD.ApplicationWindow {
    id: win
    MD.MProp.size.width: width
    MD.MProp.backgroundColor: {
        return MD.MProp.color.surface_container;
    }
    MD.MProp.textColor: MD.MProp.color.getOn(MD.MProp.backgroundColor)

    color: MD.MProp.backgroundColor
    title: "waywallen"

    readonly property alias popupPresenter: m_popup_presenter
    property var changelogPresentation: null
    property var aboutPresentation: null

    function presentPopup(source, properties) {
        const presentation = m_popup_presenter.present(source, properties || {});
        presentation.failed.connect(presentation, function (error) {
            W.Global.toastError(error);
        });
        if (presentation.status === MD.PopupPresentation.Error)
            W.Global.toastError(presentation.errorString);
        return presentation;
    }

    function showChangelog() {
        if (changelogPresentation?.active)
            return;
        changelogPresentation = presentPopup(changelogDialogComponent, {
            source: "qrc:/waywallen/ui/assets/waywallen-ui.releases.xml",
            title: qsTr("Changelog")
        });
    }

    function showAbout() {
        if (aboutPresentation?.active)
            return;
        aboutPresentation = presentPopup('waywallen.ui/PagePopup', {
            source: 'waywallen.ui/AboutPage'
        });
    }

    MD.PopupPresenter {
        id: m_popup_presenter
        host: win.contentItem
    }

    Component {
        id: changelogDialogComponent

        MD.ChangelogDialog {}
    }

    W.WindowState {
        id: windowState
        window: win
    }

    W.HealthQuery {
        id: healthQuery
    }

    W.GlobalPauseToggleQuery {
        id: globalPauseToggleQuery
        onToggled: paused => W.Action.toast(paused ? qsTr("Paused") : qsTr("Resumed"))
    }

    Shortcut {
        sequence: "Ctrl+P"
        context: Qt.ApplicationShortcut
        enabled: W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready && !globalPauseToggleQuery.querying
        onActivated: globalPauseToggleQuery.reload()
    }

    Connections {
        target: W.Notify
        function onDaemonReady() {
            healthQuery.reload();
        }
    }

    property int currentPage: 0
    property string pendingWallpaperId: ""

    readonly property bool isCompact: MD.MProp.size.isCompact

    readonly property var pageModel: [
        {
            icon: MD.Token.icon.wallpaper,
            name: qsTr("Wallpapers")
        },
        {
            icon: MD.Token.icon.explore,
            name: qsTr("Discover")
        },
        {
            icon: MD.Token.icon.monitor,
            name: qsTr("Displays")
        },
        {
            icon: MD.Token.icon.monitor_heart,
            name: qsTr("Status")
        }
    ]

    readonly property var pageComponents: ["qrc:/waywallen/ui/qml/page/WallpaperPage.qml", "qrc:/waywallen/ui/qml/page/DiscoverPage.qml", "qrc:/waywallen/ui/qml/page/DisplaysPage.qml", "qrc:/waywallen/ui/qml/page/StatusPage.qml"]

    readonly property var pageCacheable: [true, true, false, false]

    onCurrentPageChanged: {
        m_content.switchTo(pageComponents[currentPage], {}, pageCacheable[currentPage]);
    }

    function openWallpaper(wallpaperId) {
        pendingWallpaperId = String(wallpaperId || "");
        if (pendingWallpaperId.length === 0)
            return;
        if (currentPage !== 0)
            currentPage = 0;
        Qt.callLater(deliverPendingWallpaper);
    }

    function deliverPendingWallpaper() {
        if (currentPage !== 0 || pendingWallpaperId.length === 0)
            return;
        const page = m_content.currentItem;
        if (!page || typeof page.openWallpaper !== "function")
            return;
        const wallpaperId = pendingWallpaperId;
        pendingWallpaperId = "";
        page.openWallpaper(wallpaperId);
    }

    Component.onCompleted: {
        windowState.restore();
        currentPageChanged();
        // Level-check for the case where the daemon is already Ready
        // before this window finishes constructing (UI launched
        // standalone against a running daemon, page reload, etc.)
        // — `daemonReady` is edge-triggered and won't fire then.
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready) {
            healthQuery.reload();
        }
        if (W.Global.recordOpenedVersion(Qt.application.version))
            Qt.callLater(win.showChangelog);
    }

    Component.onDestruction: {
        changelogPresentation?.cancel();
        aboutPresentation?.cancel();
    }

    MD.SnakeView {
        id: m_snake
        parent: MD.Overlay.overlay
        anchors.fill: parent
        anchors.leftMargin: 24
        anchors.rightMargin: 24
    }

    Connections {
        target: W.Action
        function onToast(text, duration, flags, action) {
            m_snake.show(text, duration, flags, action);
        }
    }

    Connections {
        target: W.App
        function onErrorOccurred(error) {
            W.Global.toastError(error);
        }
    }

    // Global daemon-event toasts. Notify mirrors `GlobalEvent` from the
    // daemon; library additions surface here so the toast fires no
    // matter which page triggered the add (manual vs auto-detect).
    Connections {
        target: W.Notify
        function onLibrariesAdded(paths) {
            const n = paths.length;
            W.Action.toast(qsTr("%n library(s) added", "", n));
        }
        function onDisplayConnectionFailed(clientName, clientProtocolVersion, errorCode, reason) {
            const who = clientName.length > 0 ? clientName : qsTr("Display client");
            W.Global.toastError(qsTr("%1 connection failed: %2").arg(who).arg(reason));
        }
        function onPluginRestartFailed(pluginId, error) {
            const who = pluginId.length > 0 ? pluginId : qsTr("Plugin");
            W.Action.toast(qsTr("%1 renderer restart failed: %2").arg(who).arg(error), 6000, 1, null);
        }
    }

    W.DaemonNotRunDialog {}
    W.QrLoginDialog {}

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

            // --- Navigation rail (collapses to 96dp; auto-expands when
            // the window is wide enough to embed) ---
            Loader {
                id: m_drawer_loader
                Layout.fillHeight: true
                active: !win.isCompact
                visible: active

                sourceComponent: MD.NavigationRail {
                    id: m_rail
                    model: win.pageModel
                    currentIndex: win.currentPage

                    autoExpand: W.Global.sidebarAutoExpand
                    // The rail only re-syncs `expanded` on window-class
                    // changes; apply a runtime toggle of the setting at once.
                    onAutoExpandChanged: if (autoExpand)
                        expanded = useEmbed

                    onClicked: function (model) {
                        win.currentPage = model.index;
                    }

                    // Logo + a menu-toggle button (the rail's default header
                    // is just the toggle; we add branding alongside it).
                    header: Item {
                        implicitWidth: MD.Util.lerp(m_rail.collapsedWidth, m_rail.expandedWidth, m_rail.expansionProgress)
                        implicitHeight: m_logo.y + m_logo.height + 12

                        MD.StandardIconButton {
                            id: m_menu_btn
                            x: MD.Util.lerp((m_rail.collapsedWidth - width) / 2, 32 - (width - 24) / 2, m_rail.expansionProgress)
                            y: 4
                            icon.name: m_rail.useLarge ? MD.Token.icon.menu_open : MD.Token.icon.menu
                            onClicked: m_rail.toggle()
                        }

                        Image {
                            id: m_logo
                            width: 32
                            height: 32
                            x: MD.Util.lerp((m_rail.collapsedWidth - width) / 2, 32, m_rail.expansionProgress)
                            y: m_menu_btn.y + m_menu_btn.height + 16
                            source: "../assets/waywallen-ui.svg"
                            fillMode: Image.PreserveAspectFit
                            sourceSize.width: 64
                            sourceSize.height: 64

                            readonly property bool updateAvailable: W.UpdateChecker.enabled && W.UpdateChecker.updateAvailable

                            MD.Badge {
                                dot: true
                                visible: m_logo.updateAvailable
                            }

                            MouseArea {
                                id: m_logo_area
                                anchors.fill: parent
                                enabled: m_logo.updateAvailable
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: win.showAbout()
                            }

                            MD.ToolTip.visible: m_logo_area.containsMouse && m_logo.updateAvailable
                            MD.ToolTip.delay: 300
                            MD.ToolTip.text: qsTr("Version %1 is available").arg(W.UpdateChecker.latestVersion)
                        }

                        MD.Label {
                            visible: opacity > 0
                            opacity: m_rail.expansionProgress
                            anchors.left: m_logo.right
                            anchors.leftMargin: 12
                            anchors.verticalCenter: m_logo.verticalCenter
                            text: "waywallen"
                            typescale: MD.Token.typescale.title_large
                        }
                    }

                    footer: Item {
                        implicitHeight: m_rail_footer.implicitHeight + m_about.height + m_rail_footer.spacing * m_rail.expansionProgress

                        Column {
                            id: m_rail_footer
                            width: parent.width
                            spacing: 12 * (1 - m_rail.expansionProgress)

                            W.SidebarNowPlaying {
                                id: m_now_playing
                                width: parent.width
                                visible: W.App.presentationManager.count > 0
                                expanded: m_rail.useLarge
                                expansionProgress: m_rail.expansionProgress
                                model: W.App.presentationManager.model
                                onOpenRequested: wallpaperId => win.openWallpaper(wallpaperId)

                                Binding {
                                    target: m_rail
                                    property: "drawerGestureEnabled"
                                    value: !m_now_playing.pointerHovered
                                }
                            }

                            MD.RailItem {
                                width: parent.width
                                expand: m_rail.useLarge
                                expansionProgress: m_rail.expansionProgress
                                checked: false
                                icon.name: MD.Token.icon.extension
                                iconStyle: MD.Enum.IconAndText
                                collapsedIconStyle: MD.Enum.IconOnly
                                text: qsTr("Plugins")
                                property var presentation: null
                                onClicked: {
                                    if (presentation?.active)
                                        return;
                                    presentation = win.presentPopup('waywallen.ui/PagePopup', {
                                        source: 'waywallen.ui/PluginManagePage'
                                    });
                                }
                            }

                            MD.RailItem {
                                width: parent.width
                                expand: m_rail.useLarge
                                expansionProgress: m_rail.expansionProgress
                                checked: false
                                icon.name: MD.Token.icon.settings
                                iconStyle: MD.Enum.IconAndText
                                collapsedIconStyle: MD.Enum.IconOnly
                                text: qsTr("Settings")
                                property var presentation: null
                                onClicked: {
                                    if (presentation?.active)
                                        return;
                                    presentation = win.presentPopup('waywallen.ui/PagePopup', {
                                        source: 'waywallen.ui/SettingsPage'
                                    });
                                }
                            }
                        }

                        MD.RailItem {
                            id: m_about
                            visible: opacity > 0
                            opacity: m_rail.expansionProgress
                            width: parent.width
                            y: m_rail_footer.height + m_rail_footer.spacing * m_rail.expansionProgress
                            expand: true
                            checked: false
                            icon.name: MD.Token.icon.info
                            text: qsTr("About")
                            height: implicitHeight * m_rail.expansionProgress
                            enabled: m_rail.expansionProgress === 1
                            onClicked: win.showAbout()
                        }
                    }
                }
            }

            // --- Page content ---
            MD.PageContainer {
                id: m_content
                Layout.fillHeight: true
                Layout.fillWidth: true
                clip: true
                initialItem: Item {}

                onCurrentItemChanged: Qt.callLater(win.deliverPendingWallpaper)

                MD.MProp.page: m_page_ctx

                MD.PageContext {
                    id: m_page_ctx
                    showHeader: false
                    backgroundRadius: win.isCompact ? 0 : MD.Token.shape.corner.large
                    showBackground: !win.isCompact
                }
            }
        }

        // --- Bottom navigation bar (compact mode) ---
        Loader {
            id: m_bar_loader
            Layout.fillWidth: true
            active: win.isCompact
            visible: active

            sourceComponent: MD.Pane {
                padding: 0
                backgroundColor: MD.MProp.color.surface_container
                elevation: MD.Token.elevation.level2

                contentItem: RowLayout {
                    Repeater {
                        model: win.pageModel

                        Item {
                            Layout.fillWidth: true
                            implicitHeight: 12 + children[0].implicitHeight + 16
                            required property var modelData
                            required property int index

                            MD.BarItem {
                                anchors.fill: parent
                                anchors.topMargin: 12
                                anchors.bottomMargin: 16
                                icon.name: parent.modelData.icon
                                text: parent.modelData.name
                                checked: win.currentPage === parent.index
                                onClicked: win.currentPage = parent.index
                            }
                        }
                    }
                }
            }
        }
    }
}
