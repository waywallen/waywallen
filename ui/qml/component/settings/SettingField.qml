pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Templates as T
import QtQuick.Layouts
import Qcm.Material as MD
import waywallen.ui as W

// Renders one schema-driven control. The schema dict comes from
// `RendererPluginListQuery.renderers[i].settings[j]`.
//
// `schema.label` does not always carry human text. When a manifest
// declares a translated `label` it arrives as a PluginMessage object;
// when it does not, `pluginMessageFromPb` substitutes the raw
// `label_key` string (`renderer_query.cpp`) and `W.I18n.tr` returns it
// unchanged -- so a manifest still on the deprecated `label_key` form
// puts `settings.video.hwdec` where the name belongs. Only that second,
// string form is examined: a dotted, space-free value there is treated
// as an unresolved key and falls through to the `key` transform. A
// resolved PluginMessage is printed as it comes, whatever it contains.
ColumnLayout {
    id: root

    required property var schema
    // Stringified value (matches the wire format). When unset we use
    // schema.default_value so the form always has a base.
    property string value: ""

    // Named `committed` rather than `valueChanged` so it doesn't clash
    // with the `value` property's auto-generated notify signal.
    signal committed(string key, string newValue)

    spacing: 4

    readonly property int kU32: 1
    readonly property int kF32: 2
    readonly property int kString: 3
    readonly property int kBool: 4
    readonly property int kI32: 5

    // A PluginMessage carries a msgid the catalog resolved; a bare string
    // is the raw manifest value `pluginMessageFromPb` fell back to.
    function isPluginMessage(v) {
        return v !== null && typeof v === "object" && !!v.msgid;
    }

    // A dotted, space-free token is an unresolved i18n key, not a label.
    // Never applied to text that came from a PluginMessage.
    function isUnresolvedKey(source, s) {
        return !root.isPluginMessage(source) && s.indexOf(".") >= 0 && !/\s/.test(s);
    }

    readonly property string label: {
        const translated = W.I18n.tr(schema.label);
        if (translated.length > 0 && !root.isUnresolvedKey(schema.label, translated))
            return translated;
        const sk = schema.label_key || "";
        if (sk.length > 0 && !root.isUnresolvedKey(sk, sk))
            return sk;
        const raw = schema.key || "";
        if (raw.length === 0)
            return "";
        return raw.split("_").map(function (p) {
            return p.length === 0 ? p : p[0].toUpperCase() + p.slice(1);
        }).join(" ");
    }

    readonly property string description: {
        const translated = W.I18n.tr(schema.description);
        if (translated.length > 0 && !root.isUnresolvedKey(schema.description, translated))
            return translated;
        const dk = schema.description_key || "";
        return root.isUnresolvedKey(dk, dk) ? "" : dk;
    }
    // Bare http(s) URLs in the description become clickable links
    readonly property string descriptionHtml: {
        const c = String(MD.Token.color.primary);
        return (root.description || "").replace(/(https?:\/\/[^\s]+)/g,
            '<a href="$1"><font color="' + c + '">$1</font></a>');
    }
    readonly property bool needsRestart: schema.identity === true

    readonly property bool isRenderNode: schema.key === "render_node"
    readonly property bool isResolution: schema.key === "resolution"

    // Wire enum value → display label. Kept in sync with
    // <waywallen-bridge/resolution.h> WW_RESOLUTION_*.
    readonly property var resolutionPresets: [
        { value: "-1", label: qsTr("Custom") },
        { value: "0", label: qsTr("Origin") },
        { value: "1", label: "720p" },
        { value: "2", label: "1080p" },
        { value: "3", label: "1440p" },
        { value: "4", label: "2160p" }
    ]

    readonly property bool isTextField: {
        const t = schema.type;
        if (t === kBool)
            return false;
        if (root.isRenderNode || root.isResolution)
            return false;
        if (t === kString)
            return !(schema.choices && schema.choices.length > 0);
        // Integer settings fall back to a dense textfield when the range is too
        // wide to be useful on a slider.
        if (root._isIntegerType(t))
            return !(_hasNumericRange() && root._intRangeFitsSlider());
        return !_hasNumericRange();
    }

    function _hasNumericRange() {
        const lo = schema.min || "";
        const hi = schema.max || "";
        return lo.length > 0 && hi.length > 0;
    }

    // Integer sliders feel right up to a ~1000-unit span; beyond that the
    // step granularity gets coarse and a textfield is friendlier.
    function _intRangeFitsSlider() {
        const lo = root._toFloat(root.schema.min, 0);
        const hi = root._toFloat(root.schema.max, 0);
        return (hi - lo) <= 1000;
    }

    function _toFloat(s, fallback) {
        const n = parseFloat(s);
        return isNaN(n) ? fallback : n;
    }

    function _isIntegerType(t) {
        return t === root.kU32 || t === root.kI32;
    }

    function _stepFor() {
        const s = schema.step || "";
        if (s.length > 0) {
            return root._toFloat(s, root._isIntegerType(schema.type) ? 1 : 0.01);
        }
        return root._isIntegerType(schema.type) ? 1 : 0.01;
    }

    // Forward the user's edit but never write `root.value` ourselves —
    // `value` stays bound to the parent's expression so external pushes
    // (reset, daemon SettingsChanged) always reach us. The Connections
    // inside each control component re-syncs the visual state when the
    // user's prior interaction broke its declarative binding.
    function _emit(v) {
        root.committed(schema.key, v);
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: 6
        visible: !root.isTextField || root.needsRestart

        MD.Text {
            Layout.fillWidth: true
            visible: !root.isTextField
            typescale: MD.Token.typescale.label_large
            color: MD.Token.color.on_surface
            text: root.label
        }

        // Identity-flag affordance: a small icon + tooltip warning the
        // user that changing this won't take effect on the live
        // renderer; daemon respawns it on next apply.
        MD.Icon {
            visible: root.needsRestart
            name: MD.Token.icon.restart_alt
            size: 16
            color: MD.Token.color.on_surface_variant

            HoverHandler {
                id: hovered
            }
            MD.ToolTip.visible: hovered.hovered
            MD.ToolTip.text: qsTr("Requires renderer restart")
        }
    }

    Loader {
        id: control
        Layout.fillWidth: true

        sourceComponent: {
            if (root.isRenderNode)
                return renderNodeField;
            if (root.isResolution)
                return resolutionField;
            switch (root.schema.type) {
            case root.kBool:
                return boolField;
            case root.kU32:
            case root.kI32:
                return (root._hasNumericRange() && root._intRangeFitsSlider())
                    ? sliderField : numericField;
            case root.kF32:
                return root._hasNumericRange() ? sliderField : numericField;
            case root.kString:
                if (root.schema.choices && root.schema.choices.length > 0)
                    return choiceField;
                return stringField;
            default:
                return stringField;
            }
        }
    }

    MD.Text {
        Layout.fillWidth: true
        visible: root.description.length > 0
        text: root.descriptionHtml
        textFormat: Text.StyledText
        onLinkActivated: link => MD.Util.openUrlExternally(link)
        typescale: MD.Token.typescale.body_small
        color: MD.Token.color.on_surface_variant
        wrapMode: Text.WordWrap
    }

    Component {
        id: boolField
        RowLayout {
            spacing: 8
            MD.Switch {
                id: sw
                checked: root.value === "true"
                onToggled: root._emit(checked ? "true" : "false")
                Connections {
                    target: root
                    function onValueChanged() {
                        const c = root.value === "true";
                        if (sw.checked !== c) sw.checked = c;
                    }
                }
            }
            Item {
                Layout.fillWidth: true
            }
        }
    }

    Component {
        id: sliderField
        W.ValueSlider {
            id: slider
            Layout.fillWidth: true
            from: root._toFloat(root.schema.min, 0)
            to: root._toFloat(root.schema.max, 1)
            stepSize: root._stepFor()
            snapMode: T.Slider.SnapAlways
            value: root._toFloat(root.value, from)
            valueText: displayValue(value)
            valueMaxText: {
                const minText = displayValue(from);
                const maxText = displayValue(to);
                return minText.length > maxText.length ? minText : maxText;
            }
            function displayValue(v) {
                return root._isIntegerType(root.schema.type) ? Math.round(v).toString() : Number(v).toFixed(2);
            }
            onMoved: {
                if (root._isIntegerType(root.schema.type)) {
                    root._emit(Math.round(value).toString());
                } else {
                    root._emit(value.toString());
                }
            }
            Connections {
                target: root
                function onValueChanged() {
                    const v = root._toFloat(root.value, slider.from);
                    if (slider.value !== v) slider.value = v;
                }
            }
        }
    }

    Component {
        id: numericField
        MD.TextField {
            id: tf
            text: root.value
            placeholderText: root.label
            mdState.size: MD.Enum.S
            inputMethodHints: root.schema.type === root.kU32 ? Qt.ImhDigitsOnly : Qt.ImhFormattedNumbersOnly
            validator: root._isIntegerType(root.schema.type) ? intValidator : doubleValidator
            onTextEdited: root._emit(text)
            onEditingFinished: root._emit(text)

            IntValidator {
                id: intValidator
                bottom: root.schema.type === root.kU32 ? 0 : -2147483648
            }
            DoubleValidator {
                id: doubleValidator
                notation: DoubleValidator.StandardNotation
            }

            Connections {
                target: root
                function onValueChanged() {
                    if (tf.text !== root.value) tf.text = root.value;
                }
            }
        }
    }

    Component {
        id: stringField
        MD.TextField {
            id: stf
            text: root.value
            placeholderText: root.label
            mdState.size: MD.Enum.S
            // Commit on every edit (incl. paste) so Apply enables without
            // needing an explicit Enter/focus-out.
            onTextEdited: root._emit(text)
            onEditingFinished: root._emit(text)
            Connections {
                target: root
                function onValueChanged() {
                    if (stf.text !== root.value) stf.text = root.value;
                }
            }
        }
    }

    Component {
        id: choiceField
        MD.ComboBox {
            id: cb
            mdState.size: MD.Enum.S
            model: root.schema.choices
            currentIndex: Math.max(0, root.schema.choices.indexOf(root.value))
            onActivated: root._emit(root.schema.choices[currentIndex])
            Connections {
                target: root
                function onValueChanged() {
                    const i = Math.max(0, root.schema.choices.indexOf(root.value));
                    if (cb.currentIndex !== i) cb.currentIndex = i;
                }
            }
        }
    }

    // Resolution chip list. Wire value is the WW_RESOLUTION_* enum
    // (string-encoded), filtered against the schema's [min, max] range.
    Component {
        id: resolutionField
        Flow {
            spacing: 6

            readonly property int loValue: {
                const n = parseInt(root.schema.min || "0", 10);
                return isNaN(n) ? 0 : n;
            }
            readonly property int hiValue: {
                const n = parseInt(root.schema.max || "4", 10);
                return isNaN(n) ? 4 : n;
            }

            Repeater {
                model: root.resolutionPresets
                delegate: MD.FilterChip {
                    id: resChip
                    required property var modelData
                    readonly property int modelValue: parseInt(modelData.value, 10)
                    visible: modelValue >= parent.loValue && modelValue <= parent.hiValue
                    text: modelData.label
                    checked: root.value === modelData.value
                    onClicked: root._emit(modelData.value)
                    Connections {
                        target: root
                        function onValueChanged() {
                            const c = root.value === resChip.modelData.value;
                            if (resChip.checked !== c) resChip.checked = c;
                        }
                    }
                }
            }
        }
    }

    // GPU picker: empty value ⇒ "Auto" (renderer picks a device); otherwise
    // the value is a DRM render-node path matching one Gpu.renderNode.
    // Reads the live GPU list from App.gpuManager (populated on connect).
    Component {
        id: renderNodeField
        Flow {
            spacing: 6

            MD.FilterChip {
                id: autoChip
                text: qsTr("Auto")
                checked: root.value === ""
                onClicked: root._emit("")
                Connections {
                    target: root
                    function onValueChanged() {
                        const c = root.value === "";
                        if (autoChip.checked !== c) autoChip.checked = c;
                    }
                }
            }

            Repeater {
                model: W.App.gpuManager ? W.App.gpuManager.gpus : []
                delegate: MD.FilterChip {
                    id: gpuChip
                    required property var modelData
                    text: (modelData.driver || "drm")
                        + " " + modelData.renderMajor + ":" + modelData.renderMinor
                    checked: root.value === modelData.renderNode
                    enabled: modelData.renderNode.length > 0
                    onClicked: root._emit(modelData.renderNode)
                    MD.ToolTip.visible: gpuChip.hovered
                    MD.ToolTip.delay: 300
                    MD.ToolTip.text: W.GpuDetails.toolTipText(gpuChip.modelData)
                    Connections {
                        target: root
                        function onValueChanged() {
                            const c = root.value === gpuChip.modelData.renderNode;
                            if (gpuChip.checked !== c) gpuChip.checked = c;
                        }
                    }
                }
            }
        }
    }
}
