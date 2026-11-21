module;
#include "waywallen/update_checker.moc.h"

#include <filesystem>

module waywallen;
import qextra.qt;
import rstd.cppstd;
import :update_checker;
import :app;
import :daemon_dbus;

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
constexpr auto kAssetUrlKey      = "update/assetUrl";
constexpr auto kAssetSizeKey     = "update/assetSize";
constexpr auto kReadyVersionKey  = "update/readyVersion";
constexpr auto kLatestReleaseApi =
    "https://api.github.com/repos/waywallen/waywallen/releases/latest";
constexpr auto kReleasesPage    = "https://github.com/waywallen/waywallen/releases/latest";
constexpr auto kDaemonService   = "org.waywallen.waywallen.Daemon";
constexpr auto kUiService       = "org.waywallen.waywallen.UI";
constexpr auto kCheckInterval   = std::chrono::hours(24);
constexpr auto kRetryInterval   = std::chrono::hours(1);
constexpr auto kTransferTimeout = std::chrono::seconds(30);
constexpr auto kDownloadTimeout = std::chrono::seconds(60);
constexpr auto kRestartTick     = std::chrono::milliseconds(200);
constexpr auto kDaemonStopWait  = std::chrono::milliseconds(15000);
constexpr auto kDaemonKillWait  = std::chrono::milliseconds(20000);

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

auto service_registered(const char* name) -> bool {
    auto* interface = QDBusConnection::sessionBus().interface();
    return interface && interface->isServiceRegistered(QString::fromLatin1(name)).value();
}

// Release asset names use "aarch64"; QSysInfo spells it "arm64".
auto release_arch() -> QString {
    const auto arch = QSysInfo::currentCpuArchitecture();
    return arch == u"arm64"_s ? u"aarch64"_s : arch;
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
    m_asset_url      = QUrl(settings.value(kAssetUrlKey).toString());
    m_asset_size     = settings.value(kAssetSizeKey, 0).toLongLong();
    m_ready_version  = settings.value(kReadyVersionKey).toString();
    if (const auto secs = settings.value(kLastCheckTimeKey, 0).toLongLong(); secs > 0) {
        m_last_check_time = QDateTime::fromSecsSinceEpoch(secs);
    }

    m_timer.setSingleShot(true);
    connect(&m_timer, &QTimer::timeout, this, &UpdateChecker::check);

    m_restart_timer.setInterval(kRestartTick);
    connect(&m_restart_timer, &QTimer::timeout, this, &UpdateChecker::on_restart_tick);

    schedule();
}

bool UpdateChecker::supported() const { return kSupported; }

bool UpdateChecker::updateAvailable() const {
    if (! kSupported) return false;
    const auto latest = parse_version(m_latest_version);
    if (latest.isNull()) return false;
    return latest > parse_version(QCoreApplication::applicationVersion());
}

bool UpdateChecker::selfUpdateAvailable() const {
    return kSupported && can_self_update() && m_asset_url.isValid();
}

bool UpdateChecker::updateReady() const {
    return can_self_update() && ! m_latest_version.isEmpty() && m_ready_version == m_latest_version;
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
    QUrl    asset_url;
    qint64  asset_size = 0;
    bool    ok         = false;

    if (reply->error() == QNetworkReply::NoError) {
        const auto root = QJsonDocument::fromJson(reply->readAll()).object();
        tag             = root.value(u"tag_name"_s).toString();
        url             = root.value(u"html_url"_s).toString();
        ok              = ! parse_version(tag).isNull();

        if (ok) {
            const auto arch = release_arch();
            for (const auto& entry : root.value(u"assets"_s).toArray()) {
                const auto asset = entry.toObject();
                const auto name  = asset.value(u"name"_s).toString();
                if (! name.endsWith(u".AppImage"_s) || ! name.contains(arch)) continue;
                const QUrl candidate(asset.value(u"browser_download_url"_s).toString());
                if (candidate.scheme() != u"https"_s) continue;
                asset_url  = candidate;
                asset_size = asset.value(u"size"_s).toInteger();
                break;
            }
        }
    } else if (reply->error() != QNetworkReply::OperationCanceledError) {
        qDebug("update check failed: %s", qPrintable(reply->errorString()));
    }

    if (ok) {
        const auto version       = strip_tag_prefix(tag);
        const bool asset_changed = asset_url != m_asset_url || asset_size != m_asset_size;

        m_last_check_time = QDateTime::currentDateTimeUtc();
        m_asset_url       = asset_url;
        m_asset_size      = asset_size;

        QSettings settings;
        settings.setValue(kLastCheckTimeKey, m_last_check_time.toSecsSinceEpoch());
        settings.setValue(kLatestVersionKey, version);
        settings.setValue(kReleaseUrlKey, url);
        settings.setValue(kAssetUrlKey, asset_url.toString());
        settings.setValue(kAssetSizeKey, asset_size);

        const auto next_url = release_url(url);
        if (version != m_latest_version || next_url != m_release_url) {
            m_latest_version = version;
            m_release_url    = next_url;
            Q_EMIT latestVersionChanged();
        }
        if (asset_changed) Q_EMIT selfUpdateAvailableChanged();

        Q_EMIT lastCheckTimeChanged();
        schedule();
    } else if (m_enabled) {
        m_timer.start(kRetryInterval);
    }

    Q_EMIT checkingChanged();
    Q_EMIT checkFinished(ok);

    if (ok && m_enabled && updateAvailable() && selfUpdateAvailable() && ! updateReady())
        download();
}

