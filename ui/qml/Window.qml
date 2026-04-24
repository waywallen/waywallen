pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQml
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Templates as T

import Qcm.Material as MD
import waywallen.ui as W

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

    W.HealthQuery {
        id: healthQuery
        Component.onCompleted: reload()
    }

    property int currentPage: 0

    readonly property bool isCompact: MD.MProp.size.isCompact

    readonly property var pageModel: [
        {
            icon: MD.Token.icon.wallpaper,
            name: "Wallpapers"
        },
        {
            icon: MD.Token.icon.monitor,
            name: "Displays"
        },
        {
            icon: MD.Token.icon.monitor_heart,
            name: "Status"
        }
    ]

    readonly property var pageComponents: [
        "qrc:/waywallen/ui/qml/page/WallpaperPage.qml",
        "qrc:/waywallen/ui/qml/page/DisplaysPage.qml",
        "qrc:/waywallen/ui/qml/page/StatusPage.qml",
    ]

    onCurrentPageChanged: {
        m_content.replace(m_content.currentItem, pageComponents[currentPage], {});
    }

    Component.onCompleted: {
        currentPageChanged();
    }

    MD.Popup {
        id: m_disconnect_overlay
        visible: !W.DaemonDBusClient.daemonAvailable
        closePolicy: T.Popup.NoAutoClose
        dim: true
        modal: true
        x: Math.round((parent.width - width) / 2)
        y: Math.round((parent.height - height) / 2)
        parent: T.Overlay.overlay
        bottomPadding: 24
        contentItem: Column {
            spacing: 24
            MD.DialogHeader {
                // anchors.horizontalCenter: parent.horizontalCenter
                title: "daemon not run"
            }

            MD.DialogButtonBox {
                width: parent.width
                standardButtons: T.DialogButtonBox.Retry
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
                    // showDivider: false

                    Behavior on implicitWidth {
                        NumberAnimation {
                            duration: MD.Token.duration.short4
                        }
                    }

                    onClicked: function (model) {
                        win.currentPage = model.index;
                    }

                    drawerHeader: ColumnLayout {
                        spacing: 0

                        RowLayout {
                            Layout.fillWidth: true
                            Layout.leftMargin: 16
                            Layout.rightMargin: 16
                            Layout.topMargin: 16
                            Layout.bottomMargin: 8
                            spacing: 12

                            Image {
                                Layout.preferredWidth: 32
                                Layout.preferredHeight: 32
                                source: "qrc:/waywallen/ui/assets/waywallen-ui.svg"
                                fillMode: Image.PreserveAspectFit
                                sourceSize.width: 64
                                sourceSize.height: 64
                            }

                            MD.Label {
                                Layout.fillWidth: true
                                text: "waywallen"
                                typescale: MD.Token.typescale.title_large
                            }
                        }

                        MD.Divider {
                            Layout.fillWidth: true
                        }
                    }

                    drawerContent: ColumnLayout {
                        spacing: 0

                        MD.Divider {
                            Layout.fillWidth: true
                        }

                        MD.DrawerItem {
                            Layout.fillWidth: true
                            Layout.leftMargin: 16
                            Layout.rightMargin: 16
                            icon.name: MD.Token.icon.info
                            text: "About"
                            onClicked: MD.Util.showPopup('waywallen.ui/PagePopup', {
                                source: 'waywallen.ui/AboutPage'
                            }, win)
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
