pragma ComponentBehavior: Bound
pragma ValueTypeBehavior: Assertable
import QtQuick
import QtQuick.Layouts
import QtQuick.Shapes
import Qcm.Material as MD
import waywallen.ui as W

MD.Page {
    id: root

    title: qsTr('Displays')
    showHeader: MD.MProp.size.isCompact
    showBackground: false
    readonly property real displayGapPx: 80
    readonly property real canvasHeaderPx: 36
    readonly property real canvasPaddingPx: 8
    readonly property real canvasSnapPx: 12

    property var selection: null
    readonly property string selectedKind: selection?.kind || ""
    readonly property var selectedId: selection?.id ?? null
    property string pendingCanvasId: ""
    readonly property bool detailsVisible: !!root.selected
    property bool detailsExpanded: false
    onDetailsVisibleChanged: {
        if (!detailsVisible)
            detailsExpanded = false;
    }

    // FillMode/Rotation enum values mirror proto::FillMode /
    // proto::Rotation (control.proto). Keep the *_VALUES
    // arrays in lockstep with the enum order; *_LABELS is what the UI
    // shows.
    readonly property var kFillModeValues: [1 // STRETCHED
        , 2 // PRESERVE_ASPECT_FIT
        , 3 // PRESERVE_ASPECT_CROP
        , 7  // CENTERED
    ]
    readonly property var kFillModeLabels: [qsTr("Stretch"), qsTr("Fit"), qsTr("Crop"), qsTr("Center")]
    function fillmodeIndex(value) {
        const i = root.kFillModeValues.indexOf(value);
        return i < 0 ? 0 : i;
    }

    // Rotation segmented values, mirror proto::Rotation:
    //   1=NORMAL, 2=CW_90, 3=CW_180, 4=CW_270
    readonly property var kRotationValues: [1, 2, 3, 4]
    readonly property var kRotationLabels: ["0°", "90°", "180°", "270°"]
    function rotationIndex(value) {
        const i = root.kRotationValues.indexOf(value);
        return i < 0 ? 0 : i;
    }

    function clampPercent(value) {
        return Math.max(0, Math.min(100, Math.round(Number(value) || 0)));
    }

    function prepareCanvasLayoutUpdate() {
        canvasLayoutSetQuery.canvasId = root.selectedCanvasObject.id;
        canvasLayoutSetQuery.fillmodeSet = false;
        canvasLayoutSetQuery.locationSet = false;
        canvasLayoutSetQuery.rotationSet = false;
        canvasLayoutSetQuery.flipSet = false;
        canvasLayoutSetQuery.clearFlip = false;
        canvasLayoutSetQuery.clearFillmode = false;
        canvasLayoutSetQuery.clearLocation = false;
        canvasLayoutSetQuery.clearRotation = false;
    }

    function applyLocation(x, y) {
        if (!root.selected)
            return;
        if (root.selectedKind === "canvas") {
            root.prepareCanvasLayoutUpdate();
            canvasLayoutSetQuery.locationSet = true;
            canvasLayoutSetQuery.locationX = root.clampPercent(x);
            canvasLayoutSetQuery.locationY = root.clampPercent(y);
            canvasLayoutSetQuery.reload();
            return;
        }
        layoutSetQuery.name = root.selected.name;
        layoutSetQuery.displayId = root.selected.id;
        layoutSetQuery.fillmodeSet = false;
        layoutSetQuery.locationSet = true;
        layoutSetQuery.locationX = root.clampPercent(x);
        layoutSetQuery.locationY = root.clampPercent(y);
        layoutSetQuery.alignSet = false;
        layoutSetQuery.rotationSet = false;
        layoutSetQuery.clearFillmode = false;
        layoutSetQuery.clearLocation = false;
        layoutSetQuery.clearAlign = false;
        layoutSetQuery.clearRotation = false;
        layoutSetQuery.flipSet = false;
        layoutSetQuery.clearFlip = false;
        layoutSetQuery.reload();
    }

    function applyFillmode(value) {
        if (!root.selected)
            return;
        if (root.selectedKind === "canvas") {
            root.prepareCanvasLayoutUpdate();
            canvasLayoutSetQuery.fillmodeSet = true;
            canvasLayoutSetQuery.fillmode = value;
            canvasLayoutSetQuery.reload();
            return;
        }
        layoutSetQuery.name = root.selected.name;
        layoutSetQuery.displayId = root.selected.id;
        layoutSetQuery.fillmodeSet = true;
        layoutSetQuery.fillmode = value;
        layoutSetQuery.locationSet = false;
        layoutSetQuery.alignSet = false;
        layoutSetQuery.rotationSet = false;
        layoutSetQuery.clearFillmode = false;
        layoutSetQuery.clearLocation = false;
        layoutSetQuery.clearAlign = false;
        layoutSetQuery.clearRotation = false;
        layoutSetQuery.flipSet = false;
        layoutSetQuery.clearFlip = false;
        layoutSetQuery.reload();
    }

    function applyRotation(value) {
        if (!root.selected)
            return;
        if (root.selectedKind === "canvas") {
            root.prepareCanvasLayoutUpdate();
            canvasLayoutSetQuery.rotationSet = true;
            canvasLayoutSetQuery.rotation = value;
            canvasLayoutSetQuery.reload();
            return;
        }
        layoutSetQuery.name = root.selected.name;
        layoutSetQuery.displayId = root.selected.id;
        layoutSetQuery.fillmodeSet = false;
        layoutSetQuery.locationSet = false;
        layoutSetQuery.alignSet = false;
        layoutSetQuery.rotationSet = true;
        layoutSetQuery.rotation = value;
        layoutSetQuery.clearFillmode = false;
        layoutSetQuery.clearLocation = false;
        layoutSetQuery.clearAlign = false;
        layoutSetQuery.clearRotation = false;
        layoutSetQuery.flipSet = false;
        layoutSetQuery.clearFlip = false;
        layoutSetQuery.reload();
    }

    function applyFlip(value) {
        if (!root.selected)
            return;
        if (root.selectedKind === "canvas") {
            root.prepareCanvasLayoutUpdate();
            canvasLayoutSetQuery.flipSet = true;
            canvasLayoutSetQuery.flip = value;
            canvasLayoutSetQuery.reload();
            return;
        }
        layoutSetQuery.name = root.selected.name;
        layoutSetQuery.displayId = root.selected.id;
        layoutSetQuery.fillmodeSet = false;
        layoutSetQuery.locationSet = false;
        layoutSetQuery.alignSet = false;
        layoutSetQuery.rotationSet = false;
        layoutSetQuery.flipSet = true;
        layoutSetQuery.flip = value;
        layoutSetQuery.clearFillmode = false;
        layoutSetQuery.clearLocation = false;
        layoutSetQuery.clearAlign = false;
        layoutSetQuery.clearRotation = false;
        layoutSetQuery.clearFlip = false;
        layoutSetQuery.reload();
    }

    function resetLayout() {
        if (!root.selected)
            return;
        if (root.selectedKind === "canvas") {
            root.prepareCanvasLayoutUpdate();
            canvasLayoutSetQuery.clearFillmode = true;
            canvasLayoutSetQuery.clearLocation = true;
            canvasLayoutSetQuery.clearRotation = true;
            canvasLayoutSetQuery.clearFlip = true;
            canvasLayoutSetQuery.reload();
            return;
        }
        layoutSetQuery.name = root.selected.name;
        layoutSetQuery.displayId = root.selected.id;
        layoutSetQuery.fillmodeSet = false;
        layoutSetQuery.locationSet = false;
        layoutSetQuery.alignSet = false;
        layoutSetQuery.clearFillmode = true;
        layoutSetQuery.clearLocation = true;
        layoutSetQuery.clearAlign = true;
        layoutSetQuery.clearRotation = true;
        layoutSetQuery.rotationSet = false;
        layoutSetQuery.flipSet = false;
        layoutSetQuery.clearFlip = true;
        layoutSetQuery.reload();
    }

    W.DisplayLayoutSetQuery {
        id: layoutSetQuery
    }

    W.CanvasLayoutSetQuery {
        id: canvasLayoutSetQuery
    }

    W.DisplayRenameQuery {
        id: renameQuery
    }

    W.DisplayPauseSetQuery {
        id: displayPauseQuery
    }

    W.CanvasMutationQuery {
        id: canvasMutationQuery
        forwardError: false
        onCanvasCreated: canvasId => {
            root.pendingCanvasId = canvasId;
            root.openPendingCanvas();
        }
        onCanvasUpdated: function (canvasId, revision) {
            if (root.selectedKind === "canvas" && root.selectedId === canvasId)
                canvasEditor.accept(revision);
        }
        onCanvasDeleted: canvasId => {
            if (root.selectedKind !== "canvas" || root.selectedId !== canvasId)
                return;
            canvasEditor.clear();
            root.selection = null;
        }
    }

    W.CanvasEditorState {
        id: canvasEditor
        displays: W.App.displayManager.displays
    }

    Connections {
        target: W.App.displayManager
        function onCanvasesChanged() {
            root.openPendingCanvas();
            if (root.selectedKind !== "canvas")
                return;
            const selected = W.App.displayManager.getCanvas(String(root.selectedId));
            if (!selected) {
                canvasDialog.close();
                canvasEditor.clear();
                root.selection = null;
            } else if (!canvasEditor.dirty) {
                canvasEditor.begin(selected);
            } else {
                canvasEditor.refreshMemberConfig(selected);
            }
        }
    }

    Connections {
        target: canvasMutationQuery
        function onStatusChanged() {
            if (canvasMutationQuery.status === 3)
                W.Global.toastError(canvasMutationQuery.error || qsTr("Canvas update failed"));
        }
    }

    W.DisplayEditDialog {
        id: displayEditDialog
        onSubmitted: function (name, targetId, alias, clear) {
            renameQuery.name = name;
            renameQuery.displayId = targetId;
            renameQuery.alias = alias;
            renameQuery.clear = clear;
            renameQuery.reload();
        }
    }

    W.CanvasDialog {
        id: canvasDialog
        onSubmitted: function (name) {
            if (!root.selectedCanvasObject)
                return;
            canvasEditor.setName(name);
            root.applyCanvasDraft();
        }
    }

    MD.Action {
        id: resetCanvasAlignmentAction
        text: qsTr("Reset")
        icon.name: MD.Token.icon.restart_alt
        enabled: canvasEditor.dirty && !canvasMutationQuery.querying
        onTriggered: canvasEditor.cancel()
    }

    MD.Action {
        id: applyCanvasAlignmentAction
        text: qsTr("Apply")
        icon.name: MD.Token.icon.check
        enabled: canvasEditor.dirty && canvasEditor.name.trim().length > 0 && !canvasMutationQuery.querying
        onTriggered: root.applyCanvasDraft()
    }

    MD.Dialog {
        id: deleteCanvasDialog
        parent: MD.Overlay.overlay
        modal: true
        title: qsTr("Delete canvas?")
        standardButtons: MD.Dialog.Cancel | MD.Dialog.Ok
        contentItem: MD.Text {
            text: qsTr("The canvas layout will be removed. Its displays become independent again.")
            wrapMode: Text.Wrap
            color: MD.Token.color.on_surface_variant
        }
        onAccepted: {
            if (root.selectedCanvasObject) {
                canvasMutationQuery.removeCanvas(root.selectedCanvasObject.id, canvasEditor.baseRevision);
            }
        }
    }

    MD.Dialog {
        id: resetDisplaySettingsDialog
        parent: MD.Overlay.overlay
        modal: true
        title: qsTr("Revert layout settings?")
        standardButtons: MD.Dialog.Cancel | MD.Dialog.Reset
        contentItem: MD.Text {
            text: qsTr("The settings for this layout will be reverted to the global default. Your custom configuration will be lost.")
            wrapMode: Text.Wrap
            color: MD.Token.color.on_surface_variant
        }
        onReset: {
            root.resetLayout()
            resetDisplaySettingsDialog.close()
        }
    }

    function canvasRows(canvasObject) {
        return canvasEditor.rowsForCanvas(canvasObject);
    }

    function defaultCanvasExtent() {
        let smallest = null;
        for (const display of W.App.displayManager.displays || []) {
            const width = Number(display.width || 0);
            const height = Number(display.height || 0);
            if (width <= 0 || height <= 0)
                continue;
            if (!smallest || width * height < smallest.width * smallest.height)
                smallest = {
                    width: width,
                    height: height
                };
        }
        return smallest || {
            width: 1280,
            height: 720
        };
    }

    function layoutTargets() {
        const out = [];
        const assignedKeys = new Set();
        const defaultExtent = root.defaultCanvasExtent();
        let fallbackX = 0;
        for (const canvasObject of W.App.displayManager.canvases || []) {
            const members = root.canvasRows(canvasObject);
            const bounds = canvasEditor.topologyBounds(members);
            const empty = members.length === 0;
            const width = empty ? defaultExtent.width : bounds.width;
            const height = empty ? defaultExtent.height : bounds.height;
            for (const member of members)
                assignedKeys.add(String(member.settingsKey || ""));
            out.push({
                kind: "canvas",
                x: fallbackX,
                y: 0,
                w: width,
                h: height,
                canvasObject: canvasObject,
                members: members,
                memberBounds: bounds
            });
            fallbackX += width + root.displayGapPx;
        }
        for (const d of W.App.displayManager.displays || []) {
            if (assignedKeys.has(String(d.settingsKey || d.name || "")))
                continue;
            const w = Number(d.width || 1);
            const h = Number(d.height || 1);
            out.push({
                kind: "display",
                x: fallbackX,
                y: 0,
                w: w,
                h: h,
                placed: true,
                displayObject: d
            });
            fallbackX += w + root.displayGapPx;
        }
        return out;
    }

    function openPendingCanvas() {
        if (!root.pendingCanvasId.length)
            return;
        const canvas = W.App.displayManager.getCanvas(root.pendingCanvasId);
        if (!canvas)
            return;
        root.pendingCanvasId = "";
        root.selectCanvas(canvas);
    }

    readonly property var targets: layoutTargets()

    function targetLeftDecoration(target) {
        if (target.kind !== "canvas")
            return 0;
        return root.canvasPaddingPx + (target.h > target.w ? root.canvasHeaderPx : 0);
    }

    function targetRightDecoration(target) {
        return target.kind === "canvas" ? root.canvasPaddingPx : 0;
    }

    function targetHorizontalShift(index) {
        let shift = 0;
        for (let i = 0; i <= index; ++i) {
            shift += root.targetLeftDecoration(root.targets[i]);
            if (i < index)
                shift += root.targetRightDecoration(root.targets[i]);
        }
        return shift;
    }

    readonly property real targetHorizontalDecorationWidth: targets.reduce((width, target) => width + targetLeftDecoration(target) + targetRightDecoration(target), 0)

    readonly property real boundsW: {
        if (targets.length === 0)
            return 1;
        let min = targets[0].x;
        let max = targets[0].x + targets[0].w;
        for (const r of targets)
            min = Math.min(min, r.x);
        for (const r of targets)
            max = Math.max(max, r.x + r.w);
        return max - min || 1;
    }
    readonly property real boundsH: {
        if (targets.length === 0)
            return 1;
        let min = targets[0].y;
        let max = targets[0].y + targets[0].h;
        for (const r of targets)
            min = Math.min(min, r.y);
        for (const r of targets)
            max = Math.max(max, r.y + r.h);
        return max - min || 1;
    }
    readonly property real boundsX: targets.length > 0 ? targets.reduce((value, item) => Math.min(value, item.x), targets[0].x) : 0
    readonly property real boundsY: targets.length > 0 ? targets.reduce((value, item) => Math.min(value, item.y), targets[0].y) : 0

    function canChangeSelection(kind, id) {
        return !canvasEditor.dirty || (root.selectedKind === kind && root.selectedId === id);
    }

    function selectDisplay(displayObject) {
        if (!displayObject || !root.canChangeSelection("display", displayObject.id))
            return;
        if (root.detailsExpanded && root.selectedKind === "display" && root.selectedId === displayObject.id) {
            root.clearSelection();
            return;
        }
        canvasEditor.clear();
        root.selection = {
            kind: "display",
            id: displayObject.id
        };
        root.detailsExpanded = true;
    }

    function selectCanvas(canvasObject) {
        if (!canvasObject || !root.canChangeSelection("canvas", canvasObject.id))
            return;
        if (root.selectedKind !== "canvas" || root.selectedId !== canvasObject.id) {
            root.selection = {
                kind: "canvas",
                id: canvasObject.id
            };
            canvasEditor.begin(canvasObject);
        }
        root.detailsExpanded = true;
    }

    function clearSelection() {
        if (canvasEditor.dirty)
            return;
        root.detailsExpanded = false;
    }

    function finishClosingDetails() {
        if (root.detailsExpanded || canvasEditor.dirty)
            return;
        canvasEditor.clear();
        root.selection = null;
    }

    function applyCanvasDraft() {
        if (!root.selectedCanvasObject || !canvasEditor.dirty)
            return;
        canvasMutationQuery.updateCanvas(root.selectedCanvasObject.id, canvasEditor.baseRevision, canvasEditor.name.trim(), canvasEditor.members);
    }

    function dropDisplayOnCanvas(canvasObject, displayObject, dropX, dropY, memberBounds) {
        if (!canvasObject || !displayObject || !root.canChangeSelection("canvas", canvasObject.id))
            return false;
        root.selectCanvas(canvasObject);
        if (root.selectedKind !== "canvas" || root.selectedId !== canvasObject.id)
            return false;
        const bounds = memberBounds || ({
                x: 0,
                y: 0
            });
        canvasEditor.addDisplayAtEdge(displayObject, Number(bounds.x || 0) + (dropX - root.canvasPaddingPx) / canvas.viewScale);
        return true;
    }

    function dragPayload(source) {
        return {
            kind: source?.dragKind || "",
            displayObject: source?.displayObject || null
        };
    }

    readonly property var selected: {
        const key = root.selection;
        if (!key)
            return null;
        const objects = key.kind === "canvas" ? W.App.displayManager.canvases : W.App.displayManager.displays;
        return (objects || []).find(object => object.id === key.id) || null;
    }
    readonly property W.Display selectedDisplayObject: selected as W.Display
    readonly property W.Canvas selectedCanvasObject: selected as W.Canvas

    MD.SplitView {
        id: pageContent

        anchors.fill: parent
        anchors.leftMargin: 12
        anchors.rightMargin: 12
        anchors.topMargin: 12
        anchors.bottomMargin: 12
        orientation: width >= MD.Token.window_class.expanded.min_width ? Qt.Horizontal : Qt.Vertical

        expandTransition: Transition {
            SpringAnimation {
                property: "progress"
                spring: 3
                damping: 0.3
                epsilon: 0.001
            }
        }
        collapseTransition: Transition {
            SpringAnimation {
                property: "progress"
                spring: 3
                damping: 0.3
                epsilon: 0.001
            }
        }

        MD.Pane {
            id: displaysPane
            readonly property bool constrainSize: root.detailsExpanded && !detailsPane.MD.SplitViewBase.transitioning
            implicitWidth: 480
            implicitHeight: 320
            MD.SplitViewBase.minimumWidth: root.detailsVisible ? pageContent.availableWidth / 2 : 0
            MD.SplitViewBase.minimumHeight: root.detailsVisible ? pageContent.availableHeight / 2 : 0
            MD.SplitViewBase.preferredWidth: pageContent.availableWidth * 5 / 6
            MD.SplitViewBase.preferredHeight: pageContent.availableHeight * 5 / 9
            MD.SplitViewBase.maximumWidth: constrainSize ? pageContent.availableWidth * 5 / 6 : pageContent.availableWidth
            MD.SplitViewBase.maximumHeight: constrainSize ? pageContent.availableHeight * 5 / 9 : pageContent.availableHeight
            horizontalPadding: 24
            verticalPadding: 16
            radius: 16
            backgroundColor: MD.MProp.color.surface

            contentItem: Item {
                id: canvas
                implicitHeight: 48

                Item {
                    id: canvasActions
                    anchors.right: parent.right
                    anchors.top: parent.top
                    width: createCanvasChip.implicitWidth + refreshDisplaysButton.implicitWidth + 6 + (canvasAlignmentActionToolBar.visible ? canvasAlignmentActionToolBar.implicitWidth + 6 : 0)
                    height: createCanvasChip.implicitHeight
                    z: 100

                    MD.ActionToolBar {
                        id: canvasAlignmentActionToolBar

                        anchors.right: refreshDisplaysButton.left
                        anchors.rightMargin: 6
                        anchors.verticalCenter: parent.verticalCenter
                        visible: canvasEditor.dirty
                        actions: [resetCanvasAlignmentAction, applyCanvasAlignmentAction]
                        iconDelegate: MD.ActionIconButton {
                            id: canvasActionButton
                            mdState.size: MD.Enum.XS

                            readonly property string toolTipText: canvasActionButton.action?.tooltip || canvasActionButton.action?.text || ""

                            action: MD.ToolBarLayout.action
                            hoverEnabled: true
                            MD.ToolTip.text: toolTipText
                            MD.ToolTip.visible: mdState.interactions.hovered && toolTipText.length > 0 && !mdState.interactions.pressed
                        }
                        moreDelegate: MD.ActionIconButton {
                            mdState.size: MD.Enum.XS
                            action: canvasAlignmentActionToolBar.moreAction
                        }
                    }

                    MD.IconButton {
                        id: refreshDisplaysButton
                        mdState.size: MD.Enum.XS

                        anchors.right: createCanvasChip.left
                        anchors.rightMargin: 6
                        anchors.verticalCenter: parent.verticalCenter
                        icon.name: MD.Token.icon.refresh
                        enabled: W.DaemonDBusClient.daemonAvailable
                        hoverEnabled: true
                        MD.ToolTip.text: qsTr("Refresh displays")
                        MD.ToolTip.visible: mdState.interactions.hovered && !mdState.interactions.pressed
                        onClicked: {
                            if (!W.DaemonDBusClient.refreshDisplays())
                                W.Global.toastError(qsTr("Failed to refresh displays"));
                        }
                    }

                    MD.AssistChip {
                        id: createCanvasChip

                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        text: qsTr("Create canvas")
                        icon.name: MD.Token.icon.add
                        enabled: !canvasMutationQuery.querying
                        onClicked: canvasMutationQuery.createCanvas(qsTr("Canvas %1").arg((W.App.displayManager.canvases || []).length + 1), [])
                    }
                }

                readonly property bool hasCanvasTargets: root.targets.some(target => target.kind === "canvas")
                readonly property bool hasHorizontalCanvasTargets: root.targets.some(target => target.kind === "canvas" && target.w >= target.h)
                readonly property real drawingPadding: hasCanvasTargets ? root.canvasPaddingPx : 0
                readonly property real drawingLeft: 0
                readonly property real drawingTop: canvasActions.height + 12 + (hasHorizontalCanvasTargets ? root.canvasHeaderPx : 0) + drawingPadding
                readonly property real drawingWidth: Math.max(1, width - drawingLeft)
                readonly property real drawingHeight: Math.max(1, height - drawingTop - drawingPadding)
                readonly property real viewScale: {
                    const contentWidth = Math.max(1, drawingWidth - root.targetHorizontalDecorationWidth);
                    return Math.min(contentWidth / root.boundsW, drawingHeight / root.boundsH);
                }
                readonly property real offsetX: drawingLeft + (drawingWidth - root.boundsW * viewScale - root.targetHorizontalDecorationWidth) / 2
                readonly property real offsetY: drawingTop + (drawingHeight - root.boundsH * viewScale) / 2

                MouseArea {
                    anchors.fill: parent
                    onClicked: root.clearSelection()
                }

                ColumnLayout {
                    anchors.horizontalCenter: parent.horizontalCenter
                    y: canvas.drawingTop + (canvas.drawingHeight - height) / 2
                    width: Math.min(parent.width - 64, 480)
                    spacing: 12
                    visible: root.targets.length === 0

                    MD.Text {
                        Layout.alignment: Qt.AlignHCenter
                        text: qsTr("No displays registered")
                        typescale: MD.Token.typescale.title_medium
                        color: MD.Token.color.on_surface_variant
                    }

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
                    model: root.targets

                    delegate: Item {
                        id: targetItem
                        required property int index
                        required property var modelData

                        readonly property bool isDisplay: modelData.kind === "display"
                        readonly property bool isCanvas: modelData.kind === "canvas"
                        readonly property var d: modelData.displayObject || null
                        readonly property var canvasObject: modelData.canvasObject || null
                        readonly property var canvasMembers: isCanvas ? (modelData.members || []) : []
                        readonly property var canvasMemberBounds: isCanvas ? (modelData.memberBounds || ({
                                    x: 0,
                                    y: 0,
                                    width: 1,
                                    height: 1
                                })) : ({
                                x: 0,
                                y: 0,
                                width: 1,
                                height: 1
                            })
                        readonly property bool hasLink: isDisplay ? !!d && d.links && d.links.length > 0 : isCanvas && !!canvasObject && canvasObject.links && canvasObject.links.length > 0
                        readonly property bool isSelected: isDisplay ? root.selectedKind === "display" && root.selectedId === d?.id : root.selectedKind === "canvas" && root.selectedId === canvasObject?.id
                        readonly property bool verticalCanvasHeader: isCanvas && modelData.h > modelData.w
                        readonly property real bodyY: canvas.offsetY + (modelData.y - root.boundsY) * canvas.viewScale
                        readonly property real canvasPadding: isCanvas ? root.canvasPaddingPx : 0
                        readonly property real bodyX: canvas.offsetX + (modelData.x - root.boundsX) * canvas.viewScale + root.targetHorizontalShift(index)

                        z: isDisplay ? 2 : 1
                        x: isCanvas ? bodyX - (verticalCanvasHeader ? root.canvasHeaderPx : 0) - canvasPadding : bodyX
                        y: isCanvas ? bodyY - (verticalCanvasHeader ? 0 : root.canvasHeaderPx) - canvasPadding : bodyY
                        width: modelData.w * canvas.viewScale + (verticalCanvasHeader ? root.canvasHeaderPx : 0) + canvasPadding * 2
                        height: modelData.h * canvas.viewScale + (isCanvas && !verticalCanvasHeader ? root.canvasHeaderPx : 0) + canvasPadding * 2

                        Item {
                            id: displayCard
                            property string dragKind: "display"
                            property var displayObject: targetItem.d
                            property bool dragging: false

                            visible: targetItem.isDisplay
                            width: targetItem.width
                            height: targetItem.height
                            z: dragging ? 100 : 0

                            Drag.active: dragging
                            Drag.source: displayCard
                            Drag.keys: ["waywallen/display"]
                            Drag.hotSpot.x: width / 2
                            Drag.hotSpot.y: height / 2
                            Drag.supportedActions: Qt.MoveAction
                            Drag.proposedAction: Qt.MoveAction

                            DragHandler {
                                id: displayDrag
                                onActiveChanged: {
                                    if (active) {
                                        displayCard.dragging = true;
                                    } else if (displayCard.dragging) {
                                        displayCard.Drag.drop();
                                        displayCard.dragging = false;
                                    }
                                }
                            }

                            states: State {
                                when: displayCard.dragging
                                ParentChange {
                                    target: displayCard
                                    parent: canvas
                                }
                            }

                            Shape {
                                visible: targetItem.isDisplay
                                anchors.fill: parent
                                preferredRendererType: Shape.CurveRenderer
                                antialiasing: true

                                ShapePath {
                                    strokeColor: targetItem.isSelected ? MD.Token.color.primary : (targetItem.modelData.placed ? MD.Token.color.outline : MD.Token.color.error)
                                    strokeWidth: targetItem.isSelected ? 3 : 1.5
                                    fillColor: targetItem.hasLink ? MD.Token.color.primary_container : MD.Token.color.surface_container_highest
                                    capStyle: ShapePath.RoundCap
                                    joinStyle: ShapePath.RoundJoin

                                    PathRectangle {
                                        x: 0
                                        y: 0
                                        width: targetItem.width
                                        height: targetItem.height
                                        radius: 10
                                    }
                                }
                            }

                            MouseArea {
                                visible: targetItem.isDisplay
                                anchors.fill: parent
                                onClicked: root.selectDisplay(targetItem.d)
                            }

                            ColumnLayout {
                                visible: targetItem.isDisplay
                                anchors.centerIn: parent
                                width: Math.max(0, targetItem.width - 12)
                                spacing: 4

                                MD.Text {
                                    Layout.fillWidth: true
                                    text: targetItem.d?.displayLabel || qsTr("Display #%1").arg(targetItem.d?.id)
                                    typescale: MD.Token.typescale.title_small
                                    color: targetItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface
                                    horizontalAlignment: Text.AlignHCenter
                                    maximumLineCount: 2
                                    wrapMode: Text.Wrap
                                    elide: Text.ElideRight
                                }

                                MD.Text {
                                    Layout.alignment: Qt.AlignHCenter
                                    text: (targetItem.d?.width || 0) + " × " + (targetItem.d?.height || 0)
                                    typescale: MD.Token.typescale.label_medium
                                    color: targetItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface_variant
                                }
                            }

                            MD.Text {
                                visible: targetItem.isDisplay
                                anchors.left: parent.left
                                anchors.top: parent.top
                                anchors.margins: 6
                                text: "#" + (targetItem.d?.id || "")
                                typescale: MD.Token.typescale.label_small
                                color: targetItem.hasLink ? MD.Token.color.on_primary_container : MD.Token.color.on_surface_variant
                            }

                            W.GpuTag {
                                visible: targetItem.isDisplay
                                anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.margins: 6
                                drmRenderMajor: targetItem.d?.drmRenderMajor || 0
                                drmRenderMinor: targetItem.d?.drmRenderMinor || 0
                            }

                            Flow {
                                visible: targetItem.isDisplay
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.bottom: parent.bottom
                                anchors.margins: 6
                                spacing: 4

                                Repeater {
                                    model: targetItem.d?.runtimeConditions || []
                                    delegate: W.RuntimeConditionTag {
                                        required property var modelData
                                        condition: modelData
                                    }
                                }
                            }
                        }

                        Rectangle {
                            id: canvasFrame
                            visible: targetItem.isCanvas
                            anchors.fill: parent
                            radius: 12
                            color: targetItem.hasLink ? MD.Token.color.secondary_container : MD.Token.color.surface_container_highest
                            border.width: canvasDropArea.containsDrag || targetItem.isSelected ? 3 : 1.5
                            border.color: canvasDropArea.containsDrag || targetItem.isSelected ? MD.Token.color.primary : MD.Token.color.outline
                        }

                        Rectangle {
                            id: canvasTitleBar
                            visible: targetItem.isCanvas
                            x: 0
                            y: 0
                            width: targetItem.verticalCanvasHeader ? root.canvasHeaderPx : parent.width
                            height: targetItem.verticalCanvasHeader ? parent.height : root.canvasHeaderPx
                            color: "transparent"

                            TapHandler {
                                onTapped: root.selectCanvas(targetItem.canvasObject)
                            }

                            RowLayout {
                                id: canvasTitleContent

                                anchors.centerIn: parent
                                width: Math.max(0, (targetItem.verticalCanvasHeader ? parent.height : parent.width) - 20)
                                height: root.canvasHeaderPx
                                rotation: targetItem.verticalCanvasHeader ? -90 : 0
                                spacing: 6

                                MD.Icon {
                                    name: MD.Token.icon.dashboard
                                    size: 18
                                    color: targetItem.hasLink ? MD.Token.color.on_secondary_container : MD.Token.color.on_surface
                                }

                                MD.Text {
                                    Layout.fillWidth: true
                                    Layout.minimumWidth: 0
                                    text: targetItem.canvasObject?.name || qsTr("Unnamed canvas")
                                    typescale: MD.Token.typescale.label_medium
                                    color: targetItem.hasLink ? MD.Token.color.on_secondary_container : MD.Token.color.on_surface
                                    maximumLineCount: 1
                                    wrapMode: Text.NoWrap
                                    elide: Text.ElideRight
                                }

                                MD.Text {
                                    text: targetItem.canvasMembers.length + " · " + targetItem.modelData.w + " × " + targetItem.modelData.h
                                    typescale: MD.Token.typescale.label_small
                                    color: targetItem.hasLink ? MD.Token.color.on_secondary_container : MD.Token.color.on_surface_variant
                                }
                            }

                            Rectangle {
                                x: targetItem.verticalCanvasHeader ? parent.width - 1 : 8
                                y: targetItem.verticalCanvasHeader ? 8 : parent.height - 1
                                width: targetItem.verticalCanvasHeader ? 1 : Math.max(0, parent.width - 16)
                                height: targetItem.verticalCanvasHeader ? Math.max(0, parent.height - 16) : 1
                                color: MD.Token.color.outline_variant
                            }
                        }

                        Rectangle {
                            id: canvasTarget
                            visible: targetItem.isCanvas
                            x: targetItem.verticalCanvasHeader ? root.canvasHeaderPx : 0
                            y: targetItem.verticalCanvasHeader ? 0 : root.canvasHeaderPx
                            width: parent.width - (targetItem.verticalCanvasHeader ? root.canvasHeaderPx : 0)
                            height: parent.height - (targetItem.verticalCanvasHeader ? 0 : root.canvasHeaderPx)
                            color: "transparent"

                            DropArea {
                                id: canvasDropArea
                                anchors.fill: parent
                                keys: ["waywallen/display"]
                                enabled: !canvasMutationQuery.querying && root.canChangeSelection("canvas", targetItem.canvasObject?.id)
                                onDropped: drop => {
                                    const payload = root.dragPayload(drop.source);
                                    if (payload.kind === "display" && root.dropDisplayOnCanvas(targetItem.canvasObject, payload.displayObject, drop.x, drop.y, targetItem.canvasMemberBounds)) {
                                        drop.acceptProposedAction();
                                    } else {
                                        drop.accepted = false;
                                    }
                                }
                            }

                            TapHandler {
                                onTapped: root.selectCanvas(targetItem.canvasObject)
                            }

                            MD.Text {
                                anchors.centerIn: parent
                                visible: targetItem.canvasMembers.length === 0
                                text: qsTr("Empty canvas")
                                typescale: MD.Token.typescale.body_medium
                                color: targetItem.hasLink ? MD.Token.color.on_secondary_container : MD.Token.color.on_surface_variant
                            }

                            Repeater {
                                model: targetItem.canvasMembers

                                delegate: Rectangle {
                                    id: memberItem
                                    required property int index
                                    required property var modelData

                                    property bool dragging: false
                                    property vector2d dragStartTranslation: Qt.vector2d(0, 0)
                                    readonly property vector2d dragTranslation: Qt.vector2d(memberDrag.persistentTranslation.x - dragStartTranslation.x, memberDrag.persistentTranslation.y - dragStartTranslation.y)

                                    readonly property bool selected: targetItem.isSelected && (memberTap.pressed || dragging)
                                    readonly property var dragPosition: {
                                        const x = Number(modelData.x || 0) + (dragging ? dragTranslation.x / canvas.viewScale : 0);
                                        const y = Number(modelData.y || 0) + (dragging ? dragTranslation.y / canvas.viewScale : 0);
                                        if (!dragging)
                                            return {
                                                x: x,
                                                y: y
                                            };
                                        return canvasEditor.snappedMemberPosition(index, x, y, root.canvasSnapPx / canvas.viewScale);
                                    }
                                    x: root.canvasPaddingPx + (dragPosition.x - targetItem.canvasMemberBounds.x) * canvas.viewScale
                                    y: root.canvasPaddingPx + (dragPosition.y - targetItem.canvasMemberBounds.y) * canvas.viewScale
                                    width: modelData.width * canvas.viewScale
                                    height: modelData.height * canvas.viewScale
                                    z: dragging ? 100 : 0
                                    radius: 8
                                    color: selected ? MD.Token.color.primary_container : MD.Token.color.surface_container_highest
                                    border.width: 1
                                    border.color: MD.Token.color.outline_variant

                                    DragHandler {
                                        id: memberDrag
                                        target: null
                                        enabled: root.canChangeSelection("canvas", targetItem.canvasObject?.id)
                                        onActiveChanged: {
                                            if (active) {
                                                memberItem.dragStartTranslation = persistentTranslation;
                                                root.selectCanvas(targetItem.canvasObject);
                                                if (root.selectedKind === "canvas" && root.selectedId === targetItem.canvasObject?.id)
                                                    memberItem.dragging = true;
                                            } else if (memberItem.dragging) {
                                                const pointer = targetItem.mapFromItem(null, centroid.scenePosition.x, centroid.scenePosition.y);
                                                const translation = memberItem.dragTranslation;
                                                const baseX = Number(memberItem.modelData.x || 0);
                                                const baseY = Number(memberItem.modelData.y || 0);
                                                memberItem.dragging = false;
                                                if (pointer.x < 0 || pointer.y < 0 || pointer.x > targetItem.width || pointer.y > targetItem.height)
                                                    canvasEditor.removeAt(memberItem.index);
                                                else if (translation.x !== 0 || translation.y !== 0)
                                                    canvasEditor.moveTo(memberItem.index, baseX + translation.x / canvas.viewScale, baseY + translation.y / canvas.viewScale, root.canvasSnapPx / canvas.viewScale);
                                            }
                                        }
                                    }

                                    TapHandler {
                                        id: memberTap
                                        enabled: root.canChangeSelection("canvas", targetItem.canvasObject?.id)
                                        onPressedChanged: if (pressed)
                                            root.selectCanvas(targetItem.canvasObject)
                                    }

                                    ColumnLayout {
                                        anchors.centerIn: parent
                                        width: Math.max(0, parent.width - 12)
                                        spacing: 2

                                        MD.Text {
                                            Layout.fillWidth: true
                                            text: memberItem.modelData.label || memberItem.modelData.settingsKey
                                            horizontalAlignment: Text.AlignHCenter
                                            elide: Text.ElideMiddle
                                            typescale: MD.Token.typescale.label_medium
                                            color: memberItem.selected ? MD.Token.color.on_primary_container : MD.Token.color.on_surface
                                        }

                                        MD.Text {
                                            Layout.fillWidth: true
                                            visible: memberItem.modelData.onlineCount !== 1
                                            text: memberItem.modelData.onlineCount > 1 ? qsTr("%1 overlapping displays").arg(memberItem.modelData.onlineCount) : qsTr("Offline")
                                            horizontalAlignment: Text.AlignHCenter
                                            elide: Text.ElideRight
                                            typescale: MD.Token.typescale.label_small
                                            color: memberItem.selected ? MD.Token.color.on_primary_container : MD.Token.color.on_surface_variant
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // --- Details panel ---
        MD.Pane {
            id: detailsPane
            implicitWidth: 400
            implicitHeight: 320
            MD.SplitViewBase.minimumWidth: Math.min(320, pageContent.availableWidth / 2)
            MD.SplitViewBase.minimumHeight: Math.min(160, pageContent.availableHeight / 2)
            MD.SplitViewBase.fillWidth: true
            MD.SplitViewBase.fillHeight: true
            MD.SplitViewBase.expanded: root.detailsExpanded
            MD.SplitViewBase.onCollapsedCompleted: root.finishClosingDetails()
            visible: root.detailsVisible

            radius: 16
            backgroundColor: MD.MProp.color.surface
            clip: true

            contentItem: MD.Scrollable {
                MD.NestedScroll.smoothWheelEnabled: W.Global.smoothScrolling
                id: detailsFlick
                clip: true
                leftMargin: 16
                rightMargin: 16
                bottomMargin: 16
                contentWidth: Math.max(0, width - leftMargin - rightMargin)
                contentHeight: root.detailsVisible ? detailsContent.implicitHeight : 0
                flickableDirection: MD.Scrollable.VerticalFlick
                interactive: contentHeight > height

                MD.ScrollBarBase.vertical: MD.ScrollBar {}

                ColumnLayout {
                    id: detailsContent
                    width: detailsFlick.contentWidth
                    spacing: 8
                    visible: root.detailsVisible

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 8
                        visible: !!root.selected

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 8

                            readonly property bool canEdit: root.selectedKind === "canvas" || (root.selectedKind === "display" && W.Util.supportsDisplayRename)

                            MD.Text {
                                Layout.fillWidth: true
                                text: root.selected ? (root.selected.displayLabel || qsTr("Display #%1").arg(root.selected.id)) : ""
                                typescale: MD.Token.typescale.title_medium
                                color: MD.Token.color.on_surface
                                wrapMode: Text.Wrap
                                maximumLineCount: 2
                                elide: Text.ElideRight
                            }

                            Repeater {
                                model: root.selected ? (root.selected.runtimeConditions || []) : []
                                delegate: W.RuntimeConditionTag {
                                    required property var modelData
                                    Layout.alignment: Qt.AlignVCenter
                                    condition: modelData
                                }
                            }

                            MD.IconButton {
                                readonly property var displayObject: root.selectedDisplayObject
                                visible: root.selectedKind === "display" && !!displayObject
                                enabled: !!displayObject && !displayPauseQuery.querying
                                icon.name: displayObject?.manualPaused ? MD.Token.icon.play_arrow : MD.Token.icon.pause
                                MD.ToolTip.visible: mdState.interactions.hovered
                                MD.ToolTip.text: displayObject?.manualPaused ? qsTr("Resume display") : qsTr("Pause display")
                                onClicked: {
                                    displayPauseQuery.displayId = displayObject.id;
                                    displayPauseQuery.paused = !displayObject.manualPaused;
                                    displayPauseQuery.reload();
                                }
                            }

                            MD.IconButton {
                                visible: parent.canEdit && !!root.selected
                                enabled: !renameQuery.querying && !canvasMutationQuery.querying && !canvasEditor.dirty
                                icon.name: MD.Token.icon.edit
                                MD.ToolTip.visible: mdState.interactions.hovered
                                MD.ToolTip.text: root.selectedKind === "canvas" ? qsTr("Edit canvas") : qsTr("Edit display")
                                onClicked: {
                                    if (root.selectedKind === "canvas") {
                                        canvasDialog.openFor(root.selectedCanvasObject);
                                    } else {
                                        displayEditDialog.openFor(root.selectedDisplayObject);
                                    }
                                }
                            }

                            MD.IconButton {
                                visible: root.selectedKind === "canvas"
                                enabled: !canvasEditor.dirty && !canvasMutationQuery.querying
                                icon.name: MD.Token.icon.delete_forever
                                MD.ToolTip.visible: mdState.interactions.hovered
                                MD.ToolTip.text: qsTr("Delete canvas")
                                onClicked: deleteCanvasDialog.open()
                            }

                            MD.IconButton {
                                enabled: !canvasEditor.dirty
                                icon.name: MD.Token.icon.close
                                onClicked: root.clearSelection()
                            }
                        }

                        Flow {
                            Layout.fillWidth: true
                            spacing: 8

                            Repeater {
                                model: root.selectedDisplayObject ? [root.selectedDisplayObject] : (W.App.displayManager.displays || []).filter(d => d.canvasId === root.selectedCanvasObject?.id)
                                delegate: RowLayout {
                                    required property var modelData
                                    visible: root.selectedKind === "canvas" || modelData.effectivePaused
                                    spacing: 4

                                    MD.Text {
                                        visible: root.selectedKind === "canvas"
                                        text: parent.modelData.displayLabel
                                        typescale: MD.Token.typescale.label_medium
                                    }
                                    MD.IconButton {
                                        readonly property var displayObject: parent.modelData
                                        visible: root.selectedKind === "canvas"
                                        enabled: !displayPauseQuery.querying
                                        icon.name: displayObject.manualPaused ? MD.Token.icon.play_arrow : MD.Token.icon.pause
                                        MD.ToolTip.visible: mdState.interactions.hovered
                                        MD.ToolTip.text: displayObject.manualPaused ? qsTr("Resume display") : qsTr("Pause display")
                                        onClicked: {
                                            displayPauseQuery.displayId = displayObject.id;
                                            displayPauseQuery.paused = !displayObject.manualPaused;
                                            displayPauseQuery.reload();
                                        }
                                    }
                                    W.Tag {
                                        visible: parent.modelData.effectivePaused
                                        text: parent.modelData.manualPaused ? qsTr("Paused") : qsTr("Automatically paused")
                                    }
                                }
                            }
                            W.Tag {
                                text: qsTr("ID:") + " " + (root.selected ? (root.selectedKind === "display" ? "#" : "") + root.selected.id : "")
                                bgColor: MD.Token.color.surface_container_highest
                                fgColor: MD.Token.color.on_surface_variant
                            }

                            W.Tag {
                                readonly property var canvasBounds: canvasEditor.topologyBounds(canvasEditor.members)
                                text: {
                                    if (!root.selected)
                                        return "";
                                    const size = root.selectedKind === "canvas"
                                        ? (canvasEditor.members.length > 0 ? canvasBounds.width + " × " + canvasBounds.height : qsTr("Empty"))
                                        : root.selected.width + " × " + root.selected.height;
                                    return size;
                                }
                                bgColor: MD.Token.color.surface_container_highest
                                fgColor: MD.Token.color.on_surface_variant
                            }

                            W.Tag {
                                visible: root.selectedKind === "canvas"
                                readonly property int onlineCount: canvasEditor.members.reduce((total, member) => total + Number(member.onlineCount || 0), 0)
                                text: qsTr("%1 total, %2 online").arg(canvasEditor.members.length).arg(onlineCount)
                                bgColor: MD.Token.color.surface_container_highest
                                fgColor: MD.Token.color.on_surface_variant
                            }

                            W.Tag {
                                visible: !!root.selected && root.selectedKind === "display" && root.selected.refreshMhz > 0
                                text: root.selected ? (root.selected.refreshMhz / 1000).toFixed(3) + " Hz" : ""
                                bgColor: MD.Token.color.surface_container_highest
                                fgColor: MD.Token.color.on_surface_variant
                            }

                            W.Tag {
                                visible: !!root.selected && root.selectedKind === "display" && (root.selected.canvasId || "").length > 0
                                readonly property var rect: root.selected ? (root.selected.canvasRect || ({})) : ({})
                                text: Number(rect.width || 0) > 0 ? (rect.x + ", " + rect.y + " · " + rect.width + " × " + rect.height) : ""
                                bgColor: MD.Token.color.surface_container_highest
                                fgColor: MD.Token.color.on_surface_variant
                            }

                            MD.AssistChip {
                                visible: !!root.selected && root.selectedKind === "display" && (root.selected.canvasId || "").length > 0
                                text: {
                                    if (!root.selected)
                                        return "";
                                    const canvas = W.App.displayManager.getCanvas(root.selected.canvasId || "");
                                    return canvas ? canvas.name : qsTr("Canvas");
                                }
                                onClicked: {
                                    const canvas = W.App.displayManager.getCanvas(root.selected.canvasId || "");
                                    if (canvas)
                                        root.selectCanvas(canvas);
                                }
                            }
                        }

                        W.DisplayLayoutControls {
                            Layout.fillWidth: true
                            visible: !!root.selected && (root.selectedKind === "canvas" || !(root.selected.canvasId || "").length)
                            displayLayout: {
                                if (!root.selected)
                                    return ({});
                                if (root.selectedKind === "canvas")
                                    return root.selectedCanvasObject?.effectiveLayout || ({});
                                return root.selected.displayLayout || root.selected.effectiveLayout || ({});
                            }
                            wallpaperOverride: root.selectedDisplayObject?.layoutOverriddenByWallpaper ?? false
                            resetEnabled: {
                                if (!root.selected)
                                    return false;
                                if (root.selectedKind === "canvas") {
                                    const override = root.selectedCanvasObject?.layoutOverride || ({});
                                    return !canvasLayoutSetQuery.querying && (override.fillmodeSet === true || override.locationSet === true || override.rotationSet === true || override.flipSet === true);
                                }
                                const override = root.selected.layoutOverride || ({});
                                return override.fillmodeSet === true || override.locationSet === true || override.alignSet === true || override.rotationSet === true || override.flipSet === true;
                            }
                            fillModeValues: root.kFillModeValues
                            fillModeLabels: root.kFillModeLabels
                            rotationValues: root.kRotationValues
                            rotationLabels: root.kRotationLabels
                            onFillModeRequested: value => root.applyFillmode(value)
                            onLocationRequested: (x, y) => root.applyLocation(x, y)
                            onRotationRequested: value => root.applyRotation(value)
                            onFlipRequested: value => root.applyFlip(value)
                            onResetRequested: resetDisplaySettingsDialog.open()
                        }

                        W.RendererConnectionPanel {
                            Layout.fillWidth: true
                            target: root.selected
                        }

                        W.CanvasDisplayList {
                            Layout.fillWidth: true
                            visible: root.selectedKind === "canvas" && canvasEditor.members.length > 0
                            editor: canvasEditor
                            busy: canvasMutationQuery.querying
                        }

                    }
                }
            }
        }
    }
}
