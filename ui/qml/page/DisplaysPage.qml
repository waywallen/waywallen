pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root

    title: 'Displays'
    showHeader: MD.MProp.size.isCompact
    showBackground: false
    readonly property real displayGapPx: 80

    property var selectedId: null
    property bool paneAnimationsEnabled: false
    readonly property bool detailsVisible: !!root.selected
    readonly property real paneSpacing: 24
    readonly property real paneAvailableHeight: Math.max(0, height - paneSpacing - (detailsVisible ? paneSpacing / 2 : 0))
    readonly property real displayPaneHeight: detailsVisible ? paneAvailableHeight / 3 : paneAvailableHeight
    readonly property real detailPaneHeight: detailsVisible ? paneAvailableHeight - displayPaneHeight : 0

    W.DisplayRenameQuery {
        id: renameQuery
    }

    function layoutRects() {
        const out = [];
        let x = 0;
        for (const d of W.App.displayManager.displays || []) {
            out.push({
                x: x,
                y: 0,
                w: d.width,
                h: d.height,
                d: d
            });
            x += d.width + root.displayGapPx;
        }
        return out;
    }

    readonly property var rects: layoutRects()

    readonly property real boundsW: {
        let max = 0;
        for (const r of rects)
            max = Math.max(max, r.x + r.w);
        return max || 1;
    }
    readonly property real boundsH: {
        let max = 0;
        for (const r of rects)
            max = Math.max(max, r.y + r.h);
        return max || 1;
    }

    function selectedDisplay() {
        if (root.selectedId === null)
            return null;
        for (const d of W.App.displayManager.displays || []) {
            if (d.id === root.selectedId)
                return d;
        }
        return null;
    }

    readonly property var selected: selectedDisplay()

    Item {
        anchors.fill: parent
        anchors.leftMargin: 12
        anchors.rightMargin: 12

        Timer {
            interval: 0
            running: true
            repeat: false
            onTriggered: root.paneAnimationsEnabled = true
        }

        MD.Pane {
            id: displaysPane
            x: 0
            y: root.paneSpacing / 2
            width: parent.width
            height: root.displayPaneHeight
            horizontalPadding: 24
            verticalPadding: 16
            radius: 16
            backgroundColor: MD.MProp.color.surface

            Behavior on height {
                enabled: root.paneAnimationsEnabled

                NumberAnimation {
                    duration: 200
                    easing.type: Easing.InOutCubic
                }
            }

            contentItem: Item {
                id: canvas
                implicitHeight: 48

                readonly property real viewScale: {
                    const availW = Math.max(1, width);
                    const availH = Math.max(1, height);
                    return Math.min(availW / root.boundsW, availH / root.boundsH);
                }
                readonly property real offsetX: (width - root.boundsW * viewScale) / 2
                readonly property real offsetY: (height - root.boundsH * viewScale) / 2

                MouseArea {
                    anchors.fill: parent
                    onClicked: root.selectedId = null
                }

                ColumnLayout {
                    anchors.centerIn: parent
                    width: Math.min(parent.width - 64, 480)
                    spacing: 12
                    visible: (root.rects.length === 0)

                    MD.Text {
                        Layout.alignment: Qt.AlignHCenter
                        text: qsTr("No displays registered")
                        typescale: MD.Token.typescale.title_medium
                        color: MD.Token.color.on_surface_variant
                    }

                    // Desktop-specific install hints are self-gated on
                    // `W.Util.desktop`, so this section stays empty when
                    // the daemon can spawn its own display backend.
                    W.KdeDisplaysHelp {
                        Layout.fillWidth: true
                    }

                    W.GnomeDisplaysHelp {
                        Layout.fillWidth: true
                    }

                    W.LayerShellDisplaysHelp {
                        Layout.fillWidth: true
                    }
                }

                Repeater {
                    model: root.rects

                    delegate: Item {
                        id: rectItem
                        required property int index
                        required property var modelData

                        readonly property var d: modelData.d
                        readonly property bool hasLink: (d.links && d.links.length > 0)
                        readonly property bool isSelected: (root.selectedId === d.id)

                        x: canvas.offsetX + modelData.x * canvas.viewScale
                        y: canvas.offsetY + modelData.y * canvas.viewScale
                        width: modelData.w * canvas.viewScale
                        height: modelData.h * canvas.viewScale

                        Shape {
                            anchors.fill: parent
                            preferredRendererType: Shape.CurveRenderer
                            antialiasing: true

                            ShapePath {
                                strokeColor: rectItem.isSelected ? MD.Token.color.primary : MD.Token.color.outline
                                strokeWidth: rectItem.isSelected ? 3 : 1.5
                                fillColor: rectItem.hasLink ? MD.Token.color.primary_container : MD.Token.color.surface_container_highest
                                capStyle: ShapePath.RoundCap
                                joinStyle: ShapePath.RoundJoin

                                PathRectangle {
                                    x: 0
                                    y: 0
                                    width: rectItem.width
                                    height: rectItem.height
                                    radius: 10
                                }
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.selectedId = rectItem.isSelected ? null : rectItem.d.id
                        }

                        ColumnLayout {
                            anchors.centerIn: parent
                            width: Math.max(0, rectItem.width - 12)
                            spacing: 4

                            MD.Text {
                                Layout.fillWidth: true
                                text: rectItem.d.displayLabel || rectItem.d.name || ("Display " + rectItem.d.id)
                                typescale: MD.Token.typescale.title_small
                                color: rectItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface
                                horizontalAlignment: Text.AlignHCenter
                                elide: Text.ElideMiddle
                            }

                            MD.Text {
                                Layout.alignment: Qt.AlignHCenter
                                text: rectItem.d.width + " × " + rectItem.d.height
                                typescale: MD.Token.typescale.label_medium
                                color: rectItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface_variant
                            }
                        }

                        MD.Text {
                            anchors.left: parent.left
                            anchors.top: parent.top
                            anchors.margins: 6
                            text: "#" + rectItem.d.id
                            typescale: MD.Token.typescale.label_small
                            color: rectItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface_variant
                        }

                        W.GpuTag {
                            anchors.right: parent.right
                            anchors.top: parent.top
                            anchors.margins: 6
                            drmRenderMajor: rectItem.d.drmRenderMajor || 0
                            drmRenderMinor: rectItem.d.drmRenderMinor || 0
                        }
                    }
                }
            }
        }

        // --- Details panel ---
        MD.Pane {
            id: detailsPane
            anchors.top: displaysPane.bottom
            anchors.topMargin: root.paneSpacing
            width: parent.width
            height: root.detailPaneHeight
            visible: root.detailsVisible || height > 0.5

            leftPadding: 16
            rightPadding: 16

            radius: 16
            corners: MD.Util.corners(radius, radius, 0, 0)
            backgroundColor: MD.MProp.color.surface
            clip: true

            Behavior on height {
                enabled: root.paneAnimationsEnabled

                NumberAnimation {
                    duration: 200
                    easing.type: Easing.InOutCubic
                }
            }

            contentItem: ColumnLayout {
                spacing: 0

                MD.Flickable2 {
                id: detailsFlick
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                contentWidth: width
                contentHeight: root.selected ? detailsContent.implicitHeight : 0
                flickableDirection: MD.Flickable2.VerticalFlick
                interactive: contentHeight > height

                ColumnLayout {
                    id: detailsContent
                    width: detailsFlick.contentWidth
                    spacing: 8
                    visible: !!root.selected

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 8

                        readonly property bool canRename: W.Util.supportsDisplayRename

                        MD.TextField {
                            id: aliasField
                            Layout.fillWidth: true
                            visible: parent.canRename
                            placeholderText: root.selected ? (root.selected.name || ("Display " + root.selected.id)) : ""
                            readonly property string serverAlias: root.selected ? (root.selected.alias || "") : ""
                            onServerAliasChanged: if (!activeFocus)
                                text = serverAlias
                            Component.onCompleted: text = serverAlias
                            Connections {
                                target: root
                                function onSelectedIdChanged() {
                                    aliasField.text = aliasField.serverAlias;
                                }
                            }
                            function commit() {
                                if (!root.selected)
                                    return;
                                const trimmed = text.trim();
                                if (trimmed === serverAlias)
                                    return;
                                renameQuery.name = root.selected.name;
                                renameQuery.displayId = root.selected.id;
                                renameQuery.alias = trimmed;
                                renameQuery.clear = (trimmed.length === 0);
                                renameQuery.reload();
                            }
                            onAccepted: commit()
                            onActiveFocusChanged: if (!activeFocus)
                                commit()
                        }

                        MD.Text {
                            Layout.fillWidth: true
                            visible: !parent.canRename
                            text: root.selected ? (root.selected.displayLabel || root.selected.name || ("Display " + root.selected.id)) : ""
                            typescale: MD.Token.typescale.title_medium
                            color: MD.Token.color.on_surface
                            elide: Text.ElideRight
                        }

                        MD.IconButton {
                            visible: parent.canRename && !!root.selected && (root.selected.alias || "").length > 0
                            icon.name: MD.Token.icon.refresh
                            MD.ToolTip {
                                visible: parent.hovered
                                text: "Reset to compositor name"
                            }
                            onClicked: {
                                if (!root.selected)
                                    return;
                                renameQuery.name = root.selected.name;
                                renameQuery.displayId = root.selected.id;
                                renameQuery.alias = "";
                                renameQuery.clear = true;
                                renameQuery.reload();
                                aliasField.text = "";
                            }
                        }

                        MD.IconButton {
                            icon.name: MD.Token.icon.close
                            onClicked: root.selectedId = null
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        spacing: 24

                        RowLayout {
                            spacing: 8
                            MD.Text {
                                text: "ID:"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }
                            MD.Text {
                                text: root.selected ? "#" + root.selected.id : ""
                                typescale: MD.Token.typescale.body_medium
                                color: MD.Token.color.on_surface
                            }
                        }

                        RowLayout {
                            spacing: 8
                            MD.Text {
                                text: "Size:"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }
                            MD.Text {
                                text: root.selected ? root.selected.width + " × " + root.selected.height : ""
                                typescale: MD.Token.typescale.body_medium
                                color: MD.Token.color.on_surface
                            }
                        }

                        RowLayout {
                            visible: !!root.selected && root.selected.refreshMhz > 0
                            spacing: 8
                            MD.Text {
                                text: "Refresh:"
                                typescale: MD.Token.typescale.label_medium
                                color: MD.Token.color.on_surface_variant
                            }
                            MD.Text {
                                text: root.selected ? (root.selected.refreshMhz / 1000).toFixed(3) + " Hz" : ""
                                typescale: MD.Token.typescale.body_medium
                                color: MD.Token.color.on_surface
                            }
                        }

                        Item {
                            Layout.fillWidth: true
                        }
                    }

                    MD.Divider {
                        Layout.fillWidth: true
                        Layout.topMargin: 4
                        Layout.bottomMargin: 4
                    }

                    MD.Text {
                        text: "Connected"
                        typescale: MD.Token.typescale.title_small
                        color: MD.Token.color.on_surface
                    }

                    RowLayout {
                        id: connectedRow
                        readonly property string connectedId: {
                            if (!root.selected)
                                return "";
                            const links = root.selected.links || [];
                            return links.length > 0 ? (links[0].rendererId || "") : "";
                        }
                        // Re-resolve when the manager's renderer list changes
                        // (the `renderers` access wires up the dependency) so a
                        // late RendererUpsert or a RendererRemoved is reflected
                        // without manual refresh.
                        readonly property var renderer: {
                            const _ = W.App.rendererManager.renderers;
                            return connectedId.length > 0 ? W.App.rendererManager.get(connectedId) : null;
                        }
                        readonly property int activePlaylistId: root.selected ? Number(root.selected.activePlaylistId || 0) : 0
                        readonly property var playlistStatus: root.selected ? (root.selected.playlistStatus || ({})) : ({})
                        readonly property bool hasPlaylist: activePlaylistId > 0
                        readonly property string playlistDetail: {
                            const status = playlistStatus || ({});
                            const parts = [];
                            const count = Number(status.count || 0);
                            const position = Number(status.position || 0);
                            const remaining = Number(status.remainingSecs || 0);
                            if (count > 0)
                                parts.push(Math.min(position + 1, count) + " / " + count);
                            if (remaining > 0)
                                parts.push(Math.ceil(remaining / 60) + " min left");
                            return parts.join(" · ");
                        }
                        Layout.fillWidth: true
                        spacing: 16

                        RowLayout {
                            Layout.fillWidth: true
                            Layout.minimumWidth: 0
                            spacing: 8

                            MD.Icon {
                                readonly property string status: connectedRow.renderer ? connectedRow.renderer.status : ""
                                name: {
                                    if (!connectedRow.renderer)
                                        return MD.Token.icon.pause;
                                    return status === "paused" ? MD.Token.icon.pause : MD.Token.icon.play_arrow;
                                }
                                size: 24
                                color: !connectedRow.renderer || status === "paused" ? MD.Token.color.on_surface_variant : MD.Token.color.primary
                            }

                            ColumnLayout {
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                spacing: 0

                                MD.Text {
                                    Layout.fillWidth: true
                                    text: {
                                        const r = connectedRow.renderer;
                                        if (r) {
                                            const name = (r.name && r.name.length) ? r.name : "renderer";
                                            return r.pid > 0 ? (name + "-" + r.pid) : name;
                                        }
                                        if (connectedRow.connectedId.length > 0) {
                                            return connectedRow.connectedId;
                                        }
                                        return "Idle";
                                    }
                                    typescale: MD.Token.typescale.body_medium
                                    color: connectedRow.renderer ? MD.Token.color.on_surface : MD.Token.color.on_surface_variant
                                    font.family: connectedRow.renderer ? "monospace" : ""
                                    elide: Text.ElideMiddle
                                }

                                MD.Text {
                                    Layout.fillWidth: true
                                    visible: !!connectedRow.renderer
                                    text: {
                                        const r = connectedRow.renderer;
                                        if (!r)
                                            return "";
                                        const parts = [(r.status || ""), (r.fps || 0) + " fps"];
                                        const textureWidth = Number(r.textureWidth || 0);
                                        const textureHeight = Number(r.textureHeight || 0);
                                        if (textureWidth > 0 && textureHeight > 0)
                                            parts.push(textureWidth + " × " + textureHeight);
                                        return parts.join(" · ");
                                    }
                                    typescale: MD.Token.typescale.label_small
                                    color: MD.Token.color.on_surface_variant
                                    elide: Text.ElideRight
                                }
                            }
                        }

                        RowLayout {
                            visible: connectedRow.hasPlaylist
                            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                            Layout.maximumWidth: Math.max(220, connectedRow.width * 0.4)
                            spacing: 8

                            MD.Icon {
                                name: MD.Token.icon.playlist_play
                                size: 24
                                color: MD.Token.color.primary
                            }

                            ColumnLayout {
                                Layout.fillWidth: true
                                Layout.minimumWidth: 0
                                spacing: 0

                                MD.Text {
                                    Layout.fillWidth: true
                                    text: "Playlist #" + connectedRow.activePlaylistId
                                    typescale: MD.Token.typescale.body_medium
                                    color: MD.Token.color.on_surface
                                    elide: Text.ElideRight
                                }

                                MD.Text {
                                    Layout.fillWidth: true
                                    visible: connectedRow.playlistDetail.length > 0
                                    text: connectedRow.playlistDetail
                                    typescale: MD.Token.typescale.label_small
                                    color: MD.Token.color.on_surface_variant
                                    elide: Text.ElideRight
                                }
                            }
                        }
                    }

                    // ---- Layout (staged; applied per location on demand) ----
                    MD.Divider {
                        Layout.fillWidth: true
                        Layout.topMargin: 8
                        Layout.bottomMargin: 4
                        visible: !!root.selected
                    }

                    MD.Text {
                        visible: !!root.selected
                        text: "Layout"
                        typescale: MD.Token.typescale.title_small
                        color: MD.Token.color.on_surface
                    }

                    W.DisplayLayoutSection {
                        id: desktopLayout
                        Layout.fillWidth: true
                        visible: !!root.selected
                        display: root.selected
                        location: 0
                        title: "Desktop"
                    }

                    MD.Divider {
                        Layout.fillWidth: true
                        Layout.topMargin: 8
                        Layout.bottomMargin: 4
                        visible: lockLayout.visible
                    }

                    W.DisplayLayoutSection {
                        id: lockLayout
                        Layout.fillWidth: true
                        visible: !!root.selected && root.selected.hasLockScreen === true
                        display: root.selected
                        location: 1
                        title: "Lock screen"
                    }

                }
                }

                // Pinned action bar so Apply stays reachable when the sections scroll.
                RowLayout {
                    id: layoutActions
                    readonly property bool anyDirty: desktopLayout.dirty || (lockLayout.visible && lockLayout.dirty)
                    Layout.fillWidth: true
                    Layout.topMargin: 8
                    Layout.bottomMargin: 8
                    visible: !!root.selected
                    spacing: 8

                    Item {
                        Layout.fillWidth: true
                    }

                    MD.Button {
                        mdState.type: MD.Enum.BtText
                        text: "Revert"
                        enabled: layoutActions.anyDirty
                        onClicked: {
                            desktopLayout.discard();
                            if (lockLayout.visible)
                                lockLayout.discard();
                        }
                    }

                    MD.Button {
                        mdState.type: MD.Enum.BtFilledTonal
                        text: "Apply"
                        enabled: layoutActions.anyDirty
                        onClicked: {
                            if (desktopLayout.dirty)
                                desktopLayout.apply();
                            if (lockLayout.visible && lockLayout.dirty)
                                lockLayout.apply();
                        }
                    }
                }
            }
        }
    }
}
