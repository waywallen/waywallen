pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import QtQuick.Templates as T
import waywallen.control as WC
import waywallen.ui as W
import Qcm.Material as MD

MD.Dialog {
    id: root
    title: qsTr("Filters")
    required property var popupWindow
    property var model
    property var supportedTypes: []
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
    horizontalPadding: 16
    implicitWidth: Math.min(440, parent ? parent.width - 48 : 440)
    standardButtons: T.Dialog.Close
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

    contentItem: MD.VerticalListView {
        id: rulesView
        model: root.model
        implicitHeight: Math.min(contentHeight, 480)
        spacing: 2

        // Quick filters (Types / Tags) ride in the list header so they
        // scroll together with the structured rules.
        header: ColumnLayout {
            width: rulesView.width
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
                            checkable: false
                            text: qsTr(modelData)
                            checked: (root.skipTypes || []).indexOf(modelData) < 0
                            onClicked: root.toggleSkip(modelData)
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
                            checkable: false
                            text: W.I18n.valueLabel(root.valueLabels, modelData)
                            checked: (root.skipContentRatings || []).indexOf(modelData) < 0
                            onClicked: root.toggleSkipRating(modelData)
                        }
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
                        onClicked: root.model.removeRows(0, root.model.rowCount())
                    }
                    MD.IconButton {
                        icon.name: MD.Token.icon.add
                        onClicked: root.model.appendNewGroup()
                    }
                }
            }
        }

        delegate: W.WallpaperFilter {
            width: ListView.view.width
            popupWindow: root.popupWindow
            supportedTypes: root.supportedTypes
            allTags: tagListQuery.tags
            valueLabels: root.valueLabels
            allContentRatings: ratingListQuery.ratings
        }

        section.property: "group"
        section.criteria: ViewSection.FullString
        section.delegate: RowLayout {
            id: sectionRow
            width: ListView.view.contentWidth
            spacing: 8
            required property string section
            readonly property int groupId: parseInt(section, 10)
            readonly property int sectionIndex: root.model ? root.model.sectionIndexForGroup(groupId) : -1
            readonly property int currentOp: root.model && sectionIndex > 0 ? root.model.logicOpAt(sectionIndex) : -1

            MD.Label {
                Layout.fillWidth: true
                text: qsTr("Group %1").arg(sectionRow.sectionIndex + 1)
                typescale: MD.Token.typescale.label_medium
            }

            MD.SegmentedButtonGroup {
                visible: sectionRow.sectionIndex > 0

                MD.SegmentedButton {
                    mdState.size: MD.Enum.XS
                    text: qsTr("AND")
                    checked: sectionRow.currentOp !== WC.LogicOp.LOGIC_OP_OR
                    onClicked: root.model.setLogicOpAt(sectionRow.sectionIndex, WC.LogicOp.LOGIC_OP_AND)
                }

                MD.SegmentedButton {
                    mdState.size: MD.Enum.XS
                    text: qsTr("OR")
                    checked: sectionRow.currentOp === WC.LogicOp.LOGIC_OP_OR
                    onClicked: root.model.setLogicOpAt(sectionRow.sectionIndex, WC.LogicOp.LOGIC_OP_OR)
                }
            }

            MD.SmallIconButton {
                icon.name: MD.Token.icon.add
                onClicked: root.model.appendRuleInGroup(sectionRow.groupId)
            }
            MD.SmallIconButton {
                icon.name: MD.Token.icon.delete
                onClicked: root.model.deleteGroup(sectionRow.groupId)
            }
        }
    }
}
