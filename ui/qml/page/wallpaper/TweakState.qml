pragma ComponentBehavior: Bound
import QtCore
import QtQml
import waywallen.ui as W

QtObject {
    id: root

    readonly property int imageCacheMiB: W.Global.imageCacheMiB
    readonly property var imageCacheOptions: W.Global.imageCacheOptions
    property real itemAspectRatio: 1
    readonly property real itemHeight: itemSize / Math.max(itemAspectRatio, 0.1)
    property int itemSize: 162
    readonly property int itemSizeStep: 8
    property int itemSpacing: 12
    readonly property int itemSpacingStep: 1
    readonly property int layoutFillCell: 0
    readonly property int layoutFixed: 1
    property int layoutMode: layoutFillCell
    readonly property int maximumItemSize: 260
    readonly property int maximumItemSpacing: 16
    readonly property int minimumItemSize: 112
    readonly property int minimumItemSpacing: 0
    readonly property Settings settings: Settings {
        property alias itemAspectRatio: root.itemAspectRatio
        property alias itemSize: root.itemSize
        property alias itemSpacing: root.itemSpacing
        property alias layoutMode: root.layoutMode
        property alias viewMode: root.viewMode

        category: root.settingsCategory
    }
    property string settingsCategory: "WallpaperView"
    readonly property int viewGrid: 0
    readonly property int viewMasonry: 1
    property int viewMode: viewGrid

    function setImageCacheMiB(value) {
        W.Global.setImageCacheMiB(value);
    }
    function setItemAspectRatio(ratio) {
        const next = Number(ratio);
        itemAspectRatio = next > 0 ? next : 1;
    }
    function setItemSize(size) {
        const stepped = Math.round(Number(size) / itemSizeStep) * itemSizeStep;
        itemSize = Math.max(minimumItemSize, Math.min(maximumItemSize, stepped));
    }
    function setItemSpacing(spacing) {
        const next = Number(spacing);
        if (!Number.isFinite(next))
            return;
        itemSpacing = Math.max(minimumItemSpacing, Math.min(maximumItemSpacing, Math.round(next)));
    }
    function setLayoutMode(mode) {
        layoutMode = mode === layoutFixed ? layoutFixed : layoutFillCell;
    }
    function setViewMode(mode) {
        viewMode = mode === viewMasonry ? viewMasonry : viewGrid;
    }

    Component.onCompleted: {
        setItemSize(itemSize);
        setItemSpacing(itemSpacing);
        setItemAspectRatio(itemAspectRatio);
        setLayoutMode(layoutMode);
        setViewMode(viewMode);
    }
}
