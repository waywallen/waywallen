pragma ComponentBehavior: Bound
import QtCore
import QtQml

QtObject {
    id: root

    property string settingsCategory: "DiscoverView"

    readonly property Settings settings: Settings {
        category: root.settingsCategory
        property string selectedSourceId: ""
        property string filtersJson: "{}"
        property string matchModesJson: "{}"
        property string excludedTagsJson: "{}"
    }

    function selectedSourceId() {
        return settings.selectedSourceId;
    }

    function setSelectedSourceId(sourceId) {
        const value = String(sourceId ?? "");
        if (settings.selectedSourceId !== value)
            settings.selectedSourceId = value;
    }

    function normalize(values) {
        const seen = {};
        const out = [];
        for (const raw of values ?? []) {
            const value = String(raw);
            if (value.length === 0 || seen[value] === true)
                continue;
            seen[value] = true;
            out.push(value);
        }
        return out;
    }

    function readFilters() {
        try {
            const state = JSON.parse(settings.filtersJson);
            if (state && typeof state === "object" && !Array.isArray(state))
                return state;
        } catch (error) {
        }
        return {};
    }

    function filterValuesFor(sourceId) {
        const values = readFilters()[sourceId];
        return Array.isArray(values) ? normalize(values) : [];
    }

    function readMatchModes() {
        try {
            const state = JSON.parse(settings.matchModesJson);
            if (state && typeof state === "object" && !Array.isArray(state))
                return state;
        } catch (error) {
        }
        return {};
    }

    function excludedTagsFor(sourceId) {
        try {
            const state = JSON.parse(settings.excludedTagsJson);
            const values = state?.[sourceId];
            return Array.isArray(values) ? normalize(values) : [];
        } catch (error) {
            return [];
        }
    }

    function setExcludedTagsFor(sourceId, values) {
        if (sourceId.length === 0)
            return;
        let state = {};
        try {
            const parsed = JSON.parse(settings.excludedTagsJson);
            if (parsed && typeof parsed === "object" && !Array.isArray(parsed))
                state = parsed;
        } catch (error) {
        }
        const normalized = normalize(values);
        if (normalized.length > 0)
            state[sourceId] = normalized;
        else
            delete state[sourceId];
        settings.excludedTagsJson = JSON.stringify(state);
    }

    function matchModesFor(sourceId) {
        const value = readMatchModes()[sourceId];
        return value && typeof value === "object" && !Array.isArray(value) ? value : {};
    }

    function setMatchModesFor(sourceId, modes) {
        if (sourceId.length === 0)
            return;
        const state = readMatchModes();
        state[sourceId] = modes;
        const next = JSON.stringify(state);
        if (settings.matchModesJson !== next)
            settings.matchModesJson = next;
    }

    function setFilterValuesFor(sourceId, values) {
        if (sourceId.length === 0)
            return;
        const state = readFilters();
        const normalized = normalize(values);
        if (normalized.length > 0)
            state[sourceId] = normalized;
        else
            delete state[sourceId];
        const next = JSON.stringify(state);
        if (settings.filtersJson !== next)
            settings.filtersJson = next;
    }
}
