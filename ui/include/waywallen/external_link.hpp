#pragma once

#include <QString>
#include <QUrl>

namespace waywallen::detail
{

// Links from wallpaper descriptions, plugin text and remote metadata are
// untrusted; only web pages may be handed to the desktop.
inline bool externalLinkAllowed(const QUrl& url) {
    if (! url.isValid() || url.host().isEmpty()) return false;
    const auto scheme = url.scheme();
    return scheme.compare(QStringLiteral("http"), Qt::CaseInsensitive) == 0 ||
           scheme.compare(QStringLiteral("https"), Qt::CaseInsensitive) == 0;
}

} // namespace waywallen::detail
