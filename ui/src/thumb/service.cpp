module;
#include "waywallen/thumb/service.moc.h"
#include <QSaveFile>
#include <algorithm>

module waywallen;
import :thumb.service;
import rstd;
import rstd.cppstd;
import wavsen.decode;
import waywallen.image.definition;

using namespace Qt::Literals::StringLiterals;

namespace waywallen
{
namespace
{
QString cache_root() {
    auto root = qEnvironmentVariable("WAYWALLEN_THUMB_DIR");
    if (root.isEmpty()) {
        root = qEnvironmentVariable("XDG_CACHE_HOME");
        root =
            root.isEmpty() ? QDir::homePath() + u"/.cache/thumbnails"_s : root + u"/thumbnails"_s;
    }
    return root + u"/waywallen-v2"_s;
}

QUrl input_url(const QString& value) {
    if (value.startsWith(u":/"_s)) return QUrl(u"qrc"_s + value);
    const QUrl url(value);
    return url.scheme().isEmpty()
               ? QUrl::fromLocalFile(QDir::cleanPath(QDir::current().absoluteFilePath(value)))
               : url;
}

struct Stamp {
    QString path;
    bool    present;
    qint64  modified;
    qint64  size;
    bool    operator==(const Stamp&) const = default;
};

QList<Stamp> stamps(QStringList paths) {
    std::sort(paths.begin(), paths.end());
    paths.removeDuplicates();
    QList<Stamp> result;
    for (const auto& path : paths) {
        const QFileInfo info(path);
        const bool      present = info.isFile();
        result.append({ path,
                        present,
                        present ? info.lastModified().toMSecsSinceEpoch() : -1,
                        present ? info.size() : -1 });
    }
    return result;
}

QByteArray fingerprint(const QList<Stamp>& inputs, bool timeline, int edge) {
    QByteArray bytes = "thumbnail-v3:";
    bytes += timeline ? "timeline:" : "video:";
    bytes += QByteArray::number(edge) + '\0';
    for (const auto& input : inputs)
        bytes += QUrl::fromLocalFile(input.path).toEncoded() + '\0' +
                 QByteArray::number(input.modified) + ':' + QByteArray::number(input.size) + '\0';
    return QCryptographicHash::hash(bytes, QCryptographicHash::Sha256).toHex();
}

QImage decode_frame(const QString& path, int edge, bool video, QString& error) {
    wavsen::decode::ThumbOptions options;
    options.max_edge = rstd::u32(static_cast<rstd::uint32_t>(edge));
    if (! video) {
        options.seek_seconds  = rstd::f64();
        options.seek_fraction = rstd::f64();
    }
    auto input  = path.toStdString();
    auto result = wavsen::decode::extract_thumbnail(rstd::cppstd::as_str(input).unwrap(), options);
    if (result.is_err()) {
        error = path + u": "_s +
                QString::fromStdString(rstd::cppstd::to_string(result.unwrap_err().message));
        return {};
    }
    auto rgba = rstd::move(result).unwrap();
    return QImage(rgba.data.data(),
                  static_cast<int>(rgba.width.to_primitive()),
                  static_cast<int>(rgba.height.to_primitive()),
                  static_cast<int>(rgba.stride.to_primitive()),
                  QImage::Format_RGBA8888)
        .copy();
}

QImage compose(const ww_image::ImageDefinition& definition, int edge, QString& error) {
    QList<QImage> images;
    for (const auto& candidate : definition.preview()) {
        auto image = decode_frame(
            QString::fromStdString(rstd::cppstd::to_string(candidate)), edge, false, error);
        if (! image.isNull()) images.append(image);
    }
    if (images.isEmpty()) return {};
    auto image = images.first().convertToFormat(QImage::Format_RGBA8888_Premultiplied);
    if (images.size() < 2) return image;
    auto other =
        images[1].scaled(image.size(), Qt::KeepAspectRatioByExpanding, Qt::SmoothTransformation);
    other = other
                .copy((other.width() - image.width()) / 2,
                      (other.height() - image.height()) / 2,
                      image.width(),
                      image.height())
                .convertToFormat(QImage::Format_RGBA8888_Premultiplied);
    for (int y = 0; y < image.height(); ++y) {
        auto*        pixels   = image.scanLine(y);
        const auto*  dark     = other.constScanLine(y);
        const double boundary = image.width() * (0.575 - 0.15 * (y + 0.5) / image.height());
        for (int x = 0; x < image.width(); ++x) {
            const double coverage = x - boundary + 0.5;
            if (coverage <= 0) continue;
            const unsigned weight = coverage >= 1 ? 256 : static_cast<unsigned>(coverage * 256);
            for (int channel = 0; channel < 4; ++channel) {
                const int offset = x * 4 + channel;
                pixels[offset]   = static_cast<unsigned char>(
                    (pixels[offset] * (256 - weight) + dark[offset] * weight + 128) / 256);
            }
        }
    }
    return image;
}

bool write_image(const QImage& image, const QString& path, QString& error) {
    const auto directory = QFileInfo(path).absolutePath();
    if (! QDir().mkpath(directory)) {
        error = u"Cannot create thumbnail cache: "_s + directory;
        return false;
    }
    QFile(directory).setPermissions(QFile::ReadOwner | QFile::WriteOwner | QFile::ExeOwner);
    QSaveFile file(path);
    if (! file.open(QIODevice::WriteOnly)) {
        error = file.errorString();
        return false;
    }
    QImageWriter writer(&file, "png");
    if (! writer.write(image)) {
        error = writer.errorString();
        return false;
    }
    file.setPermissions(QFile::ReadOwner | QFile::WriteOwner);
    if (! file.commit()) {
        error = file.errorString();
        return false;
    }
    return true;
}
} // namespace

ThumbnailJob::ThumbnailJob(quint64 id, QString key, QString path, QString root, bool timeline,
                           int edge, QObject* parent)
    : QObject(parent),
      m_id(id),
      m_key(std::move(key)),
      m_path(std::move(path)),
      m_cache_root(std::move(root)),
      m_timeline(timeline),
      m_maximum_edge(edge) {
    setAutoDelete(false);
}

void ThumbnailJob::run() {
    QString error;
    for (int attempt = 0; attempt < 2; ++attempt) {
        const auto source_before = stamps({ m_path });
        if (! QFileInfo(m_path).isFile()) {
            error = u"Thumbnail source file not found: "_s + m_path;
            break;
        }
        QStringList                             dependencies { m_path };
        rstd::Option<ww_image::ImageDefinition> definition;
        if (m_timeline) {
            const auto            path = m_path.toStdString();
            rstd::prelude::String reason;
            definition =
                ww_image::ImageDefinition::open(rstd::cppstd::as_str(path).unwrap(), reason);
            if (! definition) {
                error = m_path + u": "_s + QString::fromStdString(rstd::cppstd::to_string(reason));
                break;
            }
            for (const auto& input : definition->dependencies)
                dependencies.append(QString::fromStdString(rstd::cppstd::to_string(input)));
        }
        if (source_before != stamps({ m_path })) continue;
        const auto before   = stamps(dependencies);
        const auto identity = fingerprint(before, m_timeline, m_maximum_edge);
        const auto output   = m_cache_root + u"/"_s + QString::fromLatin1(identity) + u".png"_s;
        const bool hit      = QFileInfo(output).isFile();
        QImage     image;
        if (! hit) {
            image = m_timeline ? compose(*definition, m_maximum_edge, error)
                               : decode_frame(m_path, m_maximum_edge, true, error);
            if (image.isNull()) break;
        }
        if (before != stamps(dependencies)) continue;
        if (! hit && ! write_image(image, output, error)) break;
        if (before != stamps(dependencies)) continue;
        Q_EMIT finished(m_id, m_key, output, {});
        return;
    }
    if (error.isEmpty()) error = u"Thumbnail source changed during preparation: "_s + m_path;
    Q_EMIT finished(m_id, m_key, {}, error);
}

ThumbnailService::ThumbnailService(QObject* parent): QObject(parent) {
    m_pool.setMaxThreadCount(std::max(1, std::min(4, QThread::idealThreadCount())));
}
ThumbnailService::~ThumbnailService() { m_pool.waitForDone(); }

ThumbnailService* ThumbnailService::instance() {
    if (! QCoreApplication::instance() || QCoreApplication::closingDown()) return nullptr;
    static QPointer<ThumbnailService> service;
    if (! service) service = new ThumbnailService(QCoreApplication::instance());
    return service;
}

void ThumbnailService::submit(ThumbnailRequest* request, quint64 revision, const QString& path,
                              bool timeline, int edge, bool prefetch) {
    const auto root = cache_root();
    const auto key  = QString::number(timeline) + u":"_s + QString::number(edge) + u":"_s + path +
                      QChar(0) + root;
    const int  priority = prefetch ? 0 : 1;
    auto       it       = m_pending.find(key);
    if (it != m_pending.end()) {
        it->subscribers.append({ request, revision });
        if (priority > it->priority && m_pool.tryTake(it->job)) {
            it->priority = priority;
            m_pool.start(it->job, priority);
        }
        return;
    }
    const auto id  = ++m_next_id;
    auto*      job = new ThumbnailJob(id, key, path, root, timeline, edge, this);
    m_pending.insert(key, Pending { id, job, priority, { { request, revision } } });
    connect(
        job, &ThumbnailJob::finished, this, &ThumbnailService::onFinished, Qt::QueuedConnection);
    connect(job, &ThumbnailJob::finished, job, &QObject::deleteLater);
    m_pool.start(job, priority);
}

void ThumbnailService::cancel(ThumbnailRequest* request) {
    for (auto it = m_pending.begin(); it != m_pending.end();) {
        it->subscribers.removeIf([request](const Subscription& subscriber) {
            return ! subscriber.request || subscriber.request == request;
        });
        if (it->subscribers.isEmpty()) {
            if (m_pool.tryTake(it->job)) delete it->job;
            it = m_pending.erase(it);
        } else
            ++it;
    }
}

void ThumbnailService::reprioritize() {
    for (auto& pending : m_pending) {
        const int priority =
            std::any_of(pending.subscribers.begin(),
                        pending.subscribers.end(),
                        [](const Subscription& subscriber) {
                            return subscriber.request && ! subscriber.request->prefetch();
                        })
                ? 1
                : 0;
        if (priority != pending.priority && m_pool.tryTake(pending.job)) {
            pending.priority = priority;
            m_pool.start(pending.job, priority);
        }
    }
}

void ThumbnailService::onFinished(quint64 id, const QString& key, const QString& path,
                                  const QString& error) {
    auto it = m_pending.find(key);
    if (it == m_pending.end() || it->id != id) return;
    const auto subscribers = std::move(it->subscribers);
    m_pending.erase(it);
    const auto url = path.isEmpty() ? QUrl() : QUrl::fromLocalFile(path);
    for (const auto& subscriber : subscribers)
        if (auto* request = subscriber.request.data())
            request->publish(subscriber.revision,
                             error.isEmpty() ? ThumbnailRequest::Ready : ThumbnailRequest::Failed,
                             error.isEmpty() ? url
                                             : (request->m_retain ? request->m_cache_path : QUrl()),
                             error);
}

ThumbnailRequest::ThumbnailRequest(QObject* parent): QObject(parent) {}
ThumbnailRequest::~ThumbnailRequest() {
    if (auto* service = ThumbnailService::instance()) service->cancel(this);
}
void ThumbnailRequest::classBegin() {}
void ThumbnailRequest::componentComplete() {
    m_complete = true;
    invalidate();
}

void ThumbnailRequest::setSource(const QString& value) {
    if (m_source == value) return;
    m_source = value;
    QPointer<ThumbnailRequest> guard(this);
    invalidate();
    if (guard) Q_EMIT sourceChanged();
}
void ThumbnailRequest::setResource(const QString& value) {
    if (m_resource == value) return;
    m_resource = value;
    QPointer<ThumbnailRequest> guard(this);
    invalidate();
    if (guard) Q_EMIT resourceChanged();
}
void ThumbnailRequest::setWpType(const QString& value) {
    if (m_wp_type == value) return;
    m_wp_type = value;
    QPointer<ThumbnailRequest> guard(this);
    invalidate();
    if (guard) Q_EMIT wpTypeChanged();
}
void ThumbnailRequest::setTimeline(bool value) {
    if (m_timeline == value) return;
    m_timeline = value;
    QPointer<ThumbnailRequest> guard(this);
    invalidate();
    if (guard) Q_EMIT timelineChanged();
}
void ThumbnailRequest::setEnabled(bool value) {
    if (m_enabled == value) return;
    m_enabled = value;
    QPointer<ThumbnailRequest> guard(this);
    invalidate();
    if (guard) Q_EMIT enabledChanged();
}
void ThumbnailRequest::setPrefetch(bool value) {
    if (m_prefetch == value) return;
    m_prefetch = value;
    if (m_complete)
        if (auto* service = ThumbnailService::instance()) service->reprioritize();
    Q_EMIT prefetchChanged();
}
void ThumbnailRequest::setRetainWhileLoading(bool value) {
    if (m_retain == value) return;
    m_retain = value;
    QPointer<ThumbnailRequest> guard(this);
    if (! value && m_state != Ready) publish(m_revision, m_state, {}, m_error);
    if (guard) Q_EMIT retainWhileLoadingChanged();
}
void ThumbnailRequest::setMaximumEdge(int value) {
    value = std::clamp(value, 1, 4096);
    if (m_maximum_edge == value) return;
    m_maximum_edge = value;
    QPointer<ThumbnailRequest> guard(this);
    invalidate();
    if (guard) Q_EMIT maximumEdgeChanged();
}
void ThumbnailRequest::reload() { invalidate(); }

void ThumbnailRequest::invalidate() {
    if (! m_complete) return;
    ++m_revision;
    if (auto* service = ThumbnailService::instance()) service->cancel(this);
    if (! m_scheduled) {
        m_scheduled = true;
        QTimer::singleShot(0, this, [this] {
            m_scheduled = false;
            submit();
        });
    }
    const bool active = m_enabled && (! m_source.isEmpty() || ! m_resource.isEmpty());
    publish(m_revision, active ? Loading : Idle, active && m_retain ? m_cache_path : QUrl());
}

void ThumbnailRequest::submit() {
    if (! m_enabled || (m_source.isEmpty() && m_resource.isEmpty())) return;
    const auto revision = m_revision;
    const auto input    = input_url(m_source.isEmpty() ? m_resource : m_source);
    const bool generate = m_timeline || (m_source.isEmpty() && m_wp_type == u"video"_s);
    if (! generate) {
        if (m_source.isEmpty() && m_wp_type != u"image"_s) {
            publish(revision, Failed, {}, u"No thumbnail preview source"_s);
        } else
            publish(revision, Ready, input);
        return;
    }
    if (! input.isLocalFile()) {
        publish(revision, Failed, {}, u"Thumbnail generation requires a local resource"_s);
        return;
    }
    if (auto* service = ThumbnailService::instance())
        service->submit(this,
                        revision,
                        QDir::cleanPath(input.toLocalFile()),
                        m_timeline,
                        m_maximum_edge,
                        m_prefetch);
    else
        publish(revision, Failed, {}, u"Thumbnail service is unavailable"_s);
}

void ThumbnailRequest::publish(quint64 revision, State state, QUrl path, QString error) {
    if (revision != m_revision) return;
    const bool path_changed  = path != m_cache_path;
    const bool error_changed = error != m_error;
    const bool state_changed = state != m_state;
    m_cache_path             = std::move(path);
    m_error                  = std::move(error);
    m_state                  = state;
    QPointer<ThumbnailRequest> guard(this);
    if (path_changed) Q_EMIT cachePathChanged();
    if (! guard || revision != m_revision) return;
    if (error_changed) Q_EMIT errorChanged();
    if (! guard || revision != m_revision) return;
    if (state_changed) Q_EMIT stateChanged();
}
} // namespace waywallen

#include "waywallen/thumb/service.moc.cpp"
