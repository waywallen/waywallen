pragma Singleton
pragma ComponentBehavior: Bound
import QtCore
import QtQml

QtObject {
    id: root

    readonly property var fields: ["tag", "content_rating", "wp_type"]
    readonly property var rules: readRules(settings.rulesJson)
    readonly property var _compiledRules: rules.map(rule => ({
        rule,
        pattern: [...rule.value.toLowerCase()]
    }))
    readonly property bool enabled: settings.enabled
    property int serial: 0
    readonly property Settings settings: Settings {
        category: "WallpaperBadges"
        property bool enabled: true
        property string rulesJson: '[{"id":"ai","enabled":true,"field":"tag","value":"AI-generated","text":"AI"}]'
    }

    function readRules(json) {
        try {
            const values = JSON.parse(json);
            if (!Array.isArray(values))
                return [];
            return values.slice(0, 32).filter(rule => validRule(rule));
        } catch (error) {
            return [];
        }
    }
    function validRule(rule) {
        return rule && typeof rule.id === "string" && rule.id.length > 0
            && fields.indexOf(rule.field) >= 0 && typeof rule.value === "string"
            && rule.value.trim().length > 0 && rule.value.length <= 256
            && typeof rule.text === "string" && rule.text.trim().length > 0
            && rule.text.length <= 64 && !/[\x00-\x1f\x7f]/.test(rule.text);
    }
    function setEnabled(value) {
        settings.enabled = value;
    }
    function saveRule(id, field, value, text, enabled = true) {
        value = String(value).trim();
        text = String(text).trim();
        const next = rules.slice();
        const index = next.findIndex(rule => rule.id === id);
        if (index < 0 && next.length >= 32)
            return false;
        const rule = { id: id || Date.now().toString(36) + "-" + (++serial), field, value, text, enabled };
        if (!validRule(rule))
            return false;
        if (index >= 0)
            next[index] = rule;
        else
            next.push(rule);
        settings.rulesJson = JSON.stringify(next);
        return true;
    }
    function setRuleEnabled(id, enabled) {
        const rule = rules.find(rule => rule.id === id);
        if (rule)
            saveRule(id, rule.field, rule.value, rule.text, enabled);
    }
    function removeRule(id) {
        settings.rulesJson = JSON.stringify(rules.filter(rule => rule.id !== id));
    }
    function matchesPattern(value, pattern) {
        const chars = [...String(value ?? "").toLowerCase()];
        let index = 0;
        let patternIndex = 0;
        let star = -1;
        let retry = 0;
        while (index < chars.length) {
            if (pattern[patternIndex] === "*") {
                star = patternIndex++;
                retry = index;
            } else if (pattern[patternIndex] === "?" || pattern[patternIndex] === chars[index]) {
                ++index;
                ++patternIndex;
            } else if (star >= 0) {
                patternIndex = star + 1;
                index = ++retry;
            } else {
                return false;
            }
        }
        while (pattern[patternIndex] === "*")
            ++patternIndex;
        return patternIndex === pattern.length;
    }
    function labelsFor(item) {
        if (!enabled || !item)
            return [];
        const labels = [];
        for (const compiled of _compiledRules) {
            const rule = compiled.rule;
            if (rule.enabled === false)
                continue;
            let matched = false;
            if (rule.field === "tag") {
                for (const tag of item.tags ?? []) {
                    if (matchesPattern(tag, compiled.pattern)) {
                        matched = true;
                        break;
                    }
                }
            } else {
                const value = rule.field === "content_rating" ? item.contentRating : item.wpType;
                matched = matchesPattern(value, compiled.pattern);
            }
            if (matched && labels.indexOf(rule.text) < 0)
                labels.push(rule.text);
        }
        return labels;
    }
}
