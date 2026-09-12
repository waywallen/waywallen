pragma Singleton
import QtQml
import waywallen.ui

QtObject {
    readonly property var revision: PluginTranslations.revision

    function tr(value) {
        const dependency = revision;
        return PluginTranslations.translate(value);
    }

    function optionLabel(options, value, fallback) {
        const raw = value === undefined || value === null ? "" : String(value);
        const option = (options ?? []).find(option => String(option.value) === raw);
        const label = tr(option?.labelText ?? option?.label ?? fallback ?? raw);
        return label.length > 0 ? label : raw;
    }

    // Library rows carry no options of their own: the value stored in the
    // DB is the key, and `labels` only maps it to what a source called it.
    function valueLabel(labels, value) {
        const raw = value === undefined || value === null ? "" : String(value);
        return optionLabel(null, raw, labels?.[raw]);
    }
}
