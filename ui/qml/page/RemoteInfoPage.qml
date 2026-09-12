pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    title: qsTr("Remote info")
    scrolling: !infoFlick.atYBeginning

    property var itemStore: null
    readonly property var item: itemStore?.item ?? null
    property var details: null
    property string sourceName: ""
    property int remoteCapability: 0
    property string remoteHint: ""
    property var tagOptions: []

    // A source that cannot name an item while listing it may still name it once
    // the item is opened, so let a detail lookup fill in what the row lacks.
    readonly property string authorName: {
        const own = String(root.item?.author ?? "");
        return own.length > 0 ? own : String(root.details?.author ?? "");
    }

    readonly property string formattedSize: formatSize(details?.size)
    readonly property string tagsText: formatTagList(details?.tags)

    function value(v) {
        return v === undefined || v === null ? "" : String(v);
    }

    function hasText(v) {
        return value(v).length > 0;
    }

    function formatTagList(tags) {
        if (!tags || tags.length === 0)
            return "";
        const out = [];
        for (const tag of tags)
            out.push(W.I18n.optionLabel(root.tagOptions, tag));
        return out.join(", ");
    }

    function formatBytes(bytes) {
        let v = Number(bytes ?? 0);
        if (!(v > 0))
            return "";
        const u = ["B", "KB", "MB", "GB", "TB"];
        let i = 0;
        while (v >= 1024 && i < u.length - 1) {
            v /= 1024;
            ++i;
        }
        return v.toFixed(i === 0 ? 0 : 1) + " " + u[i];
    }

    function formatSize(s) {
        const text = String(s ?? "").trim();
        if (text.length === 0)
            return "";
        if (/^\d+$/.test(text))
            return formatBytes(Number(text));
        const m = text.match(/^([\d.,]+)\s*([KMGT]?B)$/i);
        if (!m)
            return text;
        const num = parseFloat(m[1].replace(/,/g, ""));
        if (isNaN(num))
            return text;
        const unit = m[2].toUpperCase();
        if (unit === "B")
            return formatBytes(num);
        return num.toFixed(1) + " " + unit;
    }

    function subscriptionText(state) {
        switch (Number(state ?? 0)) {
        case 1: return qsTr("Unsubscribed");
        case 2: return qsTr("Subscribed");
        case 3: return qsTr("Updating");
        default: return qsTr("Unknown");
        }
    }

    component InfoLabel: MD.Text {
        required property string label

        Layout.preferredWidth: 104
        Layout.alignment: Qt.AlignTop
        text: label
        typescale: MD.Token.typescale.label_medium
        color: MD.Token.color.on_surface_variant
        elide: Text.ElideRight
        maximumLineCount: 1
    }

    component InfoValue: MD.TextEdit {
        Layout.fillWidth: true
        Layout.preferredHeight: Math.max(24, contentHeight)
        readOnly: true
        selectByMouse: true
        persistentSelection: true
        typescale: MD.Token.typescale.body_medium
        color: MD.Token.color.on_surface
        wrapMode: TextEdit.WrapAnywhere
    }

    contentItem: MD.VerticalFlickable {
        id: infoFlick
        topMargin: 12
        bottomMargin: 24
        leftMargin: 16
        rightMargin: 16

        GridLayout {
            width: infoFlick.contentWidth
            columns: 2
            columnSpacing: 12
            rowSpacing: 10

            InfoLabel {
                visible: root.hasText(root.sourceName)
                label: qsTr("Source")
            }
            InfoValue {
                visible: root.hasText(root.sourceName)
                text: root.sourceName
            }

            InfoLabel { label: qsTr("Source ID") }
            InfoValue { text: root.value(root.item?.sourceId) }

            InfoLabel { label: qsTr("Item ID") }
            InfoValue { text: root.value(root.item?.itemId) }

            InfoLabel { label: qsTr("Title") }
            InfoValue { text: root.value(root.item?.title) }

            InfoLabel {
                visible: root.hasText(root.item?.wpType)
                label: qsTr("Type")
            }
            InfoValue {
                visible: root.hasText(root.item?.wpType)
                text: root.value(root.item?.wpType)
            }

            InfoLabel {
                visible: root.authorName.length > 0
                label: qsTr("Author")
            }
            InfoValue {
                visible: root.authorName.length > 0
                text: root.authorName
            }

            InfoLabel {
                visible: root.hasText(root.item?.previewUrl)
                label: qsTr("Preview")
            }
            InfoValue {
                visible: root.hasText(root.item?.previewUrl)
                text: root.value(root.item?.previewUrl)
            }

            InfoLabel {
                visible: root.hasText(root.formattedSize)
                label: qsTr("Size")
            }
            InfoValue {
                visible: root.hasText(root.formattedSize)
                text: root.formattedSize
            }

            InfoLabel {
                visible: Number(root.details?.width ?? 0) > 0
                label: qsTr("Width")
            }
            InfoValue {
                visible: Number(root.details?.width ?? 0) > 0
                text: String(root.details?.width ?? 0)
            }

            InfoLabel {
                visible: Number(root.details?.height ?? 0) > 0
                label: qsTr("Height")
            }
            InfoValue {
                visible: Number(root.details?.height ?? 0) > 0
                text: String(root.details?.height ?? 0)
            }

            InfoLabel {
                visible: root.remoteCapability === 1
                label: qsTr("Downloaded")
            }
            InfoValue {
                visible: root.remoteCapability === 1
                text: Number(root.item?.acquisitionState ?? 0) === 3 ? "true" : "false"
            }

            InfoLabel {
                visible: root.remoteCapability === 2
                label: qsTr("Subscription")
            }
            InfoValue {
                visible: root.remoteCapability === 2
                text: root.subscriptionText(root.item?.acquisitionState)
            }

            InfoLabel {
                visible: root.hasText(root.remoteHint)
                label: qsTr("Acquisition")
            }
            InfoValue {
                visible: root.hasText(root.remoteHint)
                text: root.remoteHint
            }

            InfoLabel {
                visible: root.hasText(root.tagsText)
                label: qsTr("Tags")
            }
            InfoValue {
                visible: root.hasText(root.tagsText)
                text: root.tagsText
            }

            InfoLabel {
                visible: root.hasText(root.details?.description)
                label: qsTr("Description")
            }
            InfoValue {
                visible: root.hasText(root.details?.description)
                text: root.value(root.details?.description)
            }
        }
    }
}