void UpdateChecker::download() {
    if (! kSupported || m_download || ! updateAvailable() || ! selfUpdateAvailable()) return;

    const auto part = part_path();
    QFile::remove(part);

    auto* file = new QFile(part, this);
    if (! file->open(QIODevice::WriteOnly | QIODevice::Truncate)) {
        delete file;
        set_download_error(tr("Could not write the update next to the AppImage."));
        return;
    }
    m_download_file     = file;
    m_download_version  = m_latest_version;
    m_download_progress = 0.0;
    m_download_total    = 0;
    m_update_error.clear();

    QNetworkRequest request { m_asset_url };
    request.setRawHeader("Accept", "application/octet-stream");
    request.setHeader(QNetworkRequest::UserAgentHeader,
                      u"waywallen/%1"_s.arg(QCoreApplication::applicationVersion()));
    request.setAttribute(QNetworkRequest::RedirectPolicyAttribute,
                         QNetworkRequest::NoLessSafeRedirectPolicy);
    request.setTransferTimeout(kDownloadTimeout);

    m_download = m_network->get(request);
    connect(m_download, &QNetworkReply::readyRead, this, [this] {
        if (m_download && m_download_file) m_download_file->write(m_download->readAll());
    });
    connect(
        m_download, &QNetworkReply::downloadProgress, this, [this](qint64 received, qint64 total) {
            m_download_total    = total;
            m_download_progress = total > 0 ? qreal(received) / qreal(total) : 0.0;
            Q_EMIT downloadProgressChanged();
        });
    connect(m_download, &QNetworkReply::finished, this, &UpdateChecker::finish_download);

    Q_EMIT downloadStateChanged();
    Q_EMIT downloadProgressChanged();
}

void UpdateChecker::finish_download() {
    QNetworkReply* reply = m_download;
    m_download           = nullptr;

    if (m_download_file) {
        if (reply) m_download_file->write(reply->readAll());
        m_download_file->close();
        m_download_file->deleteLater();
        m_download_file = nullptr;
    }

    if (! reply) {
        set_download_error(tr("The update download failed."));
        return;
    }

    const bool   failed = reply->error() != QNetworkReply::NoError;
    const int    status = reply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt();
    const qint64 total  = m_download_total;
    reply->deleteLater();

    if (failed || (status != 0 && status != 200)) {
        set_download_error(tr("The update download failed."));
        return;
    }

    const auto   part     = part_path();
    const auto   target   = resolved_image();
    const qint64 expected = m_asset_size > 0 ? m_asset_size : total;
    if (! QFileInfo::exists(part) || (expected > 0 && QFileInfo(part).size() != expected)) {
        set_download_error(tr("The downloaded update is incomplete."));
        return;
    }

    QFile downloaded(part);
    if (! downloaded.open(QIODevice::ReadOnly)) {
        set_download_error(tr("The downloaded update is incomplete."));
        return;
    }
    const auto head = downloaded.read(11);
    downloaded.close();

    const bool is_appimage = head.size() == 11 &&
                             head.startsWith(QByteArrayLiteral("\x7f"
                                                               "ELF")) &&
                             head.at(8) == 'A' && head.at(9) == 'I' && head.at(10) == 2;
    if (! is_appimage) {
        set_download_error(tr("The downloaded file is not a valid AppImage."));
        return;
    }

    auto permissions = QFileInfo(target).permissions();
    permissions |= QFileDevice::ReadOwner | QFileDevice::WriteOwner | QFileDevice::ExeOwner |
                   QFileDevice::ExeGroup | QFileDevice::ExeOther;
    if (! QFile::setPermissions(part, permissions)) {
        set_download_error(tr("The update could not be installed next to the AppImage."));
        return;
    }

    std::error_code error;
    std::filesystem::rename(std::filesystem::path(part.toStdString()),
                            std::filesystem::path(target.toStdString()),
                            error);
    if (error) {
        QFile::remove(part);
        set_download_error(tr("The update could not be installed next to the AppImage."));
        return;
    }

    m_ready_version = m_download_version;
    m_download_version.clear();
    m_download_progress = 1.0;
    m_update_error.clear();
    QSettings().setValue(kReadyVersionKey, m_ready_version);
    Q_EMIT downloadProgressChanged();
    Q_EMIT downloadStateChanged();
}

