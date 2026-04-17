pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui

MD.Page {
    id: root

    HealthQuery {
        id: healthQuery
        Component.onCompleted: reload()
    }

    SourceListQuery {
        id: sourceQuery
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
                text: "Info"
                typescale: MD.Token.typescale.title_large
                color: MD.Token.color.on_surface
            }

            Item { Layout.fillWidth: true }

            MD.IconButton {
                icon.name: MD.Token.icon.refresh
                onClicked: {
                    healthQuery.reload();
                    sourceQuery.reload();
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
                spacing: 12

                // Daemon status card
                MD.Text {
                    Layout.leftMargin: 16
                    text: "Daemon"
                    typescale: MD.Token.typescale.title_medium
                    color: MD.Token.color.on_surface
                }

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
                                text: healthQuery.state || "unknown"
                                typescale: MD.Token.typescale.body_medium
                                color: MD.Token.color.on_surface
                            }
                        }
                    }
                }

                // Sources section
                MD.Divider {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                }

                MD.Text {
                    Layout.leftMargin: 16
                    text: "Source Plugins"
                    typescale: MD.Token.typescale.title_medium
                    color: MD.Token.color.on_surface
                }

                MD.Text {
                    Layout.leftMargin: 16
                    visible: !sourceQuery.sources || sourceQuery.sources.length === 0
                    text: sourceQuery.querying ? "Loading…" : "No source plugins loaded"
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
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
