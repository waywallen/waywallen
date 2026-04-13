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
                spacing: 16

                // Daemon status
                MD.Text {
                    Layout.leftMargin: 16
                    text: "Daemon"
                    typescale: MD.Token.typescale.title_medium
                    color: MD.Token.color.on_surface
                }

                Rectangle {
                    Layout.fillWidth: true
                    Layout.leftMargin: 16
                    Layout.rightMargin: 16
                    Layout.preferredHeight: daemonCol.implicitHeight + 24
                    radius: 12
                    color: MD.Token.color.surface_container_high

                    ColumnLayout {
                        id: daemonCol
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 16
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
                MD.Text {
                    Layout.leftMargin: 16
                    Layout.topMargin: 8
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

                Repeater {
                    model: sourceQuery.sources

                    Rectangle {
                        required property var modelData

                        Layout.fillWidth: true
                        Layout.leftMargin: 16
                        Layout.rightMargin: 16
                        Layout.preferredHeight: srcCol.implicitHeight + 24
                        radius: 12
                        color: MD.Token.color.surface_container_high

                        ColumnLayout {
                            id: srcCol
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.top: parent.top
                            anchors.margins: 16
                            spacing: 4

                            RowLayout {
                                Layout.fillWidth: true
                                spacing: 8

                                MD.Text {
                                    text: modelData.name || ""
                                    typescale: MD.Token.typescale.body_medium
                                    color: MD.Token.color.on_surface
                                    Layout.fillWidth: true
                                }

                                MD.Text {
                                    text: "v" + (modelData.version || "?")
                                    typescale: MD.Token.typescale.label_small
                                    color: MD.Token.color.on_surface_variant
                                }
                            }

                            MD.Text {
                                text: "Types: " + (modelData.types ? modelData.types.join(", ") : "—")
                                typescale: MD.Token.typescale.label_small
                                color: MD.Token.color.on_surface_variant
                            }
                        }
                    }
                }

                Item { Layout.preferredHeight: 16 }
            }
        }
    }
}
