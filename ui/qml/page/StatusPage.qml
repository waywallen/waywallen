pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T
import Qcm.Material as MD
import waywallen.ui

MD.Page {
    id: root

    readonly property bool anyQuerying: healthQuery.querying
        || rendererQuery.querying
        || pluginQuery.querying
        || sourceQuery.querying

    component SectionTitle: MD.Text {
        Layout.leftMargin: 16
        Layout.topMargin: 4
        typescale: MD.Token.typescale.title_medium
        color: MD.Token.color.on_surface
    }

    component SectionHint: MD.Text {
        Layout.leftMargin: 16
        Layout.rightMargin: 16
        typescale: MD.Token.typescale.body_medium
        color: MD.Token.color.on_surface_variant
    }

    component SectionDivider: MD.Divider {
        Layout.fillWidth: true
        Layout.leftMargin: 16
        Layout.rightMargin: 16
        Layout.topMargin: 4
        Layout.bottomMargin: 4
    }

    HealthQuery {
        id: healthQuery
        Component.onCompleted: reload()
    }

    RendererListQuery {
        id: rendererQuery
        Component.onCompleted: reload()
    }

    RendererPluginListQuery {
        id: pluginQuery
        Component.onCompleted: reload()
    }

    SourceListQuery {
        id: sourceQuery
        Component.onCompleted: reload()
    }

    function reloadAll() {
        healthQuery.reload();
        rendererQuery.reload();
        pluginQuery.reload();
        sourceQuery.reload();
    }

    RendererKillQuery {
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
        title: "Kill renderer?"
        parent: T.Overlay.overlay
        standardButtons: T.Dialog.Cancel | T.Dialog.Ok

        contentItem: MD.Text {
            text: "Stop the renderer process\n\"" + killDialog.rendererId + "\"?\nUnsaved frame state may be lost."
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.on_surface_variant
            wrapMode: Text.WordWrap
        }

        onAccepted: {
            killQuery.rendererId = killDialog.rendererId;
            killQuery.reload();
        }
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
                text: "Status"
                typescale: MD.Token.typescale.title_large
                color: MD.Token.color.on_surface
            }

            Item { Layout.fillWidth: true }

            Item {
                Layout.preferredWidth: 40
                Layout.preferredHeight: 40

                MD.IconButton {
                    anchors.fill: parent
                    icon.name: MD.Token.icon.refresh
                    enabled: !root.anyQuerying
                    opacity: root.anyQuerying ? 0.0 : 1.0
                    onClicked: root.reloadAll()
                }

                MD.CircularIndicator {
                    anchors.centerIn: parent
                    width: 24
                    height: 24
                    visible: root.anyQuerying
                    running: root.anyQuerying
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
                spacing: 8

                // --- Daemon ---
                SectionTitle { text: "Daemon" }

                MD.Card {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    type: MD.Enum.CardFilled
                    implicitHeight: daemonContent.implicitHeight + 32

                    contentItem: ColumnLayout {
                        id: daemonContent
                        spacing: 8

                        RowLayout {
                            spacing: 8
                            MD.Text {
                                text: "Service:"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }
                            MD.Text {
                                text: healthQuery.service || "—"
                                typescale: MD.Token.typescale.body_medium
                                color: MD.Token.color.on_surface
                            }
                        }

                        RowLayout {
                            spacing: 8
                            MD.Text {
                                text: "State:"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }

                            Rectangle {
                                Layout.preferredWidth: 8
                                Layout.preferredHeight: 8
                                radius: 4
                                color: healthQuery.state === "healthy"
                                       ? MD.Token.color.primary
                                       : MD.Token.color.error
                            }

                            MD.Text {
                                text: healthQuery.state || (healthQuery.querying ? "Loading…" : "unknown")
                                typescale: MD.Token.typescale.body_medium
                                color: MD.Token.color.on_surface
                            }
                        }
                    }
                }

                // --- Active Renderers ---
                SectionDivider {}
                SectionTitle { text: "Active Renderers" }

                SectionHint {
                    visible: !rendererQuery.renderers || rendererQuery.renderers.length === 0
                    text: rendererQuery.querying ? "Loading…" : "No active renderers"
                }

                ListView {
                    Layout.fillWidth: true
                    implicitHeight: contentHeight
                    interactive: false

                    model: rendererQuery.renderers

                    delegate: MD.ListItem {
                        required property string modelData

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
                                killDialog.rendererId = modelData;
                                killDialog.open();
                            }
                        }
                    }
                }

                // --- Renderer Plugins ---
                SectionDivider {}
                SectionTitle { text: "Renderer Plugins" }

                SectionHint {
                    typescale: MD.Token.typescale.label_medium
                    visible: pluginQuery.supportedTypes && pluginQuery.supportedTypes.length > 0
                    text: "Supported types: " + (pluginQuery.supportedTypes ? pluginQuery.supportedTypes.join(", ") : "")
                }

                SectionHint {
                    visible: !pluginQuery.renderers || pluginQuery.renderers.length === 0
                    text: pluginQuery.querying ? "Loading…" : "No renderer plugins"
                }

                ListView {
                    Layout.fillWidth: true
                    implicitHeight: contentHeight
                    interactive: false

                    model: pluginQuery.renderers

                    delegate: MD.ListItem {
                        required property var modelData

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

                // --- Source Plugins ---
                SectionDivider {}
                SectionTitle { text: "Source Plugins" }

                SectionHint {
                    visible: !sourceQuery.sources || sourceQuery.sources.length === 0
                    text: sourceQuery.querying ? "Loading…" : "No source plugins loaded"
                }

                ListView {
                    Layout.fillWidth: true
                    implicitHeight: contentHeight
                    interactive: false

                    model: sourceQuery.sources

                    delegate: MD.ListItem {
                        required property var modelData

                        width: ListView.view.width
                        text: modelData.name || ""
                        supportText: "Types: " + (modelData.types ? modelData.types.join(", ") : "—")
                        leader: MD.Icon {
                            name: MD.Token.icon.source
                            size: 24
                            color: MD.Token.color.on_surface_variant
                        }
                        trailing: MD.Text {
                            text: "v" + (modelData.version || "?")
                            typescale: MD.Token.typescale.label_small
                            color: MD.Token.color.on_surface_variant
                        }
                    }
                }

                Item { Layout.preferredHeight: 16 }
            }
        }
    }
}
