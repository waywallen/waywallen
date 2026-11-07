pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import waywallen.control as WC
import waywallen.ui as W
import Qcm.Material as MD

MD.Dialog {
    id: root
    title: qsTr("Filters")
    required property var popupWindow
    property var model
    property var supportedTypes: []
    // Renderer-declared labels for the type keys, display only.
    property var typeLabels: ({})
    // Quick type toggles (applied immediately, independent of the rule
    // editor's Apply/Reset). Chips show all types; a checked chip means
    // the type is shown. Unchecked types are what we record as skipped.
    property var skipTypes: []
    signal toggleSkip(string ty)
    // Quick tag filter (independent of the rules): selected tags directly;
    // empty = no constraint (all). Edited via the tag picker, applied as a
    // whole list.
    property var filterTags: []
    // Source-declared labels for tag and content-rating values, display only.
    property var valueLabels: ({})
    signal applyFilterTags(var tags)
    // Quick content-rating toggles (like Types): chips show all ratings;
    // checked = shown, unchecked ones are recorded as skipped.
    property var skipContentRatings: []
    signal toggleSkipRating(string rating)
    // 0=EXCLUDE, 1=ONLY, 2=INCLUDE — see WallpaperHiddenFilter.
    property int hiddenFilter: 0
    signal hiddenFilterSelected(int value)
    horizontalPadding: 16
    implicitWidth: Math.min(440, parent ? parent.width - 48 : 440)
    standardButtons: MD.Dialog.Close
    property var filterTagPresentation: null

    // Tag names for the tag-filter picker; refreshed each time the
    // dialog opens so newly-scanned tags show up.
    W.TagListQuery {
        id: tagListQuery
    }
    W.ContentRatingListQuery {
        id: ratingListQuery
    }

    Component {
        id: filterTagDialogComponent

        W.TagPickerDialog {
            allTags: tagListQuery.tags
            tagLabels: root.valueLabels
            selected: root.filterTags
            onCommit: function (tags) {
                root.applyFilterTags(tags);
            }
        }
    }

    onAboutToShow: {
        tagListQuery.reload();
        ratingListQuery.reload();
    }

    function openFilterTagDialog() {
        if (root.filterTagPresentation?.active)
            return;
        root.filterTagPresentation = root.popupWindow.presentPopup(filterTagDialogComponent);
    }

    onClosed: root.filterTagPresentation?.close()
    Component.onDestruction: root.filterTagPresentation?.cancel()

    contentItem: W.WallpaperFilterRules {
        id: rulesView
        model: root.model
        popupWindow: root.popupWindow
        supportedTypes: root.supportedTypes
        typeLabels: root.typeLabels
        allTags: tagListQuery.tags
        valueLabels: root.valueLabels
        allContentRatings: ratingListQuery.ratings
        implicitHeight: Math.min(contentHeight + topMargin + bottomMargin, 480)
        spacing: 2

        // Quick filters (Types / Tags) ride in the list header so they
        // scroll together with the structured rules.
        header: ColumnLayout {
            width: rulesView.contentWidth
            spacing: 8

            ColumnLayout {
                Layout.fillWidth: true
                Layout.bottomMargin: 4
                spacing: 4
                visible: root.supportedTypes && root.supportedTypes.length > 0

                MD.Label {
                    text: qsTr("Types")
                    typescale: MD.Token.typescale.title_medium
                }

                Flow {
                    Layout.fillWidth: true
                    spacing: 8
                    Repeater {
                        model: root.supportedTypes
                        delegate: MD.FilterChip {
                            required property var modelData
                            text: W.I18n.valueLabel(root.typeLabels, modelData)
                            selected: (root.skipTypes || []).indexOf(modelData) < 0
                            onSelectionRequested: root.toggleSkip(modelData)
                        }
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                Layout.bottomMargin: 4
                spacing: 4
                visible: tagListQuery.tags && tagListQuery.tags.length > 0

                RowLayout {
                    Layout.fillWidth: true
                    MD.Label {
                        Layout.fillWidth: true
                        text: qsTr("Tags")
                        typescale: MD.Token.typescale.title_medium
                    }
                    MD.IconButton {
                        icon.name: MD.Token.icon.edit
                        onClicked: root.openFilterTagDialog()
                    }
                }

                Flow {
                    Layout.fillWidth: true
                    visible: root.filterTags && root.filterTags.length > 0
                    spacing: 6

                    Repeater {
                        model: root.filterTags
                        delegate: W.Tag {
                            required property var modelData
                            text: W.I18n.valueLabel(root.valueLabels, modelData)
                        }
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                Layout.bottomMargin: 4
                spacing: 4
                visible: ratingListQuery.ratings && ratingListQuery.ratings.length > 0

                MD.Label {
                    text: qsTr("Content rating")
                    typescale: MD.Token.typescale.title_medium
                }

                Flow {
                    Layout.fillWidth: true
                    spacing: 8
                    Repeater {
                        model: ratingListQuery.ratings
                        delegate: MD.FilterChip {
                            required property var modelData
                            text: W.I18n.valueLabel(root.valueLabels, modelData)
                            selected: (root.skipContentRatings || []).indexOf(modelData) < 0
                            onSelectionRequested: root.toggleSkipRating(modelData)
                        }
                    }
                }
            }

            ColumnLayout {
                Layout.fillWidth: true
                Layout.bottomMargin: 4
                spacing: 4

                MD.Label {
                    text: qsTr("Visibility")
                    typescale: MD.Token.typescale.title_medium
                }

                MD.SegmentedButtonGroup {
                    size: MD.Enum.XS

                    MD.SegmentedButton {
                        text: qsTr("Not hidden")
                        checked: root.hiddenFilter === WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_EXCLUDE
                        onCheckedChangeRequested: root.hiddenFilterSelected(WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_EXCLUDE)
                    }
                    MD.SegmentedButton {
                        text: qsTr("Hidden only")
                        checked: root.hiddenFilter === WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_ONLY
                        onCheckedChangeRequested: root.hiddenFilterSelected(WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_ONLY)
                    }
                    MD.SegmentedButton {
                        text: qsTr("All")
                        checked: root.hiddenFilter === WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_INCLUDE
                        onCheckedChangeRequested: root.hiddenFilterSelected(WC.WallpaperHiddenFilter.WALLPAPER_HIDDEN_FILTER_INCLUDE)
                    }
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8
                MD.Label {
                    Layout.fillWidth: true
                    text: qsTr("Rules")
                    typescale: MD.Token.typescale.title_medium
                }
                MD.Button {
                    Layout.alignment: Qt.AlignVCenter
                    mdState.size: MD.Enum.XS
                    mdState.type: MD.Enum.BtFilledTonal
                    text: qsTr("Apply")
                    enabled: !!root.model && root.model.dirty
                    onClicked: root.model.apply()
                }
                Row {
                    spacing: 0
                    MD.IconButton {
                        icon.name: MD.Token.icon.restart_alt
                        enabled: !!root.model && root.model.dirty
                        onClicked: root.model.reset()
                    }
                    MD.IconButton {
                        icon.name: MD.Token.icon.clear_all
                        onClicked: {
                            root.model.removeRows(0, root.model.rowCount());
                            root.model.filterLogics = [];
                        }
                    }
                    MD.IconButton {
                        icon.name: MD.Token.icon.add_box
                        onClicked: root.model.appendNewGroup()
                    }
                }
            }
        }
    }
}
