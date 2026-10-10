pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    implicitWidth: aboutContent.implicitWidth + 32
    bottomPadding: 24

    // Result of a check started from this page: "", "latest" or "failed".
    property string checkResult: ""

    Connections {
        target: W.UpdateChecker
        function onCheckFinished(ok) {
            root.checkResult = ok ? "latest" : "failed";
        }
    }

    ColumnLayout {
        id: aboutContent

        anchors.centerIn: parent
        spacing: 16
        width: Math.min(parent.width - 32, implicitWidth)

        Image {
            Layout.alignment: Qt.AlignHCenter
            Layout.preferredWidth: 96
            Layout.preferredHeight: 96
            source: "qrc:/waywallen/ui/assets/waywallen-ui.svg"
            fillMode: Image.PreserveAspectFit
            visible: status === Image.Ready
        }

        MD.Text {
            Layout.alignment: Qt.AlignHCenter
            text: "waywallen"
            typescale: MD.Token.typescale.headline_large
            color: MD.Token.color.on_surface
        }

        MD.Text {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("Version %1").arg(Qt.application.version)
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.on_surface_variant
        }

        MD.Button {
            Layout.alignment: Qt.AlignHCenter
            visible: W.UpdateChecker.supported && !W.UpdateChecker.updateAvailable
            enabled: !W.UpdateChecker.checking
            text: {
                if (W.UpdateChecker.checking)
                    return qsTr("Checking for updates…");
                switch (root.checkResult) {
                case "latest":
                    return qsTr("You have the latest version");
                case "failed":
                    return qsTr("Could not check for updates");
                default:
                    return qsTr("Check for updates");
                }
            }
            mdState.type: MD.Enum.BtFilledTonal
            onClicked: W.UpdateChecker.check()
        }

        MD.Button {
            Layout.alignment: Qt.AlignHCenter
            visible: W.UpdateChecker.updateAvailable
            text: qsTr("Version %1 is available").arg(W.UpdateChecker.latestVersion)
            mdState.type: MD.Enum.BtFilled
            onClicked: MD.Util.openUrlExternally(W.UpdateChecker.releaseUrl)
        }

        Item {
            Layout.alignment: Qt.AlignHCenter
            implicitWidth: m_author_button.implicitWidth
            implicitHeight: m_author_button.contentItem.implicitHeight

            MD.Button {
                id: m_author_button
                anchors.centerIn: parent
                text: "hypengw"
                mdState.type: MD.Enum.BtText
                onClicked: MD.Util.openUrlExternally("https://github.com/hypengw")
            }
        }

        MD.Text {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("Wallpaper Manager for Linux")
            typescale: MD.Token.typescale.body_large
            color: MD.Token.color.on_surface
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }

        MD.Text {
            Layout.alignment: Qt.AlignHCenter
            text: qsTr("Waywallen is a dynamic wallpaper solution for Linux desktops.")
            typescale: MD.Token.typescale.body_medium
            color: MD.Token.color.on_surface_variant
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }

        MD.Divider {
            Layout.fillWidth: true
            Layout.topMargin: 8
            Layout.bottomMargin: 8
        }

        Item {
            Layout.fillWidth: true
            implicitWidth: actionFlow.desiredWidth
            implicitHeight: actionFlow.implicitHeight

            Flow {
                id: actionFlow

                anchors.horizontalCenter: parent.horizontalCenter
                width: Math.min(parent.width, desiredWidth)
                spacing: 8

                readonly property real desiredWidth: githubButton.implicitWidth
                                                     + issuesButton.implicitWidth
                                                     + donateButton.implicitWidth
                                                     + changelogButton.implicitWidth
                                                     + spacing * 3

                MD.Button {
                    id: githubButton
                    text: qsTr("GitHub")
                    mdState.type: MD.Enum.BtText
                    onClicked: MD.Util.openUrlExternally("https://github.com/waywallen")
                }

                MD.Button {
                    id: issuesButton
                    text: qsTr("Issues")
                    mdState.type: MD.Enum.BtText
                    onClicked: MD.Util.openUrlExternally("https://github.com/waywallen/waywallen/issues")
                }

                MD.Button {
                    id: donateButton
                    text: qsTr("Donate")
                    mdState.type: MD.Enum.BtText
                    onClicked: MD.Util.openUrlExternally("https://ko-fi.com/hypengw")
                }

                MD.Button {
                    id: changelogButton
                    text: qsTr("Changelog")
                    mdState.type: MD.Enum.BtText
                    onClicked: root.Window.window.showChangelog()
                }
            }
        }
    }
}
