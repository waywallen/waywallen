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
                spacing: 4

                // Active renderers section
                MD.Text {
                    Layout.leftMargin: 16
                    Layout.bottomMargin: 4
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

                ListView {
                    Layout.fillWidth: true
                    implicitHeight: contentHeight
                    interactive: false

                    model: rendererQuery.renderers

                    delegate: MD.ListItem {
                        required property string modelData
                        required property int index

                        width: ListView.view.width
                        text: modelData
                        font.family: "monospace"
                        leader: MD.Icon {
                            name: MD.Token.icon.play_arrow
                            size: 24
                            color: MD.Token.color.primary
                        }
                        trailing: MD.IconButton {
                            icon.name: MD.Token.icon.close
                            onClicked: {
                                m_killQuery.rendererId = modelData;
                                m_killQuery.reload();
                            }

                            RendererKillQuery {
                                id: m_killQuery
                                onStatusChanged: {
                                    if (status === 3)
                                        rendererQuery.reload();
                                }
                            }
                        }
                    }
                }

                // Plugins section
                MD.Divider {
                    Layout.fillWidth: true
                    Layout.topMargin: 8
                    Layout.bottomMargin: 8
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                }

                MD.Text {
                    Layout.leftMargin: 16
                    Layout.bottomMargin: 4
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

                ListView {
                    Layout.fillWidth: true
                    implicitHeight: contentHeight
                    interactive: false

                    model: pluginQuery.renderers

                    delegate: MD.ListItem {
                        required property var modelData
                        required property int index

                        width: ListView.view.width
                        text: modelData.name || ""
                        supportText: (modelData.types ? modelData.types.join(", ") : "") + " | priority: " + (modelData.priority || 0)
                        leader: MD.Icon {
                            name: MD.Token.icon.extension
                            size: 24
                            color: MD.Token.color.on_surface_variant
                        }
                        trailing: MD.Text {
                            text: modelData.bin || ""
                            typescale: MD.Token.typescale.label_small
                            color: MD.Token.color.on_surface_variant
                            font.family: "monospace"
                        }
                    }
                }

                Item { Layout.preferredHeight: 16 }
            }
        }
    }
}
