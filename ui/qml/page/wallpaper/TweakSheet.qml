pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

MD.BottomSheet {
    id: control

    required property Item popupParent
    required property var tweak

    dim: false
    dismissOnDragDown: true
    maxSheetWidth: 560
    parent: popupParent
    sheetType: MD.Enum.BottomSheetModal
    z: 25

    ColumnLayout {
        spacing: 0
        width: control.sheetWidth

        ColumnLayout {
            Layout.bottomMargin: 16
            Layout.fillWidth: true
            Layout.leftMargin: 16
            Layout.rightMargin: 16
            spacing: 16

            MD.Text {
                Layout.fillWidth: true
                color: MD.Token.color.on_surface
                elide: Text.ElideRight
                maximumLineCount: 1
                text: qsTr("Tweak")
                typescale: MD.Token.typescale.title_medium
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8

                MD.Text {
                    Layout.fillWidth: true
                    color: MD.Token.color.on_surface_variant
                    text: qsTr("Layout")
                    typescale: MD.Token.typescale.label_medium
                }
                MD.SegmentedButtonGroup {
                    size: MD.Enum.XS

                    MD.SegmentedButton {
                        checked: control.tweak.viewMode === control.tweak.viewGrid
                        text: qsTr("Grid")

                        onCheckedChangeRequested: control.tweak.setViewMode(control.tweak.viewGrid)
                    }
                    MD.SegmentedButton {
                        checked: control.tweak.viewMode === control.tweak.viewMasonry
                        text: qsTr("Masonry")

                        onCheckedChangeRequested: control.tweak.setViewMode(control.tweak.viewMasonry)
                    }
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8
                visible: control.tweak.viewMode === control.tweak.viewGrid

                MD.Text {
                    Layout.fillWidth: true
                    color: MD.Token.color.on_surface_variant
                    text: qsTr("Aspect ratio")
                    typescale: MD.Token.typescale.label_medium
                }
                MD.SegmentedButtonGroup {
                    size: MD.Enum.XS

                    MD.SegmentedButton {
                        checked: Math.abs(control.tweak.itemAspectRatio - 1) < 0.001
                        text: "1:1"

                        onCheckedChangeRequested: control.tweak.setItemAspectRatio(1)
                    }
                    MD.SegmentedButton {
                        checked: Math.abs(control.tweak.itemAspectRatio - 4 / 3) < 0.001
                        text: "4:3"

                        onCheckedChangeRequested: control.tweak.setItemAspectRatio(4 / 3)
                    }
                    MD.SegmentedButton {
                        checked: Math.abs(control.tweak.itemAspectRatio - 16 / 9) < 0.001
                        text: "16:9"

                        onCheckedChangeRequested: control.tweak.setItemAspectRatio(16 / 9)
                    }
                    MD.SegmentedButton {
                        checked: Math.abs(control.tweak.itemAspectRatio - 9 / 16) < 0.001
                        text: "9:16"

                        onCheckedChangeRequested: control.tweak.setItemAspectRatio(9 / 16)
                    }
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4

                RowLayout {
                    Layout.fillWidth: true

                    MD.Text {
                        Layout.fillWidth: true
                        color: MD.Token.color.on_surface_variant
                        text: qsTr("Size")
                        typescale: MD.Token.typescale.label_medium
                    }
                }
                W.ValueSlider {
                    Layout.fillWidth: true
                    from: control.tweak.minimumItemSize
                    snapMode: MD.Slider.SnapAlways
                    stepSize: control.tweak.itemSizeStep
                    to: control.tweak.maximumItemSize
                    value: control.tweak.itemSize
                    valueMaxText: qsTr("%1 px").arg(Math.round(to))
                    valueText: qsTr("%1 px").arg(Math.round(value))

                    onValueChangeRequested: value => control.tweak.setItemSize(value)
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4

                MD.Text {
                    Layout.fillWidth: true
                    color: MD.Token.color.on_surface_variant
                    text: qsTr("Spacing")
                    typescale: MD.Token.typescale.label_medium
                }
                W.ValueSlider {
                    Layout.fillWidth: true
                    from: control.tweak.minimumItemSpacing
                    snapMode: MD.Slider.SnapAlways
                    stepSize: control.tweak.itemSpacingStep
                    to: control.tweak.maximumItemSpacing
                    value: control.tweak.itemSpacing
                    valueMaxText: qsTr("%1 px").arg(Math.round(to))
                    valueText: qsTr("%1 px").arg(Math.round(value))

                    onValueChangeRequested: value => control.tweak.setItemSpacing(value)
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 8
                visible: control.tweak.viewMode === control.tweak.viewGrid

                MD.Text {
                    Layout.fillWidth: true
                    color: MD.Token.color.on_surface_variant
                    text: qsTr("Fill mode")
                    typescale: MD.Token.typescale.label_medium
                }
                MD.SegmentedButtonGroup {
                    size: MD.Enum.XS

                    MD.SegmentedButton {
                        checked: control.tweak.layoutMode === control.tweak.layoutFillCell
                        text: qsTr("Fill cell")

                        onCheckedChangeRequested: control.tweak.setLayoutMode(control.tweak.layoutFillCell)
                    }
                    MD.SegmentedButton {
                        checked: control.tweak.layoutMode === control.tweak.layoutFixed
                        text: qsTr("Fixed")

                        onCheckedChangeRequested: control.tweak.setLayoutMode(control.tweak.layoutFixed)
                    }
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4

                MD.Text {
                    color: MD.Token.color.on_surface_variant
                    text: qsTr("Image decode cache")
                    typescale: MD.Token.typescale.label_medium
                }
                W.ValueSlider {
                    Layout.fillWidth: true
                    from: 0
                    snapMode: MD.Slider.SnapAlways
                    stepSize: 1
                    to: control.tweak.imageCacheOptions.length - 1
                    value: Math.max(0, control.tweak.imageCacheOptions.indexOf(control.tweak.imageCacheMiB))
                    valueMaxText: qsTr("512 MiB")
                    valueText: qsTr("%1 MiB").arg(control.tweak.imageCacheOptions[Math.round(value)])

                    onValueChangeRequested: value => control.tweak.setImageCacheMiB(control.tweak.imageCacheOptions[Math.round(value)])
                }
                MD.Text {
                    Layout.fillWidth: true
                    color: MD.Token.color.on_surface_variant
                    text: qsTr("Lower values use less memory but may increase CPU usage when playing animated thumbnails. Set to 0 to disable caching.")
                    typescale: MD.Token.typescale.body_small
                    wrapMode: Text.WordWrap
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4

                MD.Text {
                    color: MD.Token.color.on_surface_variant
                    text: qsTr("Image GPU cache")
                    typescale: MD.Token.typescale.label_medium
                }
                W.ValueSlider {
                    Layout.fillWidth: true
                    from: 0
                    snapMode: MD.Slider.SnapAlways
                    stepSize: 1
                    to: control.tweak.imageTextureCacheOptions.length - 1
                    value: Math.max(0, control.tweak.imageTextureCacheOptions.indexOf(control.tweak.imageTextureCacheMiB))
                    valueMaxText: qsTr("256 MiB")
                    valueText: qsTr("%1 MiB").arg(control.tweak.imageTextureCacheOptions[Math.round(value)])

                    onValueChangeRequested: value => control.tweak.setImageTextureCacheMiB(control.tweak.imageTextureCacheOptions[Math.round(value)])
                }
                MD.Text {
                    Layout.fillWidth: true
                    color: MD.Token.color.on_surface_variant
                    text: qsTr("Lower values use less GPU memory but may require more texture uploads. Set to 0 to disable caching unused textures.")
                    typescale: MD.Token.typescale.body_small
                    wrapMode: Text.WordWrap
                }
            }
        }
    }
}
