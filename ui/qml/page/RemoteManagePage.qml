pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root
    title: qsTr("Manage %1").arg(W.I18n.tr(displayNameText) || displayName || sourceId)
    showHeader: true
    showBackground: false
    padding: 0
    scrolling: !manageFlick.atYBeginning

    required property string sourceId
    property string displayName: ""
    property var displayNameText: ({})
    property var schemaList: []
    property var actionList: []
    property var statusList: []
    property string avatarUrl: ""
    property var currentValues: ({})
    property var pendingValues: ({})
    property int inFlightWrites: 0

    readonly property bool saving: inFlightWrites > 0

    readonly property bool hasAccountControls: statusList.length > 0 || actionList.length > 0
    readonly property bool hasContent: hasAccountControls || schemaList.length > 0
    readonly property string accountGroup: {
        if (statusList.length > 0 && statusList[0].group)
            return W.I18n.tr(statusList[0].groupLabel) || statusList[0].group;
        if (actionList.length > 0 && actionList[0].group)
            return W.I18n.tr(actionList[0].groupLabel) || actionList[0].group;
        return qsTr("Login");
    }

    readonly property var flatSchemas: {
        const buckets = {};
        for (let i = 0; i < schemaList.length; ++i) {
            const schema = schemaList[i];
            const localizedGroup = schema.group_label || ({});
            const key = schema.group && schema.group.length > 0 ? schema.group : "";
            if (!buckets[key]) {
                buckets[key] = {
                    label: W.I18n.tr(localizedGroup) || schema.group || qsTr("General"),
                    items: []
                };
            }
            buckets[key].items.push(schema);
        }
        const groups = Object.keys(buckets).sort();
        const flattened = [];
        for (let i = 0; i < groups.length; ++i) {
            const group = groups[i];
            const bucket = buckets[group];
            const items = bucket.items;
            items.sort((a, b) => (a.order || 0) - (b.order || 0));
            for (let j = 0; j < items.length; ++j) {
                flattened.push({
                    group: bucket.label,
                    schema: items[j],
                    first: j === 0,
                    last: j === items.length - 1
                });
            }
        }
        return flattened;
    }

    function sourceFromAvailability() {
        const sources = availabilityQuery.sources || [];
        for (let i = 0; i < sources.length; ++i) {
            if (sources[i].id === sourceId)
                return sources[i];
        }
        return null;
    }

    function refreshAvailability() {
        const source = sourceFromAvailability();
        if (!source)
            return;
        displayName = source.displayName || source.name || sourceId;
        displayNameText = source.displayNameText || ({});
        schemaList = source.settings || [];
        actionList = source.actions || [];
        statusList = source.status || [];
        avatarUrl = source.avatarUrl || "";
    }

    function valueFor(schema) {
        if (schema.key in currentValues)
            return String(currentValues[schema.key]);
        return String(schema.default_value || "");
    }

    function mergeVisibleValues(serverValues) {
        const merged = Object.assign({}, serverValues || {});
        for (const key in pendingValues)
            merged[key] = pendingValues[key];
        currentValues = merged;
    }

    function commitValue(key, value) {
        const visible = Object.assign({}, currentValues);
        visible[key] = value;
        currentValues = visible;
        const pending = Object.assign({}, pendingValues);
        pending[key] = value;
        pendingValues = pending;
        saveTimer.restart();
    }

    function flushPending() {
        if (Object.keys(pendingValues).length === 0)
            return;
        const batch = pendingValues;
        pendingValues = ({});
        inFlightWrites += 1;
        patchQuery.patch(sourceId, batch);
    }

    function prepareClose() {
        saveTimer.stop();
        flushPending();
    }

    function runAction(action) {
        if (Number(action.kind) === 3) {
            actionForm.openFor(action);
            return;
        }
        actionQuery.pluginId = sourceId;
        actionQuery.actionId = action.id;
        actionQuery.reload();
    }

    function submitAction(action, values) {
        actionQuery.pluginId = sourceId;
        actionQuery.actionId = action.id;
        actionQuery.invoke(values);
    }

    W.PluginActionFormDialog {
        id: actionForm
        onSubmitted: function (values) {
            root.submitAction(action, values);
        }
    }

    W.RemoteAvailabilityQuery {
        id: availabilityQuery
        onSourcesChanged: root.refreshAvailability()
    }

    W.SettingsGetQuery {
        id: settingsQuery
        onPluginsChanged: root.mergeVisibleValues(plugins[root.sourceId] || ({}))
    }

    W.RemoteSettingsPatchQuery {
        id: patchQuery
        forwardError: false
        onCompleted: function (sourceId, values, accepted, error) {
            if (sourceId !== root.sourceId)
                return;
            root.inFlightWrites = Math.max(0, root.inFlightWrites - 1);
            if (!accepted) {
                W.Global.toastError(error.length > 0 ? error : qsTr("Couldn't save remote settings"));
            }
            if (Object.keys(root.pendingValues).length > 0)
                saveTimer.restart();
            else if (root.inFlightWrites === 0) {
                settingsQuery.reload();
            }
        }
    }

    W.PluginActionQuery {
        id: actionQuery
        forwardError: false
        onCompleted: function (accepted, error, sessionId) {
            if (!accepted) {
                W.Global.toastError(error.length > 0 ? error : qsTr("Login action failed"));
            } else {
                availabilityQuery.reload();
            }
        }
    }

    Timer {
        id: saveTimer
        interval: 250
        repeat: false
        onTriggered: root.flushPending()
    }

    Connections {
        target: W.Notify
        function onDaemonReady() {
            availabilityQuery.reload();
            settingsQuery.reload();
        }
        function onSettingsChanged() {
            availabilityQuery.reload();
            if (!root.saving && Object.keys(root.pendingValues).length === 0)
                settingsQuery.reload();
        }
        function onPluginStateChanged() {
            availabilityQuery.reload();
        }
    }

    Component.onCompleted: {
        if (W.Notify.daemonPhase === W.Notify.DaemonPhase.Ready) {
            availabilityQuery.reload();
            settingsQuery.reload();
        }
    }

    contentItem: MD.VerticalFlickable {
        MD.NestedScroll.smoothWheelEnabled: W.Global.smoothScrolling
        id: manageFlick
        topMargin: 4
        bottomMargin: 24
        leftMargin: 16
        rightMargin: 16

        Item {
            width: manageFlick.contentWidth
            implicitHeight: manageColumn.implicitHeight

            ColumnLayout {
                id: manageColumn
                anchors.horizontalCenter: parent.horizontalCenter
                width: Math.min(parent.width, 720)
                spacing: 2

                MD.Text {
                    Layout.fillWidth: true
                    visible: root.hasAccountControls
                    text: root.accountGroup
                    typescale: MD.Token.typescale.title_small
                    color: MD.Token.color.on_surface_variant
                    topPadding: 16
                    bottomPadding: 6
                    leftPadding: 4
                }

                Rectangle {
                    Layout.fillWidth: true
                    visible: root.hasAccountControls
                    implicitHeight: accountColumn.implicitHeight + 24
                    color: MD.Token.color.surface_container
                    radius: 16

                    ColumnLayout {
                        id: accountColumn
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: 16
                        anchors.rightMargin: 16
                        spacing: 10

                        Repeater {
                            model: root.statusList
                            delegate: RowLayout {
                                id: statusRow
                                required property var modelData
                                required property int index
                                Layout.fillWidth: true
                                spacing: 12

                                MD.Text {
                                    text: W.I18n.tr(statusRow.modelData.labelText)
                                    color: MD.Token.color.on_surface_variant
                                    typescale: MD.Token.typescale.body_medium
                                }
                                Item {
                                    Layout.fillWidth: true
                                }
                                W.ThumbnailImage {
                                    Layout.preferredWidth: 32
                                    Layout.preferredHeight: 32
                                    Layout.maximumWidth: 32
                                    Layout.maximumHeight: 32
                                    visible: statusRow.index === 0 && root.avatarUrl.length > 0
                                    source: root.avatarUrl
                                    maxSize: Qt.size(64, 64)
                                    fillMode: Image.PreserveAspectCrop
                                    radius: 16
                                }
                                MD.Text {
                                    Layout.maximumWidth: accountColumn.width * 0.65
                                    text: W.I18n.tr(statusRow.modelData.valueText)
                                    color: MD.Token.color.on_surface
                                    typescale: MD.Token.typescale.body_medium
                                    horizontalAlignment: Text.AlignRight
                                    wrapMode: Text.Wrap
                                }
                            }
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 8
                            visible: root.actionList.length > 0

                            Repeater {
                                model: root.actionList
                                delegate: ColumnLayout {
                                    id: actionItem
                                    required property var modelData
                                    Layout.fillWidth: true
                                    visible: actionItem.modelData.visible === undefined || actionItem.modelData.visible
                                    spacing: 6

                                    MD.Text {
                                        Layout.fillWidth: true
                                        visible: text.length > 0
                                        text: W.I18n.tr(actionItem.modelData.descriptionText)
                                        typescale: MD.Token.typescale.body_medium
                                        color: MD.Token.color.on_surface_variant
                                        wrapMode: Text.WordWrap
                                    }

                                    MD.Button {
                                        text: W.I18n.tr(actionItem.modelData.labelText)
                                        enabled: !actionQuery.querying && (actionItem.modelData.enabled === undefined || actionItem.modelData.enabled)
                                        mdState.type: MD.Enum.BtFilledTonal
                                        onClicked: root.runAction(actionItem.modelData)
                                    }
                                }
                            }
                        }
                    }
                }

                Repeater {
                    model: root.flatSchemas
                    delegate: ColumnLayout {
                        id: settingGroupItem
                        required property var modelData
                        Layout.fillWidth: true
                        spacing: 2

                        MD.Text {
                            Layout.fillWidth: true
                            visible: settingGroupItem.modelData.first
                            text: settingGroupItem.modelData.group
                            typescale: MD.Token.typescale.title_small
                            color: MD.Token.color.on_surface_variant
                            topPadding: 16
                            bottomPadding: 6
                            leftPadding: 4
                        }

                        Rectangle {
                            Layout.fillWidth: true
                            implicitHeight: settingField.implicitHeight + 16
                            color: MD.Token.color.surface_container

                            readonly property real radiusBig: 16
                            topLeftRadius: settingGroupItem.modelData.first ? radiusBig : 0
                            topRightRadius: settingGroupItem.modelData.first ? radiusBig : 0
                            bottomLeftRadius: settingGroupItem.modelData.last ? radiusBig : 0
                            bottomRightRadius: settingGroupItem.modelData.last ? radiusBig : 0

                            W.SettingField {
                                id: settingField
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.verticalCenter: parent.verticalCenter
                                anchors.leftMargin: 16
                                anchors.rightMargin: 16
                                schema: settingGroupItem.modelData.schema
                                value: root.valueFor(settingGroupItem.modelData.schema)
                                onCommitted: function (key, newValue) {
                                    root.commitValue(key, newValue);
                                }
                            }
                        }
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    Layout.topMargin: 8
                    visible: root.saving || Object.keys(root.pendingValues).length > 0
                    Item {
                        Layout.fillWidth: true
                    }
                    MD.BusyIndicator {
                        implicitWidth: 18
                        implicitHeight: 18
                        running: visible
                    }
                    MD.Text {
                        text: qsTr("Saving…")
                        typescale: MD.Token.typescale.body_small
                        color: MD.Token.color.on_surface_variant
                    }
                }

                MD.Text {
                    Layout.fillWidth: true
                    Layout.topMargin: 48
                    visible: !root.hasContent && !availabilityQuery.querying
                    text: qsTr("This remote has no login or settings to manage.")
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    typescale: MD.Token.typescale.body_medium
                    color: MD.Token.color.on_surface_variant
                }
            }
        }
    }
}