void UpdateChecker::set_download_error(const QString& error) {
    if (m_download_file) {
        m_download_file->close();
        m_download_file->deleteLater();
        m_download_file = nullptr;
    }
    if (m_download) {
        m_download->deleteLater();
        m_download = nullptr;
    }
    QFile::remove(part_path());
    m_update_error      = error;
    m_download_progress = 0.0;
    Q_EMIT downloadProgressChanged();
    Q_EMIT downloadStateChanged();
}

void UpdateChecker::restartToUpdate() {
    if (! updateReady() || m_restarting) return;

    m_restarting           = true;
    m_restart_sigterm_sent = false;
    m_update_error.clear();
    m_restart_clock.start();
    DaemonDBusClient::instance()->setShutdownQuitHold(true);
    Q_EMIT downloadStateChanged();

    if (! service_registered(kDaemonService)) {
        launch_new_image();
        return;
    }
    if (! DaemonDBusClient::instance()->quitDaemon()) signal_daemon_terminate();
    m_restart_timer.start(kRestartTick);
}

void UpdateChecker::on_restart_tick() {
    if (service_registered(kDaemonService)) {
        const auto elapsed = std::chrono::milliseconds(m_restart_clock.elapsed());
        if (elapsed > kDaemonStopWait && ! m_restart_sigterm_sent) {
            m_restart_sigterm_sent = true;
            signal_daemon_terminate();
        } else if (elapsed > kDaemonKillWait) {
            fail_restart(tr("Could not stop the running daemon. Restart Waywallen manually."));
        }
        return;
    }
    m_restart_timer.stop();
    launch_new_image();
}

void UpdateChecker::signal_daemon_terminate() {
    auto* client = DaemonDBusClient::instance();
    for (const auto& entry : client->listWaywallenProcesses()) {
        const auto pid = entry.toMap().value(u"pid"_s).toUInt();
        if (pid != 0) client->killProcess(pid);
    }
}

void UpdateChecker::launch_new_image() {
    // Let the new UI claim the single-instance name instead of raising us.
    QDBusConnection::sessionBus().unregisterService(QString::fromLatin1(kUiService));
    if (! QProcess::startDetached(resolved_image(), {})) {
        fail_restart(tr("Could not start the new AppImage. Launch Waywallen again."));
        return;
    }
    QCoreApplication::quit();
}

void UpdateChecker::fail_restart(const QString& error) {
    m_restart_timer.stop();
    m_restarting   = false;
    m_update_error = error;
    DaemonDBusClient::instance()->setShutdownQuitHold(false);
    if (! service_registered(kDaemonService))
        QProcess::startDetached(resolved_image(), { QStringLiteral("--no-ui") });
    Q_EMIT downloadStateChanged();
}

auto UpdateChecker::resolved_image() const -> QString {
    const auto path = qEnvironmentVariable("APPIMAGE");
    if (path.isEmpty()) return {};
    return QFileInfo(path).canonicalFilePath();
}

auto UpdateChecker::can_self_update() const -> bool {
    const auto image = resolved_image();
    if (image.isEmpty()) return false;
    const QFileInfo file(image);
    return file.isFile() && file.isWritable() && QFileInfo(file.absolutePath()).isWritable();
}

auto UpdateChecker::part_path() const -> QString { return resolved_image() + u".part"_s; }

} // namespace waywallen

#include "waywallen/update_checker.moc.cpp"
