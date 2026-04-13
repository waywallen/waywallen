pragma ValueTypeBehavior: Assertable
import QtCore
import QtQuick
import QtQml
import QtQuick.Window
import QtQuick.Layouts
import QtQuick.Templates as T

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

    RowLayout {
        anchors.fill: parent
        spacing: 0

        // Navigation rail
        Rectangle {
            Layout.fillHeight: true
            Layout.preferredWidth: 72
            color: MD.Token.color.surface

            ColumnLayout {
                anchors.fill: parent
                anchors.topMargin: 8
                spacing: 4

                Repeater {
                    model: [
                        { icon: MD.Token.icon.wallpaper, label: "Wallpapers" },
                        { icon: MD.Token.icon.tune, label: "Renderers" },
                        { icon: MD.Token.icon.info, label: "Info" }
                    ]

                    Rectangle {
                        required property var modelData
                        required property int index

                        Layout.preferredWidth: 56
                        Layout.preferredHeight: 56
                        Layout.alignment: Qt.AlignHCenter
                        radius: 16
                        color: win.currentPage === index
                               ? MD.Token.color.secondary_container
                               : "transparent"

                        ColumnLayout {
                            anchors.centerIn: parent
                            spacing: 2

                            MD.Icon {
                                Layout.alignment: Qt.AlignHCenter
                                name: modelData.icon
                                size: 24
                                color: win.currentPage === index
                                       ? MD.Token.color.on_secondary_container
                                       : MD.Token.color.on_surface_variant
                            }

                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                text: modelData.label
                                typescale: MD.Token.typescale.label_small
                                color: win.currentPage === index
                                       ? MD.Token.color.on_secondary_container
                                       : MD.Token.color.on_surface_variant
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: win.currentPage = index
                            cursorShape: Qt.PointingHandCursor
                        }
                    }
                }

                Item { Layout.fillHeight: true }

                // Connection status dot
                Rectangle {
                    Layout.preferredWidth: 12
                    Layout.preferredHeight: 12
                    Layout.alignment: Qt.AlignHCenter
                    Layout.bottomMargin: 16
                    radius: 6
                    color: {
                        if (healthQuery.status === 3)
                            return MD.Token.color.primary;
                        if (healthQuery.querying)
                            return MD.Token.color.secondary;
                        return MD.Token.color.error;
                    }

                    MD.Text {
                        anchors.top: parent.bottom
                        anchors.topMargin: 2
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: {
                            if (healthQuery.status === 3) return "OK";
                            if (healthQuery.querying) return "…";
                            return "!";
                        }
                        typescale: MD.Token.typescale.label_small
                        color: MD.Token.color.on_surface_variant
                    }
                }
            }
        }

        // Separator
        Rectangle {
            Layout.fillHeight: true
            Layout.preferredWidth: 1
            color: MD.Token.color.outline_variant
        }

        // Page content
        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: win.currentPage

            WallpaperPage {}
            RenderersPage {}
            InfoPage {}
        }
    }
}
