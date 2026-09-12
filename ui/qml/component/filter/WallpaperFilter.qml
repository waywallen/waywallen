pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import waywallen.control as WC
import waywallen.ui as W
import Qcm.Material as MD

MD.ItemDelegate {
    id: root
    required property var model
    required property int index
    required property var popupWindow
    property var supportedTypes: []
    property var allTags: []
    property var valueLabels: ({})
    property var allContentRatings: []
    property WC.wallpaperStringFilter emptyStringFilter
    property WC.wallpaperIntFilter emptyIntFilter
    property WC.wallpaperTagFilter emptyTagFilter

    font.capitalization: Font.MixedCase

    readonly property var typeOptions: [
        { name: qsTr("Name"),           value: WC.WallpaperFilterType.WALLPAPER_FILTER_TYPE_NAME,    kind: "string"  },
        { name: qsTr("Type"),           value: WC.WallpaperFilterType.WALLPAPER_FILTER_TYPE_WP_TYPE, kind: "wp_type" },
        { name: qsTr("Width"),          value: WC.WallpaperFilterType.WALLPAPER_FILTER_TYPE_WIDTH,   kind: "int"     },
        { name: qsTr("Height"),         value: WC.WallpaperFilterType.WALLPAPER_FILTER_TYPE_HEIGHT,  kind: "int"     },
        { name: qsTr("Size"),           value: WC.WallpaperFilterType.WALLPAPER_FILTER_TYPE_SIZE,    kind: "int"     },
        { name: qsTr("Tag"),            value: WC.WallpaperFilterType.WALLPAPER_FILTER_TYPE_TAG,     kind: "tag"     },
        { name: qsTr("Content rating"), value: WC.WallpaperFilterType.WALLPAPER_FILTER_TYPE_CONTENT_RATING, kind: "rating" }
    ]

    readonly property var currentOption: typeOptions.find(e => e.value === root.model.type) || null

    readonly property var currentSpec: {
        if (!currentOption)
            return emptySpec;
        if (currentOption.kind === "string")
            return stringSpec;
        if (currentOption.kind === "wp_type")
            return wpTypeSpec;
        if (currentOption.kind === "int")
            return intSpec;
        if (currentOption.kind === "tag")
            return tagSpec;
        if (currentOption.kind === "rating")
            return ratingSpec;
        return emptySpec;
    }

    function applyType(option) {
        root.model.type = option.value;
        switch (option.kind) {
        case "string":
        case "wp_type":
        case "rating":
            root.model.stringFilter = emptyStringFilter;
            break;
        case "int":
            root.model.intFilter = emptyIntFilter;
            break;
        case "tag":
            root.model.tagFilter = emptyTagFilter;
            break;
        }
    }

    W.StringFilter {
        id: stringSpec
        filter: root.currentOption && root.currentOption.kind === "string" ? root.model : null
    }
    W.WpTypeFilter {
        id: wpTypeSpec
        filter: root.currentOption && root.currentOption.kind === "wp_type" ? root.model : null
        supportedTypes: root.supportedTypes
    }
    W.IntFilter {
        id: intSpec
        filter: root.currentOption && root.currentOption.kind === "int" ? root.model : null
    }
    W.TagFilter {
        id: tagSpec
        popupWindow: root.popupWindow
        filter: root.currentOption && root.currentOption.kind === "tag" ? root.model : null
        allTags: root.allTags
        valueLabels: root.valueLabels
        availableWidth: chipFlow.width
    }
    W.ContentRatingFilter {
        id: ratingSpec
        filter: root.currentOption && root.currentOption.kind === "rating" ? root.model : null
        allRatings: root.allContentRatings
        valueLabels: root.valueLabels
    }
    W.EmptyFilter { id: emptySpec }

    contentItem: RowLayout {
        Flow {
            id: chipFlow
            Layout.fillWidth: true
            spacing: 12

            MD.InputChip {
                id: nameChip
                text: root.currentOption ? root.currentOption.name : qsTr("Filter")
                onClicked: typeMenu.open()
            }

            MD.InputChip {
                id: conditionChip
                text: {
                    const spec = root.currentSpec;
                    const item = (spec.conditionModel || []).find(e => e.value === spec.condition);
                    return item ? item.name : "";
                }
                onClicked: conditionMenu.open()

                MD.Menu {
                    id: conditionMenu
                    parent: conditionChip
                    y: parent.height
                    model: root.currentSpec.conditionModel || []
                    contentDelegate: MD.MenuItem {
                        required property var modelData
                        text: modelData.name
                        onClicked: {
                            root.currentSpec.condition = modelData.value;
                            conditionMenu.close();
                        }
                    }
                }
            }

            Loader {
                id: valueLoader
                sourceComponent: root.currentSpec.valueDelegate
            }
        }

        MD.SmallIconButton {
            icon.name: MD.Token.icon.close
            onClicked: {
                const view = root.ListView.view;
                if (!view || !view.model)
                    return;
                view.model.removeRow(root.index);
            }
        }
    }

    background: MD.Rectangle {
        corners: {
            const view = root.ListView.view;
            if (!view || !view.model)
                return 0;
            const model = view.model;
            void(view.count);
            return MD.Util.listCorners(model.rowIndexInGroup(root.index),
                                       model.rowCountInGroupOf(root.index), 12);
        }
        color: root.MD.MProp.color.surface
    }

    MD.Menu {
        id: typeMenu
        parent: root
        y: root.contentItem.y + root.contentItem.height
        model: root.typeOptions
        contentDelegate: MD.MenuItem {
            required property var modelData
            text: modelData.name
            onClicked: {
                root.applyType(modelData);
                typeMenu.close();
            }
        }
    }
}
