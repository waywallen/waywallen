pragma ComponentBehavior: Bound
import QtQuick
import Qcm.Material as MD

FocusScope {
    id: root

    property int _layoutRevision: 0
    property bool _ready: false
    property var _savedPosition: null
    readonly property int anchorIndex: view.item?.anchorIndex ?? -1
    property real bottomMargin: itemSpacing / 2
    readonly property real cellWidth: view.item?.cellCrossExtent ?? 0
    readonly property real contentY: view.item?.contentY ?? 0
    readonly property int count: itemSource.count
    property int currentIndex: -1
    readonly property string currentKey: view.item?.currentKey ?? ""
    property Component delegate
    readonly property real displayItemHeight: displayItemWidth / Math.max(tweak.itemAspectRatio, 0.1)
    readonly property real displayItemWidth: masonry || fillCell ? cellWidth : Math.min(tweak.itemSize, cellWidth)
    readonly property string errorString: itemSource.errorString || view.item?.errorString || ""
    readonly property bool fillCell: tweak.layoutMode === tweak.layoutFillCell
    readonly property int firstVisibleIndex: view.item?.firstVisibleIndex ?? -1
    readonly property real itemSpacing: tweak.itemSpacing ?? 12
    property real leftMargin: itemSpacing / 2
    readonly property bool masonry: tweak.viewMode === tweak.viewMasonry
    property var model
    readonly property bool positioning: view.item?.positioning ?? false
    property real rightMargin: itemSpacing / 2
    property list<string> roles: ["item"]
    readonly property MD.Shimmer shimmer: sharedShimmer
    property real topMargin: itemSpacing / 2
    required property var tweak

    signal activated(int index, int modifiers)

    function itemRect(index) {
        return view.item ? view.item.itemRect(index) : Qt.rect(0, 0, 0, 0);
    }
    function keyAtIndex(index) {
        return view.item ? view.item.keyAtIndex(index) : "";
    }
    function positionAtIndex(index, alignment = 0) {
        return view.item ? view.item.positionAtIndex(index, alignment) : false;
    }
    function switchLayout() {
        if (view.item) {
            const first = view.item.anchorIndex;
            _savedPosition = {
                key: first >= 0 ? view.item.keyAtIndex(first) : "",
                y: first >= 0 ? view.item.itemRect(first).y : 0,
                currentKey: view.item.currentKey
            };
        }
        view.sourceComponent = masonry ? masonryView : gridView;
    }

    clip: true

    Component.onCompleted: {
        _ready = true;
        switchLayout();
    }
    Keys.onEnterPressed: event => {
        if (currentIndex >= 0)
            activated(currentIndex, event.modifiers);
    }
    Keys.onReturnPressed: event => {
        if (currentIndex >= 0)
            activated(currentIndex, event.modifiers);
    }
    onActiveFocusChanged: {
        if (activeFocus && view.item)
            view.item.forceActiveFocus();
    }
    onCurrentIndexChanged: {
        if (view.item && view.item.currentIndex !== currentIndex)
            view.item.currentIndex = currentIndex;
    }
    onErrorStringChanged: {
        if (errorString.length > 0)
            console.warn("WallpaperView:", errorString);
    }
    onMasonryChanged: {
        if (_ready)
            switchLayout();
    }

    MD.Shimmer {
        id: sharedShimmer

        boundsItem: root
        boundsMode: MD.Shimmer.Item
    }
    MD.ItemModelSource {
        id: itemSource

        keyRole: "itemKey"
        model: root.model
        roles: root.roles
    }
    Connections {
        function onHasMoreChanged() {
            itemSource.notifyFetchStateChanged();
        }

        target: root.model
    }
    Connections {
        function onCurrentChanged() {
            root.currentIndex = view.item.currentIndex;
        }
        function onItemsNeeded(demand) {
            itemSource.requestMore(demand);
        }
        function onLayoutChanged() {
            ++root._layoutRevision;
        }

        target: view.item
    }
    Loader {
        id: view

        anchors.fill: parent

        onLoaded: {
            const saved = root._savedPosition;
            root._savedPosition = null;
            if (saved) {
                item.currentKey = saved.currentKey;
                root.currentIndex = item.currentIndex;
                if (saved.key.length > 0)
                    item.positionAtKey(saved.key, 0, root.topMargin - saved.y);
            } else {
                item.currentIndex = root.currentIndex;
            }
            if (root.activeFocus)
                item.forceActiveFocus();
        }
    }
    Component {
        id: gridView

        MD.LazyGrid {
            bottomMargin: root.bottomMargin
            cacheExtent: 300
            clip: true
            crossSpacing: root.itemSpacing
            delegate: root.delegate
            focus: true
            itemExtent: root.fillCell ? cellCrossExtent / Math.max(root.tweak.itemAspectRatio, 0.1) : root.tweak.itemHeight
            leftMargin: root.leftMargin
            mainSpacing: root.itemSpacing
            minimumCellExtent: root.tweak.itemSize
            rightMargin: root.rightMargin
            source: itemSource
            topMargin: root.topMargin

            MD.ScrollBarBase.vertical: MD.ScrollBar {}
        }
    }
    Component {
        id: masonryView

        MD.LazyMasonryGrid {
            bottomMargin: root.bottomMargin
            cacheExtent: 300
            clip: true
            crossSpacing: root.itemSpacing
            delegate: root.delegate
            estimatedItemExtent: cellCrossExtent / Math.max(root.tweak.itemAspectRatio, 0.1)
            focus: true
            leftMargin: root.leftMargin
            mainSpacing: root.itemSpacing
            minimumCellExtent: root.tweak.itemSize
            rightMargin: root.rightMargin
            source: itemSource
            topMargin: root.topMargin

            MD.ScrollBarBase.vertical: MD.ScrollBar {}
        }
    }
    Rectangle {
        readonly property rect itemBounds: {
            const revision = root._layoutRevision;
            return view.item && root.currentIndex >= 0 ? view.item.itemRect(root.currentIndex) : Qt.rect(0, 0, 0, 0);
        }
        readonly property real outset: Math.min(4, root.itemSpacing / 2)

        border.color: MD.Token.color.primary
        border.width: 3
        color: "transparent"
        height: Math.max(0, itemBounds.height + 2 * outset)
        objectName: "currentItemOutline"
        radius: Math.min(MD.Token.shape.corner.extra_small, root.itemSpacing) + outset
        visible: root.currentIndex >= 0 && itemBounds.width > 0 && itemBounds.height > 0
        width: Math.max(0, itemBounds.width + 2 * outset)
        x: itemBounds.x - outset
        y: itemBounds.y - outset
        z: 2
    }
}
