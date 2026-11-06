module;
#include "waywallen/notify.moc.h"

module waywallen;
import qextra.qt;
import :notify;
import :app;
import :msg.store;

namespace proto = waywallen::control::v1;

namespace waywallen
{

namespace
{
auto pb_phase_to_enum(proto::DaemonPhase p) -> Notify::DaemonPhase {
    switch (p) {
    case proto::DaemonPhase::DAEMON_PHASE_READY: return Notify::DaemonPhase::Ready;
    default: return Notify::DaemonPhase::Starting;
    }
}
} // namespace

auto Notify::instance() -> Notify* {
    // Lazy-constructed once; parented to App so it rides the normal
    // QObject ownership tree and gets cleaned up on app teardown.
    static Notify* the = new Notify(App::instance());
    return the;
}

Notify* Notify::create(QQmlEngine*, QJSEngine*) {
    auto n = instance();
    QJSEngine::setObjectOwnership(n, QJSEngine::CppOwnership);
    return n;
}

auto Notify::taskProgressSnapshot(const QString& queryId, TaskProgressSnapshot& out) const -> bool {
    auto it = m_task_progress.constFind(queryId);
    if (it == m_task_progress.constEnd()) {
        return false;
    }
    out = it.value();
    return true;
}

Notify::Notify(QObject* parent): QObject(parent) {
    auto* backend = App::instance()->backend();
    if (! backend) {
        return;
    }

    // Subscribe to the daemon's server-event channel exactly once.
    // Backend lives for the App's lifetime; the connection is parented
    // to `this` so the QueuedConnection unwinds cleanly on shutdown.
    connect(
        backend,
        &Backend::eventReceived,
        this,
        [this](const proto::Event& evt) {
            if (evt.hasWallpaperSyncFinished()) {
                const auto& done = evt.wallpaperSyncFinished();
                Q_EMIT wallpaperSyncFinished(done.count(), done.error());
            } else if (evt.hasWallpaperHiddenChanged()) {
                const auto& changed = evt.wallpaperHiddenChanged();
                Q_EMIT wallpaperHiddenChanged(changed.wallpaperIds(), changed.hidden());
            } else if (evt.hasLibrariesAdded()) {
                const auto& src = evt.librariesAdded().paths();
                QStringList paths;
                paths.reserve(src.size());
                for (const auto& p : src) {
                    paths.push_back(p);
                }
                Q_EMIT librariesAdded(paths);
            } else if (evt.hasStatusSync()) {
                const auto&       s         = evt.statusSync();
                const bool        new_scan  = s.scanInProgress();
                const quint32     new_tasks = s.activeTaskCount();
                const DaemonPhase new_phase = pb_phase_to_enum(s.phase());
                const auto        new_backend =
                    s.hasDisplayBackend() ? s.displayBackend() : proto::DisplayBackendStatus {};
                const bool new_paused  = s.globalPaused();
                const bool new_muted   = s.globalMuted();
                const bool new_stopped = s.globalStopped();
                const bool ready_edge =
                    new_phase == DaemonPhase::Ready && m_daemon_phase != DaemonPhase::Ready;
                if (new_scan != m_scan_in_progress || new_tasks != m_active_task_count ||
                    new_phase != m_daemon_phase || new_backend != m_display_backend ||
                    new_paused != m_global_paused || new_muted != m_global_muted ||
                    new_stopped != m_global_stopped) {
                    m_scan_in_progress  = new_scan;
                    m_active_task_count = new_tasks;
                    m_daemon_phase      = new_phase;
                    m_display_backend   = new_backend;
                    m_global_paused     = new_paused;
                    m_global_muted      = new_muted;
                    m_global_stopped    = new_stopped;
                    Q_EMIT statusChanged();
                }
                if (ready_edge) {
                    Q_EMIT daemonReady();
                }
            } else if (evt.hasSettingsChanged()) {
                Q_EMIT settingsChanged();
            } else if (evt.hasDisplayConnectionFailed()) {
                const auto& f = evt.displayConnectionFailed();
                Q_EMIT displayConnectionFailed(
                    f.clientName(), f.clientProtocolVersion(), f.errorCode(), f.reason());
            } else if (evt.hasRemoteDownloadProgress()) {
                const auto& p        = evt.remoteDownloadProgress();
                const auto& transfer = p.transfer();
                AppStore::instance()->setRemoteAcquisitionState(
                    p.sourceId(),
                    p.id_proto(),
                    static_cast<int>(p.state()),
                    p.hasTransfer(),
                    transfer.receivedBytes(),
                    transfer.hasTotalBytes() ? transfer.totalBytes() : 0);
                Q_EMIT remoteDownloadProgress(
                    p.sourceId(), p.id_proto(), static_cast<int>(p.state()), p.error());
            } else if (evt.hasQrLoginProgress()) {
                const auto& p = evt.qrLoginProgress();
                Q_EMIT qrLoginProgress(p.sessionId(),
                                       p.pluginId(),
                                       p.actionId(),
                                       static_cast<int>(p.state()),
                                       p.qrImage(),
                                       pluginMessageFromPb(p.displayValueText(), p.displayValue()),
                                       pluginMessageFromPb(p.errorText(), p.error()),
                                       pluginMessageFromPb(p.titleText(), p.title()),
                                       pluginMessageFromPb(p.instructionText(), p.instruction()));
            } else if (evt.hasPluginStateChanged()) {
                Q_EMIT pluginStateChanged();
            } else if (evt.hasPlaylistChanged()) {
                Q_EMIT playlistChanged();
            } else if (evt.hasPluginChanged()) {
                Q_EMIT pluginChanged();
            } else if (evt.hasPluginUpdateChanged()) {
                Q_EMIT pluginUpdateChanged();
            } else if (evt.hasPluginRestartFailed()) {
                const auto& f = evt.pluginRestartFailed();
                Q_EMIT pluginRestartFailed(f.pluginId(), f.error());
            } else if (evt.hasTaskProgress()) {
                const auto&          p = evt.taskProgress();
                TaskProgressSnapshot snap {
                    .progress    = p.progress(),
                    .progressing = p.progressing(),
                    .ended       = p.ended(),
                    .error       = p.error(),
                    .message     = p.message(),
                    .sequence    = ++m_task_progress_sequence,
                };
                m_task_progress.insert(p.queryId(), snap);
                Q_EMIT taskProgress(p.queryId(),
                                    snap.progress,
                                    snap.progressing,
                                    snap.ended,
                                    snap.error,
                                    snap.message,
                                    snap.sequence);
            }
        },
        Qt::QueuedConnection);

    // Desync guard: on WS drop, revert to the pessimistic starting
    // state so the dialog re-asserts and the next StatusSync from a
    // fresh connection re-derives truth from the wire — UI never holds
    // stale Ready while the daemon is gone.
    connect(backend, &Backend::disconnected, this, [this] {
        const bool changed = m_scan_in_progress || m_active_task_count != 0 ||
                             m_daemon_phase != DaemonPhase::Starting ||
                             m_display_backend != proto::DisplayBackendStatus {} ||
                             m_global_paused || m_global_muted || m_global_stopped;
        if (! changed) {
            return;
        }
        m_scan_in_progress  = false;
        m_active_task_count = 0;
        m_daemon_phase      = DaemonPhase::Starting;
        m_display_backend   = proto::DisplayBackendStatus {};
        m_global_paused     = false;
        m_global_muted      = false;
        m_global_stopped    = false;
        m_task_progress.clear();
        m_task_progress_sequence = 0;
        Q_EMIT statusChanged();
    });
}
Notify::~Notify() = default;

} // namespace waywallen

#include "waywallen/notify.moc.cpp"
