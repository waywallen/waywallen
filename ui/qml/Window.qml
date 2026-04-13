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

    readonly property var pageModel: [
        { icon: MD.Token.icon.wallpaper, label: "Wallpapers" },
        { icon: MD.Token.icon.tune, label: "Renderers" },
        { icon: MD.Token.icon.info, label: "Info" }
    ]

    // --- Expanded layout: left rail + content ---
    RowLayout {
        id: m_large_layout
        anchors.fill: parent
        visible: false
        spacing: 0

        ColumnLayout {
            Layout.fillHeight: true
            Layout.preferredWidth: 72
            Layout.topMargin: 8
            spacing: 0

            Repeater {
                model: win.pageModel

                MD.RailItem {
                    required property var modelData
                    required property int index

                    Layout.alignment: Qt.AlignHCenter
                    icon.name: modelData.icon
                    text: modelData.label
                    checked: win.currentPage === index
                    onClicked: win.currentPage = index
                }
            }

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

        MD.Divider {}

        LayoutItemProxy {
            Layout.fillWidth: true
            Layout.fillHeight: true
            target: m_content
        }
    }

    // --- Compact layout: content + bottom nav ---
    ColumnLayout {
        id: m_small_layout
        anchors.fill: parent
        visible: false
        spacing: 0

        LayoutItemProxy {
            Layout.fillWidth: true
            Layout.fillHeight: true
            target: m_content
        }

        MD.Pane {
            Layout.fillWidth: true
            padding: 0
            backgroundColor: MD.MProp.color.surface_container
            elevation: MD.Token.elevation.level2

            RowLayout {
                anchors.fill: parent

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
                            text: parent.modelData.label
                            checked: win.currentPage === parent.index
                            onClicked: win.currentPage = parent.index
                        }
                    }
                }
            }
        }
    }

    // --- Shared content (owned off-screen, proxied into active layout) ---
    Item {
        visible: false

        QC.StackView {
            id: m_content
            implicitWidth: 800
            implicitHeight: 600
            clip: true

            initialItem: m_wallpaperPage

            Connections {
                target: win
                function onCurrentPageChanged() {
                    const pages = [m_wallpaperPage, m_renderersPage, m_infoPage];
                    const page = pages[win.currentPage];
                    if (m_content.currentItem !== page) {
                        m_content.replace(page, QC.StackView.CrossFade);
                    }
                }
            }
        }

        WallpaperPage {
            id: m_wallpaperPage
        }
        RenderersPage {
            id: m_renderersPage
        }
        InfoPage {
            id: m_infoPage
        }
    }

    // --- Layout switch on window class change ---
    Connections {
        target: win.MD.MProp.size
        function onWindowClassChanged() {
            const isCompact = win.MD.MProp.size.isCompact;
            m_small_layout.visible = isCompact;
            m_large_layout.visible = !isCompact;
        }
        Component.onCompleted: {
            this.onWindowClassChanged();
        }
    }
}
