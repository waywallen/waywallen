pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

// Renderer/plugin settings hosted inside PagePopup. Props are an open-time
// snapshot; edits stay pending until Apply.
MD.Page {
    id: root
    title: qsTr("Configure %1").arg(displayName.length > 0 ? displayName : pluginName)
    scrolling: !settingsList.atYBeginning

    property string pluginName: ""
    // Human-readable name for the title; falls back to pluginName (the config key).
    property string displayName: ""
    // Plugin-declared read-only status rows and action buttons.
    property var statusList: []
    property var actionList: []

    W.PluginActionQuery {
        id: actionQuery
        forwardError: false
        onCompleted: function (accepted, error, sessionId) {
            if (!accepted)
                W.Global.toastError(error.length > 0 ? error : qsTr("Action failed"));
        }
    }
    function runAction(actionId) {
        actionQuery.pluginId = root.pluginName;
        actionQuery.actionId = actionId;
        actionQuery.reload();
    }

    // Live-refresh the status rows and action buttons after a sign-in/out (or any
    // settings change) so the page updates in place instead of needing a reopen.
    W.RemoteAvailabilityQuery {
        id: availabilityQuery
        onSourcesChanged: {
            const list = sources || [];
            for (let i = 0; i < list.length; ++i) {
                if (list[i].id === root.pluginName) {
                    root.statusList = list[i].status || [];
                    root.actionList = list[i].actions || [];
                    return;
                }
            }
        }
    }

    Connections {
        target: W.Notify
        function onSettingsChanged() {
            availabilityQuery.reload();
        }
        function onPluginStateChanged() {
            availabilityQuery.reload();
        }
    }
    property var schemaList: []
    property var currentValues: ({})
    // SettingsSet is full-replace, so we forward the rest of the plugin
    // map and the global block verbatim — otherwise editing one plugin
    // would wipe everyone else.
    property var allCurrentPlugins: ({})
    property var currentGlobal: ({})
    property var pendingValues: ({})

    W.SettingsSetQuery {
        id: setQuery
        // 2 = QAsyncResult::Status::Finished.
        onStatusChanged: {
            if (status === 2)
                MD.Util.closePopup(root.MD.MProp.page);
        }
    }

    function valueFor(key) {
        const pv = root.pendingValues;
        const cv = root.currentValues;
        if (key in pv)
            return pv[key];
        if (key in cv)
            return cv[key];
        for (let i = 0; i < root.schemaList.length; ++i) {
            const s = root.schemaList[i];
            if (s.key === key)
                return s.default_value;
        }
        return "";
    }

    function reset() {
        root.pendingValues = ({});
    }

    function _serialize(map) {
        const keys = Object.keys(map).sort();
        const out = {};
        for (let i = 0; i < keys.length; ++i)
            out[keys[i]] = map[keys[i]];
        return JSON.stringify(out);
    }

    function _baseline() {
        const m = ({});
        for (let i = 0; i < schemaList.length; ++i) {
            const s = schemaList[i];
            m[s.key] = s.default_value;
        }
        for (const k in currentValues)
            m[k] = currentValues[k];
        return m;
    }

    function _merged() {
        const m = _baseline();
        for (const k in pendingValues)
            m[k] = pendingValues[k];
        return m;
    }

    // Compare serialized baseline to merged-with-pending; only enable
    // Apply/Reset when the user has produced a real delta (a no-op edit
    // — type the current value, then back out — leaves us clean).
    readonly property bool isDirty: _serialize(_baseline()) !== _serialize(_merged())

    function apply() {
        const plugins = Object.assign({}, root.allCurrentPlugins);
        plugins[root.pluginName] = _merged();
        setQuery.global = root.currentGlobal;
        setQuery.plugins = plugins;
        setQuery.reload();
    }

    readonly property var flatSchemas: {
        const buckets = {};
        for (let i = 0; i < schemaList.length; ++i) {
            const s = schemaList[i];
            const localizedGroup = s.group_label || ({});
            const key = (s.group && s.group.length > 0) ? s.group : "";
            if (!buckets[key]) {
                buckets[key] = {
                    label: W.I18n.tr(localizedGroup) || s.group || qsTr("General"),
                    items: []
                };
            }
            buckets[key].items.push(s);
        }
        const keys = Object.keys(buckets).sort();
        const out = [];
        for (let i = 0; i < keys.length; ++i) {
            const k = keys[i];
            const bucket = buckets[k];
            const items = bucket.items;
            items.sort(function (a, b) {
                return (a.order || 0) - (b.order || 0);
            });
            for (let j = 0; j < items.length; ++j) {
                let pos;
                if (items.length === 1)
                    pos = "single";
                else if (j === 0)
                    pos = "first";
                else if (j === items.length - 1)
                    pos = "last";
                else
                    pos = "middle";
                out.push({
                    "group": bucket.label,
                    "schema": items[j],
                    "position": pos
                });
            }
        }
        return out;
    }

    footer: MD.DialogButtonBox {
        horizontalPadding: 24
        topPadding: 12
        bottomPadding: 16

        MD.Button {
            text: qsTr("Reset")
            mdState.type: MD.Enum.BtText
            enabled: root.isDirty
            MD.DialogButtonBox.buttonRole: MD.DialogButtonBox.ResetRole
            onClicked: root.reset()
        }
        MD.Button {
            text: qsTr("Apply")
            mdState.type: MD.Enum.BtText
            enabled: root.isDirty
            MD.DialogButtonBox.buttonRole: MD.DialogButtonBox.ApplyRole
            onClicked: root.apply()
        }
    }

    contentItem: MD.VerticalLazyList {
        id: settingsList
        expand: true
        clip: true
        extraTopMargin: 4
        extraBottomMargin: 24
        extraLeftMargin: 16
        extraRightMargin: 16
        source: MD.ListSnapshotSource {
            id: settingsSource
            items: root.flatSchemas
        }
        spacing: 2

        // Shown only when an Apply fails. 3 = QAsyncResult::Status::Error.
        header: Item {
            implicitHeight: errorText.visible ? errorText.implicitHeight + 8 : 0
            MD.Text {
                id: errorText
                width: parent.width
                visible: setQuery.status === 3
                text: setQuery.error
                color: MD.Token.color.error
                typescale: MD.Token.typescale.body_small
                wrapMode: Text.WordWrap
            }
        }

        footer: ColumnLayout {
            width: settingsList.contentWidth
            spacing: 6

            readonly property string sectionLabel: {
                if (root.statusList.length > 0 && root.statusList[0].group)
                    return W.I18n.tr(root.statusList[0].groupLabel) || root.statusList[0].group;
                if (root.actionList.length > 0 && root.actionList[0].group)
                    return W.I18n.tr(root.actionList[0].groupLabel) || root.actionList[0].group;
                return "";
            }

            MD.Text {
                Layout.fillWidth: true
                visible: parent.sectionLabel.length > 0
                text: parent.sectionLabel
                typescale: MD.Token.typescale.title_small
                color: MD.Token.color.on_surface_variant
                topPadding: 16
                bottomPadding: 6
                leftPadding: 4
            }

            Repeater {
                model: root.statusList
                delegate: RowLayout {
                    required property var modelData
                    Layout.fillWidth: true
                    Layout.leftMargin: 4
                    Layout.rightMargin: 4
                    MD.Text {
                        text: W.I18n.tr(modelData.labelText)
                        color: MD.Token.color.on_surface_variant
                        typescale: MD.Token.typescale.body_medium
                    }
                    Item {
                        Layout.fillWidth: true
                    }
                    MD.Text {
                        text: modelData.value
                        color: MD.Token.color.on_surface
                        typescale: MD.Token.typescale.body_medium
                    }
                }
            }

            Flow {
                Layout.fillWidth: true
                Layout.topMargin: 8
                Layout.leftMargin: 4
                spacing: 8
                visible: root.actionList.length > 0

                Repeater {
                    model: root.actionList
                    delegate: MD.Button {
                        required property var modelData
                        text: W.I18n.tr(modelData.labelText)
                        visible: modelData.visible === undefined || modelData.visible
                        enabled: modelData.enabled === undefined || modelData.enabled
                        mdState.type: MD.Enum.BtFilledTonal
                        onClicked: root.runAction(modelData.id)
                    }
                }
            }
        }

        delegate: Column {
            id: settingRow
            required property MD.LazyRow row
            readonly property var modelData: row.value
            readonly property bool startsSection: {
                void (row.revision);
                return row.index === 0 || settingsSource.valueAt(row.index - 1)?.group !== modelData.group;
            }

            MD.Text {
                visible: settingRow.startsSection
                width: parent.width
                text: settingRow.modelData.group
                typescale: MD.Token.typescale.title_small
                color: MD.Token.color.on_surface_variant
                topPadding: 16
                bottomPadding: 6
                leftPadding: 4
            }

            Rectangle {
                id: itemRect
                readonly property var modelData: settingRow.modelData
                width: parent.width
                implicitHeight: fieldCol.implicitHeight + 16
                color: MD.Token.color.surface_container

                readonly property real radiusBig: 16
                readonly property bool roundTop: modelData.position === "single" || modelData.position === "first"
                readonly property bool roundBottom: modelData.position === "single" || modelData.position === "last"

                topLeftRadius: roundTop ? radiusBig : 0
                topRightRadius: roundTop ? radiusBig : 0
                bottomLeftRadius: roundBottom ? radiusBig : 0
                bottomRightRadius: roundBottom ? radiusBig : 0

                ColumnLayout {
                    id: fieldCol
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.leftMargin: 16
                    anchors.rightMargin: 16

                    W.SettingField {
                        Layout.fillWidth: true
                        schema: itemRect.modelData.schema
                        value: root.valueFor(itemRect.modelData.schema.key)
                        onCommitted: function (key, newValue) {
                            const next = Object.assign({}, root.pendingValues);
                            next[key] = newValue;
                            root.pendingValues = next;
                        }
                    }
                }
            }
        }
    }
}
