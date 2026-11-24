#include "waywallen/external_link.hpp"
#include <iostream>

using waywallen::detail::externalLinkAllowed;

int main() {
    int failed = 0;
    const auto check = [&](const char* link, bool expected) {
        if (externalLinkAllowed(QUrl(QString::fromUtf8(link))) != expected) {
            std::cerr << "unexpected result for " << link << '\n';
            ++failed;
        }
    };
    check("http://example.com", true);
    check("https://steamcommunity.com/sharedfiles/filedetails/?id=1", true);
    check("HTTPS://Example.com/Path", true);
    check("HtTp://example.com", true);
    check("file:///etc/passwd", false);
    check("file://host/share", false);
    check("steam://run/440", false);
    check("javascript:alert(1)", false);
    check("mailto:someone@example.com", false);
    check("ftp://example.com", false);
    check("/home/user/file", false);
    check("example.com", false);
    check("https://", false);
    check("", false);
    return failed == 0 ? 0 : 1;
}
