pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui

MD.Page {
    id: root

    RendererListQuery {
        id: rendererQuery
        Component.onCompleted: reload()
    }

    RendererPluginListQuery {
        id: pluginQuery
        Component.onCompleted: reload()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Title bar
        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            Layout.topMargin: 12
            Layout.bottomMargin: 8
            spacing: 8

            MD.Text {
                text: "Renderers"
                typescale: MD.Token.typescale.title_large
                color: MD.Token.color.on_surface
            }

            Item { Layout.fillWidth: true }

            MD.IconButton {
                icon.name: MD.Token.icon.refresh
                onClicked: {
                    rendererQuery.reload();
                    pluginQuery.reload();
                }
            }
        }

        MD.Flickable {
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentHeight: contentCol.implicitHeight + 32

            ColumnLayout {
                id: contentCol
                width: parent.width
                spacing: 16

                // Active renderers
                MD.Text {
                    Layout.leftMargin: 16
                    text: "Active"
                    typescale: MD.Token.typescale.title_medium
                    color: MD.Token.color.on_surface
                }

                MD.Text {
                    Layout.leftMargin: 16
                    visible: !rendererQuery.renderers || rendererQuery.renderers.length === 0
                    text: "No active renderers"
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                }

                Repeater {
                    model: rendererQuery.renderers

                    Rectangle {
                        required property string modelData
                        required property int index

                        Layout.fillWidth: true
                        Layout.leftMargin: 16
                        Layout.rightMargin: 16
                        Layout.preferredHeight: 56
                        radius: 12
                        color: MD.Token.color.surface_container_high

                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 16
                            anchors.rightMargin: 8
                            spacing: 8

                            Rectangle {
                                Layout.preferredWidth: 8
                                Layout.preferredHeight: 8
                                radius: 4
                                color: MD.Token.color.primary
                            }

                            MD.Text {
                                text: modelData
                                typescale: MD.Token.typescale.body_medium
                                color: MD.Token.color.on_surface
                                Layout.fillWidth: true
                                elide: Text.ElideMiddle
                                font.family: "monospace"
                            }

                            MD.IconButton {
                                icon.name: MD.Token.icon.close
                                onClicked: {
                                    killQuery.rendererId = modelData;
                                    killQuery.reload();
                                }

                                RendererKillQuery {
                                    id: killQuery
                                    onStatusChanged: {
                                        if (status === 3)
                                            rendererQuery.reload();
                                    }
                                }
                            }
                        }
                    }
                }

                // Renderer plugins
                MD.Text {
                    Layout.leftMargin: 16
                    Layout.topMargin: 8
                    text: "Plugins"
                    typescale: MD.Token.typescale.title_medium
                    color: MD.Token.color.on_surface
                }

                MD.Text {
                    Layout.leftMargin: 16
                    visible: pluginQuery.supportedTypes && pluginQuery.supportedTypes.length > 0
                    text: "Supported types: " + (pluginQuery.supportedTypes ? pluginQuery.supportedTypes.join(", ") : "")
                    typescale: MD.Token.typescale.label_medium
                    color: MD.Token.color.on_surface_variant
                }

                Repeater {
                    model: pluginQuery.renderers

                    Rectangle {
                        required property var modelData

                        Layout.fillWidth: true
                        Layout.leftMargin: 16
                        Layout.rightMargin: 16
                        Layout.preferredHeight: 64
                        radius: 12
                        color: MD.Token.color.surface_container_high

                        RowLayout {
                            anchors.fill: parent
                            anchors.leftMargin: 16
                            anchors.rightMargin: 16
                            spacing: 12

                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 2

                                MD.Text {
                                    text: modelData.name || ""
                                    typescale: MD.Token.typescale.body_medium
                                    color: MD.Token.color.on_surface
                                }

                                MD.Text {
                                    text: (modelData.types ? modelData.types.join(", ") : "") + " | priority: " + (modelData.priority || 0)
                                    typescale: MD.Token.typescale.label_small
                                    color: MD.Token.color.on_surface_variant
                                }
                            }

                            MD.Text {
                                text: modelData.bin || ""
                                typescale: MD.Token.typescale.label_small
                                color: MD.Token.color.on_surface_variant
                                font.family: "monospace"
                            }
                        }
                    }
                }

                Item { Layout.preferredHeight: 16 }
            }
        }
    }
}
