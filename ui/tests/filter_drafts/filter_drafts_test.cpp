#include "filter.qpb.h"
#include "waywallen/model/wallpaper_filter_validation.hpp"
#include <QCoreApplication>
#include <QProtobufSerializer>
#include <iostream>
#include <stdexcept>
#include <vector>

using namespace waywallen::control::v1;
using Type            = WallpaperFilterTypeGadget::WallpaperFilterType;
using StringCondition = StringConditionGadget::StringCondition;
using IntCondition    = IntConditionGadget::IntCondition;
using waywallen::detail::wallpaperFilterRuleValid;
using waywallen::detail::wallpaperFilterStateValid;

void require(bool value, const char* message) {
    if (! value) throw std::runtime_error(message);
}

auto rule(int type, int condition = 1, qint64 value = 7) -> WallpaperFilterRule {
    WallpaperFilterRule result;
    result.setType(static_cast<Type>(type));
    result.setGroup(0);
    if (type == 3 || type == 5 || type == 6 || type == 7) {
        WallpaperIntFilter filter;
        filter.setCondition(static_cast<IntCondition>(condition));
        filter.setValue(value);
        result.setIntFilter(filter);
    } else if (type == 9) {
        WallpaperTagFilter filter;
        filter.setCondition(static_cast<StringCondition>(condition));
        filter.setValues({ "Nature" });
        result.setTagFilter(filter);
    } else {
        WallpaperStringFilter filter;
        filter.setCondition(static_cast<StringCondition>(condition));
        filter.setValue("Nature");
        result.setStringFilter(filter);
    }
    return result;
}

int main(int argc, char** argv) {
    QCoreApplication app(argc, argv);
    try {
        QProtobufSerializer serializer;
        for (int type : { 1, 2, 3, 5, 6, 7, 8, 9 }) {
            auto       valid  = rule(type);
            const auto before = valid.serialize(&serializer);
            require(wallpaperFilterRuleValid(valid), "valid configured rule rejected");
            require(valid.serialize(&serializer) == before, "validation mutated payload");
            auto unspecified = rule(type, 0);
            require(! wallpaperFilterRuleValid(unspecified), "unspecified condition accepted");
            require(! wallpaperFilterRuleValid(rule(type, 99)), "unknown condition accepted");
        }
        for (int condition : { 1, 2, 3, 4 }) {
            for (int type : { 1, 2, 8, 9 })
                require(wallpaperFilterRuleValid(rule(type, condition)),
                        "string condition rejected");
        }
        for (int condition : { 1, 2, 3, 4, 5, 6 }) {
            for (int type : { 5, 6, 7 })
                require(wallpaperFilterRuleValid(rule(type, condition)),
                        "numeric condition rejected");
        }
        require(wallpaperFilterRuleValid(rule(3, 2)), "library exclusion rejected");
        require(! wallpaperFilterRuleValid(rule(3, 1, 0)), "zero library accepted");
        require(! wallpaperFilterRuleValid(rule(3, 1, -1)), "negative library accepted");
        require(! wallpaperFilterRuleValid(rule(3, 3)), "unsupported library condition accepted");
        require(! wallpaperFilterRuleValid(WallpaperFilterRule {}), "unconfigured draft accepted");
        require(! wallpaperFilterRuleValid(rule(99)), "unknown filter type accepted");
        auto legacyTag = rule(1);
        legacyTag.setType(static_cast<Type>(9));
        require(wallpaperFilterRuleValid(legacyTag), "legacy string-backed tag rejected");
        auto wrongPayload = rule(5);
        wrongPayload.setType(static_cast<Type>(1));
        require(! wallpaperFilterRuleValid(wrongPayload), "wrong payload type accepted");
        auto negativeGroup = rule(1);
        negativeGroup.setGroup(-1);
        require(! wallpaperFilterRuleValid(negativeGroup), "negative group accepted");
        auto first  = rule(1);
        auto second = rule(5);
        second.setGroup(2);
        std::vector<WallpaperFilterRule> rules { first, second };
        std::vector<FilterLogic>         logics;
        require(wallpaperFilterStateValid(rules, logics), "independent groups rejected");
        FilterLogic link;
        link.setGroupA(0);
        link.setGroupB(2);
        for (int op : { 1, 2 }) {
            link.setOp(static_cast<LogicOpGadget::LogicOp>(op));
            logics = { link };
            require(wallpaperFilterStateValid(rules, logics), "valid group link rejected");
        }
        link.setGroupB(3);
        logics = { link };
        require(! wallpaperFilterStateValid(rules, logics), "dangling group accepted");
        link.setGroupB(2);
        link.setOp(static_cast<LogicOpGadget::LogicOp>(0));
        logics = { link };
        require(! wallpaperFilterStateValid(rules, logics), "unspecified logic accepted");
        require(wallpaperFilterStateValid(std::vector<WallpaperFilterRule> {},
                                          std::vector<FilterLogic> {}),
                "cleared state rejected");
        std::cout << "PASS: eight types, supported conditions, incomplete drafts, legacy tags, "
                     "group references and read-only validation\n";
    } catch (const std::exception& error) {
        std::cerr << "FAIL: " << error.what() << '\n';
        return 1;
    }
}
