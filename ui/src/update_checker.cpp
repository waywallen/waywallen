module;
#include "waywallen/update_checker.moc.h"

module waywallen;
import qextra.qt;
import rstd.cppstd;
import :update_checker;
import :app;

using namespace Qt::Literals::StringLiterals;

namespace waywallen
{

namespace
{

#if LITO_FEAT_UPDATE_CHECK
constexpr bool kSupported = true;
#else
constexpr bool kSupported = false;
#endif

constexpr auto kEnabledKey       = "update/checkEnabled";
constexpr auto kLastCheckTimeKey = "update/lastCheckTime";
constexpr auto kLatestVersionKey = "update/latestVersion";
constexpr auto kReleaseUrlKey    = "update/releaseUrl";
constexpr auto kLatestReleaseApi =
    "https://api.github.com/repos/waywallen/waywallen/releases/latest";
constexpr auto kReleasesPage    = "https://github.com/waywallen/waywallen/releases/latest";
constexpr auto kCheckInterval   = std::chrono::hours(24);
constexpr auto kRetryInterval   = std::chrono::hours(1);
constexpr auto kTransferTimeout = std::chrono::seconds(30);

// Release tags are spelled "v0.4.3".
auto strip_tag_prefix(const QString& tag) -> QString {
    if (tag.startsWith(u'v') || tag.startsWith(u'V')) return tag.sliced(1);
    return tag;
}

auto parse_version(const QString& text) -> QVersionNumber {
    return QVersionNumber::fromString(strip_tag_prefix(text));
}

auto release_url(const QString& text) -> QUrl {
    const QUrl url(text);
    if (url.scheme() == u"https"_s && url.host() == u"github.com"_s) return url;
    return QUrl(QString::fromLatin1(kReleasesPage));
}

} // namespace

auto UpdateChecker::instance() -> UpdateChecker* {
    static UpdateChecker* the = new UpdateChecker(App::instance());
    return the;
}

UpdateChecker* UpdateChecker::create(QQmlEngine*, QJSEngine*) {
    auto* inst = instance();
    QJSEngine::setObjectOwnership(inst, QJSEngine::CppOwnership);
    return inst;
}

UpdateChecker::UpdateChecker(QObject* parent)
    : QObject(parent), m_network(new QNetworkAccessManager(this)) {
    QSettings settings;
    m_enabled        = kSupported && settings.value(kEnabledKey, true).toBool();
    m_latest_version = settings.value(kLatestVersionKey).toString();
    m_release_url    = release_url(settings.value(kReleaseUrlKey).toString());
    if (const auto secs = settings.value(kLastCheckTimeKey, 0).toLongLong(); secs > 0) {
        m_last_check_time = QDateTime::fromSecsSinceEpoch(secs);
    }

    m_timer.setSingleShot(true);
    connect(&m_timer, &QTimer::timeout, this, &UpdateChecker::check);
    schedule();
}

bool UpdateChecker::supported() const { return kSupported; }

bool UpdateChecker::updateAvailable() const {
    if (! kSupported) return false;
    const auto latest = parse_version(m_latest_version);
    if (latest.isNull()) return false;
    return latest > parse_version(QCoreApplication::applicationVersion());
}

void UpdateChecker::setEnabled(bool enabled) {
    if (! kSupported || m_enabled == enabled) return;
    m_enabled = enabled;
    QSettings().setValue(kEnabledKey, enabled);
    schedule();
    Q_EMIT enabledChanged();
}

void UpdateChecker::schedule() {
    if (! m_enabled) {
        m_timer.stop();
        return;
    }
    if (checking()) return;

    auto delay = std::chrono::milliseconds(0);
    if (m_last_check_time.isValid()) {
        const auto elapsed =
            std::chrono::milliseconds(m_last_check_time.msecsTo(QDateTime::currentDateTimeUtc()));
        // A check time in the future means the clock went back; wait a full
        // interval rather than checking on every start.
        if (elapsed < std::chrono::milliseconds(0)) {
            delay = kCheckInterval;
        } else if (elapsed < kCheckInterval) {
            delay = kCheckInterval - elapsed;
        }
    }
    m_timer.start(delay);
}

void UpdateChecker::check() {
    if (! kSupported || checking()) return;
    m_timer.stop();

    QNetworkRequest request { QUrl(QString::fromLatin1(kLatestReleaseApi)) };
    request.setRawHeader("Accept", "application/vnd.github+json");
    request.setHeader(QNetworkRequest::UserAgentHeader,
                      u"waywallen/%1"_s.arg(QCoreApplication::applicationVersion()));
    request.setTransferTimeout(kTransferTimeout);

    m_reply = m_network->get(request);
    connect(m_reply, &QNetworkReply::finished, this, [this, reply = m_reply] {
        handleReply(reply);
    });
    Q_EMIT checkingChanged();
}

void UpdateChecker::handleReply(QNetworkReply* reply) {
    reply->deleteLater();
    if (reply != m_reply) return;
    m_reply = nullptr;

    QString tag;
    QString url;
    if (reply->error() == QNetworkReply::NoError) {
        const auto doc = QJsonDocument::fromJson(reply->readAll());
        tag            = doc.object().value(u"tag_name"_s).toString();
        url            = doc.object().value(u"html_url"_s).toString();
    } else if (reply->error() != QNetworkReply::OperationCanceledError) {
        qDebug("update check failed: %s", qPrintable(reply->errorString()));
    }

    const bool ok = ! parse_version(tag).isNull();
    if (ok) {
        const auto version = strip_tag_prefix(tag);
        m_last_check_time  = QDateTime::currentDateTimeUtc();

        QSettings settings;
        settings.setValue(kLastCheckTimeKey, m_last_check_time.toSecsSinceEpoch());
        settings.setValue(kLatestVersionKey, version);
        settings.setValue(kReleaseUrlKey, url);

        const auto next_url = release_url(url);
        if (version != m_latest_version || next_url != m_release_url) {
            m_latest_version = version;
            m_release_url    = next_url;
            Q_EMIT latestVersionChanged();
        }
        Q_EMIT lastCheckTimeChanged();
        schedule();
    } else if (m_enabled) {
        m_timer.start(kRetryInterval);
    }

    Q_EMIT checkingChanged();
    Q_EMIT checkFinished(ok);
}

} // namespace waywallen

#include "waywallen/update_checker.moc.cpp"
