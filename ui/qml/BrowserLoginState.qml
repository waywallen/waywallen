pragma Singleton
import QtCore
import QtQml

QtObject {
    id: root

    property string helperChoices: "{}"
    readonly property var choices: {
        try {
            const parsed = JSON.parse(helperChoices);
            return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : ({});
        } catch (error) {
            return ({});
        }
    }

    function helperId(sourceId, actionId, helpers, defaultId) {
        const selected = choices[JSON.stringify([sourceId, actionId])] || "";
        if (helpers.some(helper => helper.id === selected))
            return selected;
        if (helpers.some(helper => helper.id === defaultId))
            return defaultId;
        return helpers.length > 0 ? helpers[0].id : "";
    }

    function selectHelper(sourceId, actionId, helperId) {
        const updated = Object.assign({}, choices);
        updated[JSON.stringify([sourceId, actionId])] = helperId;
        helperChoices = JSON.stringify(updated);
    }

    readonly property Settings _settings: Settings {
        category: "BrowserLogin"
        property alias helperChoices: root.helperChoices
    }
}
