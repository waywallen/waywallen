#pragma once

namespace waywallen::detail
{

// Match the backend's supported filter types, payloads and condition codes.
template<class Rule>
bool wallpaperFilterRuleValid(const Rule& rule) {
    if (rule.group() < 0) return false;
    const auto stringValid = [&] {
        return rule.hasStringFilter() && static_cast<int>(rule.stringFilter().condition()) >= 1 &&
               static_cast<int>(rule.stringFilter().condition()) <= 4;
    };
    const auto intValid = [&] {
        return rule.hasIntFilter() && static_cast<int>(rule.intFilter().condition()) >= 1 &&
               static_cast<int>(rule.intFilter().condition()) <= 6;
    };
    switch (static_cast<int>(rule.type())) {
    case 1:
    case 2:
    case 8: return stringValid();
    case 3:
        return intValid() && rule.intFilter().value() > 0 &&
               static_cast<int>(rule.intFilter().condition()) <= 2;
    case 5:
    case 6:
    case 7: return intValid();
    case 9:
        return stringValid() ||
               (rule.hasTagFilter() && static_cast<int>(rule.tagFilter().condition()) >= 1 &&
                static_cast<int>(rule.tagFilter().condition()) <= 4);
    default: return false;
    }
}

template<class Rules, class Logics>
bool wallpaperFilterStateValid(const Rules& rules, const Logics& logics) {
    for (const auto& rule : rules) {
        if (! wallpaperFilterRuleValid(rule)) return false;
    }
    for (const auto& logic : logics) {
        if (static_cast<int>(logic.op()) != 1 && static_cast<int>(logic.op()) != 2) return false;
        bool hasA = false;
        bool hasB = false;
        for (const auto& rule : rules) {
            hasA = hasA || rule.group() == logic.groupA();
            hasB = hasB || rule.group() == logic.groupB();
        }
        if (! hasA || ! hasB) return false;
    }
    return true;
}

} // namespace waywallen::detail
