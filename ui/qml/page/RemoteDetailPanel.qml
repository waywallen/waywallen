pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

Item {
    id: root

    property var item: null
    property var details: null
    property bool nestedScrollEnabled: false

    // A source that cannot name an item while listing it may still name it once
    // the item is opened, so let a detail lookup fill in what the row lacks.
    readonly property string authorName: {
        const own = String(root.item?.author ?? "");
        return own.length > 0 ? own : String(root.details?.author ?? "");
    }
    readonly property string authorAvatarUrl: {
        const own = String(root.item?.authorAvatarUrl ?? "");
        return own.length > 0 ? own : String(root.details?.authorAvatarUrl ?? "");
    }
    readonly property string authorUrl: String(root.details?.authorUrl ?? "")
    property int remoteCapability: 0
    property string remoteHint: ""
    property var downloadAction: null
    property var tagOptions: []
    property int downloadState: 0
    property int subscriptionState: 0

    signal back
    signal showInfo
    signal downloadRequested
    signal removeRequested
    signal subscriptionRefreshRequested
    signal subscriptionChangeRequested(bool subscribed)

    MD.Action {
        id: linkAction
        text: qsTr("Open web page")
        icon.name: MD.Token.icon.web
        visible: String(root.details?.webUrl ?? "").length > 0
        onTriggered: MD.Util.openUrlExternally(root.details.webUrl)
    }

    MD.Action {
        id: infoAction
        text: qsTr("Info")
        icon.name: MD.Token.icon.info
        enabled: root.item !== null
        onTriggered: root.showInfo()
    }

    MD.Action {
        id: closeAction
        text: qsTr("Close")
        icon.name: MD.Token.icon.close
        onTriggered: root.back()
    }

    readonly property list<MD.Action> detailActions: [linkAction, infoAction, closeAction]

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        MD.VerticalListView {
            id: detailView
            MD.NestedScroll.enabled: root.nestedScrollEnabled
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            model: []
            spacing: 8
            leftMargin: 16
            rightMargin: 16
            topMargin: 0
            bottomMargin: 8

            header: ColumnLayout {
                width: detailView.contentWidth
                spacing: 12

                W.ThumbnailImage {
                    Layout.fillWidth: true
                    Layout.minimumHeight: 200
                    Layout.preferredHeight: 200
                    Layout.topMargin: 4
                    source: root.item?.previewUrl ?? ""
                    fillMode: Image.PreserveAspectFit
                }

                MD.Text {
                    Layout.fillWidth: true
                    text: root.item?.title || qsTr("Untitled")
                    typescale: MD.Token.typescale.title_large
                    color: MD.Token.color.on_surface
                    wrapMode: Text.Wrap
                    maximumLineCount: 2
                    elide: Text.ElideRight
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    MD.Text {
                        Layout.fillWidth: true
                        text: root.item?.wpType ?? ""
                        typescale: MD.Token.typescale.label_large
                        color: MD.Token.color.on_surface_variant
                        maximumLineCount: 1
                        elide: Text.ElideRight
                    }

                    W.DetailActionBar {
                        actions: root.detailActions
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    visible: root.authorName.length > 0
                    Accessible.role: root.authorUrl.length > 0 ? Accessible.Link : Accessible.StaticText
                    Accessible.name: root.authorName
                    Accessible.onPressAction: {
                        if (root.authorUrl.length > 0)
                            MD.Util.openUrlExternally(root.authorUrl);
                    }

                    TapHandler {
                        enabled: root.authorUrl.length > 0
                        onTapped: MD.Util.openUrlExternally(root.authorUrl)
                    }

                    HoverHandler {
                        enabled: root.authorUrl.length > 0
                        cursorShape: Qt.PointingHandCursor
                    }

                    W.ThumbnailImage {
                        Layout.preferredWidth: 32
                        Layout.preferredHeight: 32
                        Layout.maximumWidth: 32
                        Layout.maximumHeight: 32
                        visible: root.authorAvatarUrl.length > 0
                        source: root.authorAvatarUrl
                        maxSize: Qt.size(64, 64)
                        fillMode: Image.PreserveAspectCrop
                        radius: 16
                    }

                    MD.Text {
                        Layout.fillWidth: true
                        text: qsTr("by %1").arg(root.authorName)
                        textFormat: Text.PlainText
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface_variant
                        wrapMode: Text.WordWrap
                    }
                }

                GridLayout {
                    id: metadata
                    Layout.fillWidth: true
                    columns: 2
                    columnSpacing: 12
                    rowSpacing: 4
                    visible: hasResolution || hasSize

                    readonly property bool hasResolution:
                        Number(root.details?.width ?? 0) > 0 && Number(root.details?.height ?? 0) > 0
                    readonly property string formattedSize: W.I18n.formatSize(root.details?.size)
                    readonly property bool hasSize: formattedSize.length > 0

                    MD.Text {
                        visible: metadata.hasResolution
                        text: qsTr("Resolution")
                        typescale: MD.Token.typescale.label_medium
                        color: MD.Token.color.on_surface_variant
                    }
                    MD.Text {
                        visible: metadata.hasResolution
                        text: (root.details?.width ?? 0) + "×" + (root.details?.height ?? 0)
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface
                    }

                    MD.Text {
                        visible: metadata.hasSize
                        text: qsTr("Size")
                        typescale: MD.Token.typescale.label_medium
                        color: MD.Token.color.on_surface_variant
                    }
                    MD.Text {
                        visible: metadata.hasSize
                        text: metadata.formattedSize
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface
                    }
                }

                W.RemoteDetailFields {
                    Layout.fillWidth: true
                    fields: root.details?.fields ?? []
                    labelForField: field => W.I18n.tr(field.labelText) || String(field.label ?? "")
                }

                Flow {
                    Layout.fillWidth: true
                    spacing: 6
                    visible: (root.details?.tags?.length ?? 0) > 0

                    Repeater {
                        model: root.details?.tags ?? []
                        delegate: W.Tag {
                            required property string modelData
                            text: W.I18n.optionLabel(root.tagOptions, modelData)
                        }
                    }
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 4
                    visible: String(root.details?.description ?? "").length > 0
                             || Boolean(root.details?.querying)

                    MD.Divider { Layout.fillWidth: true }

                    MD.Text {
                        text: qsTr("Description")
                        typescale: MD.Token.typescale.label_large
                        color: MD.Token.color.on_surface_variant
                    }
                    MD.Text {
                        Layout.fillWidth: true
                        text: root.details?.querying ? qsTr("Loading…") : (root.details?.description ?? "")
                        visible: text.length > 0
                        typescale: MD.Token.typescale.body_medium
                        color: MD.Token.color.on_surface
                        wrapMode: Text.WordWrap
                    }
                }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            Layout.topMargin: 8
            Layout.bottomMargin: 8
            spacing: 8
            visible: root.remoteCapability === 1 || root.remoteCapability === 2

            Item {
                id: downloadControl
                visible: root.remoteCapability === 1
                Layout.fillWidth: true
                implicitHeight: downloadButton.implicitHeight

                readonly property string label: W.I18n.tr(root.downloadAction?.labelText ?? "")
                readonly property string tooltip: root.downloadState === 3 ? "" : W.I18n.tr(root.downloadAction?.tooltipText ?? "")

                HoverHandler {
                    id: downloadHover
                }
                MD.ToolTip.visible: downloadHover.hovered && downloadControl.tooltip.length > 0
                MD.ToolTip.text: downloadControl.tooltip

                MD.Button {
                    id: downloadButton
                    anchors.fill: parent
                    mdState.type: root.downloadState === 3 ? MD.Enum.BtFilledTonal : MD.Enum.BtFilled
                    enabled: root.downloadState === 3 || (root.downloadAction?.enabled ?? true) && root.downloadState !== 1 && root.downloadState !== 2
                    text: {
                        switch (root.downloadState) {
                        case 1: return qsTr("Pending");
                        case 2: return qsTr("Downloading");
                        case 3: return qsTr("Remove");
                        }
                        if (downloadControl.label.length > 0)
                            return downloadControl.label;
                        return root.downloadState === 4 || root.downloadState === 5 ? qsTr("Retry") : qsTr("Download");
                    }
                    onClicked: {
                        if (root.downloadState === 3)
                            root.removeRequested();
                        else
                            root.downloadRequested();
                    }
                }
            }

            MD.Button {
                visible: root.remoteCapability === 2
                Layout.fillWidth: true
                mdState.type: root.subscriptionState === 2 ? MD.Enum.BtFilledTonal : MD.Enum.BtFilled
                enabled: root.subscriptionState !== 3
                text: {
                    switch (root.subscriptionState) {
                    case 0: return qsTr("Retry");
                    case 1: return qsTr("Subscribe");
                    case 2: return qsTr("Unsubscribe");
                    case 3: return qsTr("Updating…");
                    default: return qsTr("Subscribe");
                    }
                }
                onClicked: {
                    if (root.subscriptionState === 0)
                        root.subscriptionRefreshRequested();
                    else
                        root.subscriptionChangeRequested(root.subscriptionState === 1);
                }
            }

            MD.Text {
                visible: root.remoteCapability !== 0 && root.remoteHint.length > 0
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                color: MD.Token.color.on_surface_variant
                typescale: MD.Token.typescale.body_small
                text: root.remoteHint
            }
        }
    }
}
