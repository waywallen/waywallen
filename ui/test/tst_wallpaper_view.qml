pragma ComponentBehavior: Bound
import QtQuick
import QtTest
import Qcm.Material as MD
import "../qml/component" as W

TestCase {
    id: test

    function init() {
        settings.viewMode = settings.viewGrid;
        settings.itemAspectRatio = 1;
        settings.itemSpacing = 12;
        rows.clear();
        for (let i = 0; i < 120; ++i)
            rows.append({
                itemKey: "key-" + i,
                extent: i % 3 === 0 ? 220 : 90
            });
    }
    function test_itemSpacing_data() {
        return [
            {
                tag: "grid",
                viewMode: settings.viewGrid
            },
            {
                tag: "masonry",
                viewMode: settings.viewMasonry
            }
        ];
    }
    function test_itemSpacing(data) {
        settings.viewMode = data.viewMode;
        const view = createTemporaryObject(viewComponent, test);
        verify(view);
        tryCompare(view, "count", 120);
        for (const spacing of [12, 0, 16]) {
            settings.itemSpacing = spacing;
            verifySpacing(view, spacing, data.viewMode === settings.viewMasonry);
        }
    }
    function test_currentItemOutline_data() {
        return test_itemSpacing_data();
    }
    function test_currentItemOutline(data) {
        settings.viewMode = data.viewMode;
        const view = createTemporaryObject(viewComponent, test);
        verify(view);
        tryCompare(view, "count", 120);
        const outline = findChild(view, "currentItemOutline");
        verify(outline);
        for (const spacing of [16, 12, 4, 2, 0]) {
            settings.itemSpacing = spacing;
            verifySpacing(view, spacing, data.viewMode === settings.viewMasonry);
            for (const index of [1, 0]) {
                view.currentIndex = index;
                verifyOutline(view, outline, spacing, index);
            }
            view.currentIndex = -1;
            tryCompare(outline, "visible", false);
            compare(view.currentKey, "");
            view.currentIndex = 2;
            verifyOutline(view, outline, spacing, 2);
        }
    }
    function test_layoutSwitchKeepsIdentityAndOffset() {
        const view = createTemporaryObject(viewComponent, test);
        verify(view);
        tryCompare(view, "count", 120);
        tryVerify(() => view.itemRect(0).height > 0);
        verify(view.positionAtIndex(45));
        tryVerify(() => view.anchorIndex >= 40);
        tryVerify(() => !view.positioning);
        view.currentIndex = 46;
        const key = view.keyAtIndex(view.anchorIndex);
        const index = view.anchorIndex;
        const y = view.itemRect(index).y;
        settings.viewMode = settings.viewMasonry;
        tryVerify(() => view.itemRect(index).height === rows.get(index).extent);
        tryVerify(() => !view.positioning);
        tryVerify(() => Math.abs(view.itemRect(index).y - y) <= 1);
        compare(view.currentKey, "key-46");
        compare(view.keyAtIndex(index), key);
        compare(rows.count, 120);
        compare(view.errorString, "");
        const backIndex = view.anchorIndex;
        const backY = view.itemRect(backIndex).y;
        settings.viewMode = settings.viewGrid;
        tryVerify(() => !view.positioning);
        tryVerify(() => Math.abs(view.itemRect(backIndex).y - backY) <= 1);
        compare(view.currentKey, "key-46");
    }
    function test_sharedShimmerSurvivesReuseAndLayoutSwitch() {
        const view = createTemporaryObject(shimmerViewComponent, test);
        verify(view);
        tryCompare(view, "count", 120);
        tryVerify(() => findChild(view, "shimmerCell-key-0") !== null);
        const controller = view.shimmer;
        verify(controller);
        compare(controller.boundsMode, MD.Shimmer.Item);
        compare(controller.boundsItem, view);
        controller.running = false;
        controller.seek(0.375);
        compare(controller.phase, 0.375);
        for (const index of [0, 1, 2]) {
            const cell = findChild(view, "shimmerCell-key-" + index);
            verify(cell);
            compare(cell.shimmer, controller);
        }

        verify(view.positionAtIndex(60));
        tryVerify(() => !view.positioning);
        tryVerify(() => view.anchorIndex > 40);
        const anchor = view.anchorIndex;
        const visibleCell = findChild(view, "shimmerCell-key-" + anchor);
        verify(visibleCell);
        compare(visibleCell.shimmer, controller);
        compare(view.shimmer, controller);
        compare(controller.phase, 0.375);

        settings.viewMode = settings.viewMasonry;
        tryVerify(() => view.itemRect(anchor).height === rows.get(anchor).extent);
        tryVerify(() => !view.positioning);
        const masonryCell = findChild(view, "shimmerCell-key-" + anchor);
        verify(masonryCell);
        compare(masonryCell.shimmer, controller);
        compare(view.shimmer, controller);
        compare(controller.boundsItem, view);
        compare(controller.phase, 0.375);

        settings.viewMode = settings.viewGrid;
        tryVerify(() => !view.positioning);
        verify(view.positionAtIndex(0));
        tryVerify(() => !view.positioning);
        tryVerify(() => findChild(view, "shimmerCell-key-0") !== null);
        compare(findChild(view, "shimmerCell-key-0").shimmer, controller);
        compare(view.shimmer, controller);
        compare(controller.phase, 0.375);
        compare(view.errorString, "");
    }
    function test_layoutSwitchWithEarlierTallImage() {
        settings.viewMode = settings.viewMasonry;
        rows.setProperty(0, "extent", 10000);
        const view = createTemporaryObject(viewComponent, test);
        verify(view);
        tryVerify(() => view.itemRect(0).height === 10000);
        verify(view.positionAtIndex(40));
        tryVerify(() => !view.positioning);
        tryCompare(view, "firstVisibleIndex", 0);
        tryVerify(() => view.anchorIndex > 0);
        const anchor = view.anchorIndex;
        const key = view.keyAtIndex(anchor);
        const y = view.itemRect(anchor).y;
        view.currentIndex = anchor;
        settings.viewMode = settings.viewGrid;
        tryVerify(() => !view.positioning);
        tryVerify(() => Math.abs(view.itemRect(anchor).y - y) <= 1);
        compare(view.currentKey, key);
        compare(view.errorString, "");
    }
    function test_resizeAndRowChanges() {
        settings.viewMode = settings.viewMasonry;
        const view = createTemporaryObject(viewComponent, test);
        verify(view);
        tryVerify(() => view.itemRect(4).height > 0);
        verify(view.positionAtIndex(40));
        tryVerify(() => view.contentY > 500);
        tryVerify(() => !view.positioning);
        view.currentIndex = 40;
        rows.insert(0, {
            itemKey: "inserted",
            extent: 180
        });
        tryCompare(view, "count", 121);
        tryCompare(view, "currentIndex", 41);
        compare(view.currentKey, "key-40");
        tryVerify(() => view.itemRect(41).height > 0);
        const anchorIndex = view.anchorIndex;
        const anchorKey = view.keyAtIndex(anchorIndex);
        const y = view.itemRect(anchorIndex).y;
        view.width = 320;
        tryVerify(() => view.cellWidth < 160);
        tryVerify(() => Math.abs(view.itemRect(anchorIndex).y - y) <= 1);
        compare(view.keyAtIndex(anchorIndex), anchorKey);
        compare(view.errorString, "");
    }
    function verifyOutline(view, outline, spacing, index) {
        const tolerance = 0.001;
        const outset = Math.min(4, spacing / 2);
        tryCompare(outline, "visible", true);
        tryCompare(view, "currentKey", "key-" + index);
        tryVerify(() => Math.abs(outline.x - (view.itemRect(index).x - outset)) < tolerance);
        tryVerify(() => Math.abs(outline.y - (view.itemRect(index).y - outset)) < tolerance);
        tryVerify(() => Math.abs(outline.width - (view.itemRect(index).width + 2 * outset)) < tolerance);
        tryVerify(() => Math.abs(outline.height - (view.itemRect(index).height + 2 * outset)) < tolerance);
        fuzzyCompare(outline.radius, Math.min(MD.Token.shape.corner.extra_small, spacing) + outset, tolerance);
        compare(outline.border.width, 3);
        for (let neighbor = 0; neighbor < 9; ++neighbor) {
            if (neighbor === index)
                continue;
            const rect = view.itemRect(neighbor);
            const overlapWidth = Math.min(outline.x + outline.width, rect.x + rect.width) - Math.max(outline.x, rect.x);
            const overlapHeight = Math.min(outline.y + outline.height, rect.y + rect.height) - Math.max(outline.y, rect.y);
            verify(!(overlapWidth > tolerance && overlapHeight > tolerance), "Outline for " + index + " overlaps item " + neighbor + " at spacing " + spacing);
        }
        if (spacing === 0) {
            const rect = view.itemRect(index);
            compare(outline.x, rect.x);
            compare(outline.y, rect.y);
            compare(outline.x + outline.width, rect.x + rect.width);
            compare(outline.y + outline.height, rect.y + rect.height);
            compare(outline.radius, 0);
        }
    }
    function verifySpacing(view, spacing, masonry) {
        const tolerance = 0.001;
        const margin = spacing / 2;
        const cellWidth = (view.width - 2 * margin - 2 * spacing) / 3;
        tryCompare(view, "leftMargin", margin);
        tryCompare(view, "rightMargin", margin);
        tryCompare(view, "topMargin", margin);
        tryCompare(view, "bottomMargin", margin);
        tryVerify(() => Math.abs(view.cellWidth - cellWidth) < tolerance);
        tryVerify(() => Math.abs(view.itemRect(0).height - (masonry ? 220 : cellWidth)) < tolerance);
        tryVerify(() => Math.abs(view.itemRect(1).height - (masonry ? 90 : cellWidth)) < tolerance);
        tryVerify(() => Math.abs(view.itemRect(3).height - (masonry ? 220 : cellWidth)) < tolerance);
        tryVerify(() => Math.abs(view.itemRect(1).x - view.itemRect(0).x - view.itemRect(0).width - spacing) < tolerance);
        tryVerify(() => Math.abs(view.itemRect(2).x - view.itemRect(1).x - view.itemRect(1).width - spacing) < tolerance);
        const previous = masonry ? 1 : 0;
        tryVerify(() => Math.abs(view.itemRect(3).y - view.itemRect(previous).y - view.itemRect(previous).height - spacing) < tolerance);
        const first = view.itemRect(0);
        const second = view.itemRect(1);
        const last = view.itemRect(2);
        const next = view.itemRect(3);
        fuzzyCompare(first.width, cellWidth, tolerance);
        fuzzyCompare(second.width, cellWidth, tolerance);
        fuzzyCompare(last.width, cellWidth, tolerance);
        fuzzyCompare(first.y, second.y, tolerance);
        fuzzyCompare(second.y, last.y, tolerance);
        fuzzyCompare(next.x, masonry ? second.x : first.x, tolerance);
        if (spacing === 0) {
            fuzzyCompare(first.x, 0, tolerance);
            fuzzyCompare(first.y, 0, tolerance);
            fuzzyCompare(last.x + last.width, view.width, tolerance);
        }
        compare(view.errorString, "");
    }

    height: 500
    name: "WallpaperView"
    visible: true
    when: windowShown
    width: 500

    QtObject {
        id: settings

        property real itemAspectRatio: 1
        readonly property real itemHeight: itemSize / itemAspectRatio
        property real itemSize: 140
        property real itemSpacing: 12
        readonly property int layoutFillCell: 0
        property int layoutMode: layoutFillCell
        readonly property int viewGrid: 0
        readonly property int viewMasonry: 1
        property int viewMode: viewGrid
    }
    ListModel {
        id: rows

        property bool hasMore: false
    }
    Component {
        id: viewComponent

        W.WallpaperView {
            height: 500
            model: rows
            roles: ["extent"]
            tweak: settings
            width: 500

            delegate: Rectangle {
                required property MD.LazyRow row

                color: "transparent"
                implicitHeight: row.value.extent
            }
        }
    }
    Component {
        id: shimmerViewComponent

        W.WallpaperView {
            id: shimmerView

            height: 500
            model: rows
            roles: ["extent"]
            tweak: settings
            width: 500

            delegate: MD.Skeleton {
                required property MD.LazyRow row

                implicitHeight: row.value.extent
                objectName: "shimmerCell-" + row.key
                shimmer: shimmerView.shimmer
            }
        }
    }
}
