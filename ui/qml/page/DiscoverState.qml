pragma Singleton
pragma ComponentBehavior: Bound
import QtCore
import QtQml

QtObject {
    id: root

    property string settingsCategory: "DiscoverView"
    signal filtersChanged(string sourceId)

    readonly property Settings settings: Settings {
        category: root.settingsCategory
        property string selectedSourceId: ""
        property string filtersJson: "{}"
        property string matchModesJson: "{}"
        property string excludedTagsJson: "{}"
        property string hiddenBadgesJson: "{}"
    }

    function selectedSourceId() {
        return settings.selectedSourceId;
    }

    function hiddenBadgesFor(sourceId) {
        try {
            const state = JSON.parse(settings.hiddenBadgesJson);
            const values = state?.[sourceId];
            return Array.isArray(values) ? normalize(values) : [];
        } catch (error) {
            return [];
        }
    }
    function setBadgeVisible(sourceId, id, visible) {
        if (sourceId.length === 0 || id.length === 0)
            return;
        const next = hiddenBadgesFor(sourceId).filter(value => value !== id);
        if (!visible)
            next.push(id);
        settings.hiddenBadgesJson = valuesJson(settings.hiddenBadgesJson, sourceId, next);
    }
    function visibleBadges(sourceId, badges, definitions) {
        const allowed = (definitions ?? []).map(definition => definition.id);
        const hidden = hiddenBadgesFor(sourceId);
        return (badges ?? []).filter(badge => allowed.indexOf(badge.id) >= 0
            && hidden.indexOf(badge.id) < 0).map(badge => badge.text);
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
        setFilterSelectionFor(sourceId, filterValuesFor(sourceId), values);
    }

    function valuesJson(json, sourceId, values) {
        let state = {};
        try {
            const parsed = JSON.parse(json);
            if (parsed && typeof parsed === "object" && !Array.isArray(parsed))
                state = parsed;
        } catch (error) {
        }
        const normalized = normalize(values);
        if (normalized.length > 0)
            state[sourceId] = normalized;
        else
            delete state[sourceId];
        return JSON.stringify(state);
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
        if (settings.matchModesJson === next)
            return;
        settings.matchModesJson = next;
        filtersChanged(sourceId);
    }

    function setFilterValuesFor(sourceId, values) {
        setFilterSelectionFor(sourceId, values, excludedTagsFor(sourceId));
    }

    function setFilterSelectionFor(sourceId, values, excluded) {
        if (sourceId.length === 0)
            return;
        const included = normalize(values);
        const exclusions = normalize(excluded).filter(value => included.indexOf(value) < 0);
        const nextValues = valuesJson(settings.filtersJson, sourceId, included);
        const nextExcluded = valuesJson(settings.excludedTagsJson, sourceId, exclusions);
        if (settings.filtersJson === nextValues && settings.excludedTagsJson === nextExcluded)
            return;
        settings.filtersJson = nextValues;
        settings.excludedTagsJson = nextExcluded;
        filtersChanged(sourceId);
    }
}
