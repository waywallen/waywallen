pragma ValueTypeBehavior: Assertable
import QtCore
import QtQuick
import QtQml
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Templates as T
import QtQuick.Controls as QC

import Qcm.Material as MD
import waywallen.ui

MD.ApplicationWindow {
    id: win
    MD.MProp.size.width: width
    MD.MProp.backgroundColor: {
        const c = MD.MProp.size.windowClass;
        switch (c) {
        case MD.Enum.WindowClassCompact:
            return MD.Token.color.surface;
        default:
            return MD.Token.color.surface_container;
        }
    }
    MD.MProp.textColor: MD.MProp.color.getOn(MD.MProp.backgroundColor)

    color: MD.MProp.backgroundColor
    height: 600
    visible: true
    width: 900
    title: "waywallen"

    HealthQuery {
        id: healthQuery
        Component.onCompleted: reload()
    }

    property int currentPage: 0

    readonly property bool isCompact: MD.MProp.size.isCompact

    readonly property var pageModel: [
        { icon: MD.Token.icon.wallpaper, name: "Wallpapers" },
        { icon: MD.Token.icon.tune, name: "Renderers" },
        { icon: MD.Token.icon.info, name: "Info" }
    ]

    readonly property var pageComponents: [
        "qrc:/waywallen/ui/qml/page/WallpaperPage.qml",
        "qrc:/waywallen/ui/qml/page/RenderersPage.qml",
        "qrc:/waywallen/ui/qml/page/InfoPage.qml"
    ]

    onCurrentPageChanged: {
        m_content.replace(m_content.currentItem, pageComponents[currentPage], {});
    }

    Component.onCompleted: {
        currentPageChanged();
    }

    // Disconnected overlay — shows when daemon is not reachable via DBus.
    Rectangle {
        id: m_disconnect_overlay
        anchors.fill: parent
        z: 1000
        visible: !DaemonDBusClient.daemonAvailable
        color: Qt.rgba(0, 0, 0, 0.6)

        MouseArea {
            anchors.fill: parent
            // Eat clicks so the UI behind is non-interactive.
        }

        ColumnLayout {
            anchors.centerIn: parent
            spacing: 16

            QC.Label {
                Layout.alignment: Qt.AlignHCenter
                text: "waywallen daemon 未运行"
                color: MD.Token.color.on_surface
                font.pixelSize: 18
            }

            QC.Button {
                Layout.alignment: Qt.AlignHCenter
                text: "启动 daemon"
                onClicked: DaemonDBusClient.launchDaemon()
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

            // --- Sidebar drawer (expanded mode) ---
            Loader {
                id: m_drawer_loader
                Layout.fillHeight: true
                active: !win.isCompact
                visible: active

                sourceComponent: MD.StandardDrawer {
                    model: win.pageModel
                    currentIndex: win.currentPage
                    showDivider: false

                    Behavior on implicitWidth {
                        NumberAnimation {
                            duration: MD.Token.duration.short4
                        }
                    }

                    onClicked: function (model) {
                        win.currentPage = model.index;
                    }

                    drawerContent: ColumnLayout {
                        spacing: 0

                        Item { Layout.fillHeight: true }

                        StatusDot {
                            Layout.alignment: Qt.AlignHCenter
                            Layout.bottomMargin: 16
                            statusColor: {
                                if (healthQuery.status === 3)
                                    return MD.Token.color.primary;
                                if (healthQuery.querying)
                                    return MD.Token.color.secondary;
                                return MD.Token.color.error;
                            }
                            statusText: {
                                if (healthQuery.status === 3) return "OK";
                                if (healthQuery.querying) return "…";
                                return "!";
                            }
                        }
                    }
                }
            }

            // --- Page content ---
            MD.StackView {
                id: m_content
                Layout.fillHeight: true
                Layout.fillWidth: true
                Layout.margins: win.isCompact ? 0 : 8
                clip: true
                initialItem: Item {}

                MD.MProp.page: m_page_ctx

                MD.PageContext {
                    id: m_page_ctx
                    showHeader: false
                    backgroundRadius: win.isCompact ? 0 : 12
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
