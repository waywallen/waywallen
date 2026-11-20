use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;

use super::playback::{has_external_playback, renderer_stream_indices, PlaybackObserverBackend};
use super::pulse::{
    list_playback_streams, move_playback_streams, AudioCaptureBackend, CaptureErrorKind,
    PulseCapture, PulsePlaybackObserver,
};
use super::window::{AudioPcmWindow, AUDIO_WINDOW_END_OF_STREAM};
use crate::events::GlobalEvent;
use crate::settings::SettingsStore;
use crate::tasks::{TaskKind, TaskManager};
use crate::wallframe::ipc::proto::{AudioStreamFormat, AudioWindow};
use crate::wallframe::renderer_manager::{RendererEventKind, RendererManager};
use crate::wallframe::routing::Router;

const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const DISPATCH_INTERVAL: Duration = Duration::from_millis(33);
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(5);
const INACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioServiceState {
    Closed,
    Starting,
    Active,
    WarmIdle,
    Backoff,
    Stopping,
}

enum CaptureWorkerCommand {
    Configure {
        enabled: bool,
        demanded: bool,
    },
    /// Drop the current capture so the next open follows the chosen output.
    Reopen,
    Shutdown,
}

enum PlaybackWorkerCommand {
    Demand(bool),
    Shutdown,
}

pub struct AudioService {
    state: watch::Receiver<AudioServiceState>,
}

impl AudioService {
    pub fn start(
        manager: Arc<RendererManager>,
        router: Arc<Router>,
        settings: Arc<SettingsStore>,
        mut events: tokio::sync::broadcast::Receiver<GlobalEvent>,
        mut shutdown: watch::Receiver<bool>,
        tasks: &TaskManager,
    ) -> Self {
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let (frame_tx, frame_rx) = watch::channel::<Option<AudioPcmWindow>>(None);
        let (state_tx, state_rx) = watch::channel(AudioServiceState::Closed);
        // Output device the capture worker listens to. Read each time the
        // worker opens a capture, so a changed choice only needs a reopen.
        let output = Arc::new(std::sync::Mutex::new(
            settings
                .global()
                .renderer
                .audio_output_name()
                .map(str::to_owned),
        ));
        let capture_output = output.clone();
        let worker = std::thread::spawn(move || {
            run_worker(command_rx, frame_tx, state_tx, DEFAULT_IDLE_TIMEOUT, || {
                let sink = capture_output.lock().ok().and_then(|output| output.clone());
                PulseCapture::open(sink.as_deref())
                    .map(|capture| Box::new(capture) as Box<dyn AudioCaptureBackend>)
            });
        });

        let (playback_command_tx, playback_command_rx) = std::sync::mpsc::channel();
        let (playback_tx, playback_rx) = watch::channel(false);
        let process_ownership = manager.subscribe_process_ownership();
        let playback_worker = std::thread::spawn(move || {
            run_playback_worker(playback_command_rx, playback_tx, process_ownership, || {
                PulsePlaybackObserver::open()
                    .map(|observer| Box::new(observer) as Box<dyn PlaybackObserverBackend>)
            });
        });

        let mut subscriptions = manager.subscribe_subscriptions();
        tasks.spawn_async(TaskKind::Service, "service/audio", async move {
            let mut capture_demand = !subscriptions
                .borrow()
                .subscribers_with_wire_revision(RendererEventKind::Audio)
                .is_empty();
            let mut capture_enabled = settings.global().audio_capture_enabled;
            let _ = command_tx.send(CaptureWorkerCommand::Configure {
                enabled: capture_enabled,
                demanded: capture_demand,
            });
            let mut frame_rx = frame_rx;
            let mut playback_rx = playback_rx;
            let mut audio_output = settings.global().renderer.audio_output_name().map(str::to_owned);
            let mut renderer_audio_enabled = settings.global().renderer.enable_audio;
            let mut playback_demand = settings.global().mute_when_other_audio;
            let _ = playback_command_tx.send(PlaybackWorkerCommand::Demand(playback_demand));
            let mut dispatch = tokio::time::interval(DISPATCH_INTERVAL);
            dispatch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_sent = None;

            loop {
                tokio::select! {
                    changed = subscriptions.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        capture_demand = !subscriptions
                            .borrow()
                            .subscribers_with_wire_revision(RendererEventKind::Audio)
                            .is_empty();
                        if !capture_demand {
                            last_sent = None;
                        }
                        if command_tx.send(CaptureWorkerCommand::Configure {
                            enabled: capture_enabled,
                            demanded: capture_demand,
                        }).is_err() {
                            break;
                        }
                    }
                    changed = playback_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let active = *playback_rx.borrow();
                        router.set_other_playback_active(active).await;
                    }
                    event = events.recv() => {
                        match event {
                            Ok(GlobalEvent::SettingsChanged)
                            | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                let global = settings.global();
                                if global.audio_capture_enabled != capture_enabled {
                                    capture_enabled = global.audio_capture_enabled;
                                    if command_tx.send(CaptureWorkerCommand::Configure {
                                        enabled: capture_enabled,
                                        demanded: capture_demand,
                                    }).is_err() {
                                        break;
                                    }
                                    if capture_enabled {
                                        if let Some(frame) = frame_rx.borrow_and_update().as_ref() {
                                            last_sent = last_sent.max(Some((frame.generation, frame.sequence)));
                                        }
                                    } else {
                                        let frame_identity = frame_rx
                                            .borrow_and_update()
                                            .as_ref()
                                            .map(|frame| (frame.generation, frame.sequence));
                                        let (generation, sequence) = next_audio_identity(
                                            last_sent.max(frame_identity),
                                        );
                                        last_sent = Some((generation, sequence));
                                        send_end(&manager, generation, sequence).await;
                                    }
                                }
                                let next_output =
                                    global.renderer.audio_output_name().map(str::to_owned);
                                let output_changed = next_output != audio_output;
                                // Turning audio back on can open a new stream on a
                                // renderer that started before the choice changed.
                                let audio_reenabled = global.renderer.enable_audio
                                    && !renderer_audio_enabled;
                                renderer_audio_enabled = global.renderer.enable_audio;
                                if output_changed {
                                    log::info!(
                                        "audio output changed to {}",
                                        next_output.as_deref().unwrap_or("the default device")
                                    );
                                    audio_output = next_output.clone();
                                    if let Ok(mut shared) = output.lock() {
                                        *shared = next_output.clone();
                                    }
                                    last_sent = None;
                                    if command_tx.send(CaptureWorkerCommand::Reopen).is_err() {
                                        break;
                                    }
                                }
                                if output_changed || (audio_reenabled && audio_output.is_some()) {
                                    route_renderer_streams(
                                        &manager,
                                        audio_output.clone(),
                                        audio_reenabled && !output_changed,
                                    );
                                }
                                let demanded = global.mute_when_other_audio;
                                if demanded != playback_demand {
                                    playback_demand = demanded;
                                    if playback_command_tx
                                        .send(PlaybackWorkerCommand::Demand(demanded))
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = dispatch.tick() => {
                        if !capture_enabled {
                            continue;
                        }
                        let Some(frame) = frame_rx.borrow_and_update().clone() else {
                            continue;
                        };
                        let identity = (frame.generation, frame.sequence);
                        if last_sent.is_some_and(|sent| identity <= sent) {
                            continue;
                        }
                        last_sent = Some(identity);
                        let targets = manager
                            .subscription_snapshot()
                            .subscribers_with_wire_revision(RendererEventKind::Audio);
                        for (id, revision) in targets {
                            if let Err(error) = manager
                                .send_audio_window_latest(
                                    &id,
                                    AudioWindow {
                                        subscription_revision: revision,
                                        generation: frame.generation,
                                        sequence: frame.sequence,
                                        captured_at_ns: frame.captured_at_ns,
                                        end_sample_frame: frame.end_sample_frame,
                                        format: AudioStreamFormat {
                                            sample_rate_hz: frame.sample_rate_hz,
                                            channels: frame.channels,
                                        },
                                        frames: frame.frames,
                                        flags: 0,
                                        samples: frame.samples.to_vec(),
                                    },
                                )
                                .await
                            {
                                log::debug!("renderer {id}: audio dispatch dropped: {error}");
                            }
                        }
                    }
                    result = shutdown.changed() => {
                        if result.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                }
            }

            let _ = command_tx.send(CaptureWorkerCommand::Shutdown);
            let _ = playback_command_tx.send(PlaybackWorkerCommand::Shutdown);
            let _ = tokio::task::spawn_blocking(move || worker.join()).await;
            let _ = tokio::task::spawn_blocking(move || playback_worker.join()).await;
            Ok(())
        });

        Self { state: state_rx }
    }

    pub fn state(&self) -> AudioServiceState {
        *self.state.borrow()
    }
}

/// Moves the audio streams of running renderers to `output` (`None` = the
/// system default device). Renderers started later pick the output up from
/// their environment, so this only has to catch the ones already playing.
///
/// Talking to the sound server blocks, so it runs on the blocking pool.
/// `delayed` waits a moment first, for a stream that is just being opened.
fn route_renderer_streams(manager: &RendererManager, output: Option<String>, delayed: bool) {
    let ownership = manager.subscribe_process_ownership().borrow().clone();
    tokio::task::spawn_blocking(move || {
        if delayed {
            std::thread::sleep(Duration::from_secs(1));
        }
        let streams = match list_playback_streams() {
            Ok(streams) => streams,
            Err(error) => {
                log::warn!("audio output: could not list playback streams: {error}");
                return;
            }
        };
        let indices = renderer_stream_indices(&streams, &ownership);
        if indices.is_empty() {
            return;
        }
        match move_playback_streams(output.as_deref(), &indices) {
            Ok(moved) => log::info!(
                "audio output: moved {moved} of {} renderer stream(s) to {}",
                indices.len(),
                output.as_deref().unwrap_or("the default device")
            ),
            Err(error) => log::warn!("audio output: could not move renderer streams: {error}"),
        }
    });
}

fn next_audio_identity(last: Option<(u64, u64)>) -> (u64, u64) {
    match last {
        None => (0, 1),
        Some((generation, sequence)) => sequence
            .checked_add(1)
            .map(|next| (generation, next))
            .unwrap_or_else(|| (generation.saturating_add(1), 0)),
    }
}

async fn send_end(manager: &RendererManager, generation: u64, sequence: u64) {
    let targets = manager
        .subscription_snapshot()
        .subscribers_with_wire_revision(RendererEventKind::Audio);
    let captured_at_ns = monotonic_now_ns();
    for (id, revision) in targets {
        if let Err(error) = manager
            .send_audio_window_latest(
                &id,
                AudioWindow {
                    subscription_revision: revision,
                    generation,
                    sequence,
                    captured_at_ns,
                    end_sample_frame: 0,
                    format: AudioStreamFormat {
                        sample_rate_hz: 0,
                        channels: 0,
                    },
                    frames: 0,
                    flags: AUDIO_WINDOW_END_OF_STREAM,
                    samples: Vec::new(),
                },
            )
            .await
        {
            log::debug!("renderer {id}: audio end dispatch dropped: {error}");
        }
    }
}

fn run_worker<F>(
    commands: std::sync::mpsc::Receiver<CaptureWorkerCommand>,
    frames: watch::Sender<Option<AudioPcmWindow>>,
    states: watch::Sender<AudioServiceState>,
    idle_timeout: Duration,
    mut open_backend: F,
) where
    F: FnMut() -> Result<Box<dyn AudioCaptureBackend>, super::pulse::CaptureError>,
{
    let mut backend: Option<Box<dyn AudioCaptureBackend>> = None;
    let mut enabled = true;
    let mut demand = false;
    let mut idle_deadline: Option<Instant> = None;
    let mut retry_deadline: Option<Instant> = None;
    let mut backoff = Duration::from_secs(1);
    let mut unavailable_latched = false;

    loop {
        let timeout = if demand {
            ACTIVE_POLL_INTERVAL
        } else {
            idle_deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(INACTIVE_POLL_INTERVAL)
                .min(INACTIVE_POLL_INTERVAL)
        };
        match commands.recv_timeout(timeout) {
            Ok(CaptureWorkerCommand::Configure {
                enabled: next_enabled,
                demanded,
            }) => {
                let next_demand = next_enabled && demanded;
                if next_enabled == enabled && next_demand == demand {
                    continue;
                }
                enabled = next_enabled;
                demand = next_demand;
                frames.send_replace(None);
                retry_deadline = None;
                backoff = Duration::from_secs(1);
                if !enabled {
                    idle_deadline = None;
                    unavailable_latched = false;
                    drop(backend.take());
                    states.send_replace(AudioServiceState::Closed);
                } else if demand {
                    idle_deadline = None;
                    unavailable_latched = false;
                    if let Some(capture) = backend.as_mut() {
                        capture.discard();
                        states.send_replace(AudioServiceState::Active);
                    }
                } else if backend.is_some() {
                    idle_deadline = Some(Instant::now() + idle_timeout);
                    states.send_replace(AudioServiceState::WarmIdle);
                } else {
                    states.send_replace(AudioServiceState::Closed);
                }
            }
            Ok(CaptureWorkerCommand::Reopen) => {
                // The old capture listens to the previous output. Start over
                // so a demanded capture reopens on the new one right away.
                frames.send_replace(None);
                drop(backend.take());
                retry_deadline = None;
                backoff = Duration::from_secs(1);
                unavailable_latched = false;
                idle_deadline = None;
                states.send_replace(AudioServiceState::Closed);
            }
            Ok(CaptureWorkerCommand::Shutdown)
            | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                states.send_replace(AudioServiceState::Stopping);
                frames.send_replace(None);
                drop(backend.take());
                states.send_replace(AudioServiceState::Closed);
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }

        if !demand {
            if idle_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                drop(backend.take());
                idle_deadline = None;
                states.send_replace(AudioServiceState::Closed);
            }
            continue;
        }

        if backend.is_none()
            && !unavailable_latched
            && retry_deadline.is_none_or(|deadline| Instant::now() >= deadline)
        {
            states.send_replace(AudioServiceState::Starting);
            match open_backend() {
                Ok(mut capture) => {
                    capture.discard();
                    backend = Some(capture);
                    retry_deadline = None;
                    backoff = Duration::from_secs(1);
                    states.send_replace(AudioServiceState::Active);
                    log::info!("audio response: PulseAudio capture active");
                }
                Err(error) => {
                    let permanent = matches!(
                        error.kind,
                        CaptureErrorKind::LibraryUnavailable | CaptureErrorKind::MissingSymbol
                    );
                    log::warn!("audio response unavailable: {error}");
                    unavailable_latched = permanent;
                    if permanent {
                        states.send_replace(AudioServiceState::Closed);
                    } else {
                        retry_deadline = Some(Instant::now() + backoff);
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        states.send_replace(AudioServiceState::Backoff);
                    }
                }
            }
        }

        let Some(capture) = backend.as_mut() else {
            continue;
        };
        match capture.snapshot(monotonic_now_ns()) {
            Ok(None) => {}
            Ok(Some(frame)) => {
                frames.send_replace(Some(frame));
            }
            Err(error) => {
                log::warn!("audio response capture failed: {error}");
                frames.send_replace(None);
                drop(backend.take());
                retry_deadline = Some(Instant::now() + backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
                states.send_replace(AudioServiceState::Backoff);
            }
        }
    }
}

fn run_playback_worker<F>(
    commands: std::sync::mpsc::Receiver<PlaybackWorkerCommand>,
    states: watch::Sender<bool>,
    ownership: watch::Receiver<
        crate::wallframe::renderer_manager::RendererProcessOwnershipSnapshot,
    >,
    mut open_backend: F,
) where
    F: FnMut() -> Result<Box<dyn PlaybackObserverBackend>, super::pulse::CaptureError>,
{
    let mut backend: Option<Box<dyn PlaybackObserverBackend>> = None;
    let mut demand = false;
    let mut retry_deadline: Option<Instant> = None;
    let mut backoff = Duration::from_secs(1);
    let mut unavailable_latched = false;

    loop {
        match commands.recv_timeout(INACTIVE_POLL_INTERVAL) {
            Ok(PlaybackWorkerCommand::Demand(next)) => {
                if next == demand {
                    continue;
                }
                demand = next;
                retry_deadline = None;
                backoff = Duration::from_secs(1);
                unavailable_latched = false;
                if !demand {
                    drop(backend.take());
                    set_playback_state(&states, false);
                }
            }
            Ok(PlaybackWorkerCommand::Shutdown)
            | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                set_playback_state(&states, false);
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }

        if !demand {
            continue;
        }

        if backend.is_none()
            && !unavailable_latched
            && retry_deadline.is_none_or(|deadline| Instant::now() >= deadline)
        {
            match open_backend() {
                Ok(observer) => {
                    backend = Some(observer);
                    retry_deadline = None;
                    backoff = Duration::from_secs(1);
                    log::info!("other audio: PulseAudio playback observer active");
                }
                Err(error) => {
                    let permanent = matches!(
                        error.kind,
                        CaptureErrorKind::LibraryUnavailable | CaptureErrorKind::MissingSymbol
                    );
                    log::warn!("other audio unavailable: {error}");
                    unavailable_latched = permanent;
                    if !permanent {
                        retry_deadline = Some(Instant::now() + backoff);
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                    }
                    set_playback_state(&states, false);
                }
            }
        }

        let Some(observer) = backend.as_mut() else {
            continue;
        };
        match observer.snapshot() {
            Ok(streams) => {
                let active = has_external_playback(&streams, &ownership.borrow());
                set_playback_state(&states, active);
            }
            Err(error) => {
                log::warn!("other audio observer failed: {error}");
                drop(backend.take());
                retry_deadline = Some(Instant::now() + backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
                set_playback_state(&states, false);
            }
        }
    }
}

fn set_playback_state(states: &watch::Sender<bool>, active: bool) {
    if *states.borrow() != active {
        states.send_replace(active);
    }
}

fn monotonic_now_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return 0;
    }
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

#[cfg(test)]
mod tests {
    use super::super::pulse::{CaptureError, CaptureErrorKind};
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeCapture {
        generation: u64,
        stops: Arc<AtomicUsize>,
    }

    struct FakePlayback {
        stops: Arc<AtomicUsize>,
    }

    impl AudioCaptureBackend for FakeCapture {
        fn snapshot(
            &mut self,
            _captured_at_ns: u64,
        ) -> Result<Option<AudioPcmWindow>, CaptureError> {
            let _ = self.generation;
            Ok(None)
        }

        fn discard(&mut self) {}
    }

    impl Drop for FakeCapture {
        fn drop(&mut self) {
            self.stops.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl PlaybackObserverBackend for FakePlayback {
        fn snapshot(
            &mut self,
        ) -> Result<Vec<super::super::pulse::PulsePlaybackStream>, CaptureError> {
            Ok(vec![super::super::pulse::PulsePlaybackStream {
                index: 1,
                process_id: 0,
                corked: 0,
                muted: 0,
                has_nonzero_volume: 1,
            }])
        }
    }

    impl Drop for FakePlayback {
        fn drop(&mut self) {
            self.stops.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn configure_capture(
        commands: &std::sync::mpsc::Sender<CaptureWorkerCommand>,
        enabled: bool,
        demanded: bool,
    ) {
        commands
            .send(CaptureWorkerCommand::Configure { enabled, demanded })
            .unwrap();
    }

    #[test]
    fn playback_demand_opens_observer_and_closes_it_when_disabled() {
        let (commands_tx, commands_rx) = std::sync::mpsc::channel();
        let (states_tx, states_rx) = watch::channel(false);
        let (_ownership_tx, ownership_rx) = watch::channel(Default::default());
        let opens = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let test_opens = Arc::clone(&opens);
        let test_stops = Arc::clone(&stops);
        let worker = std::thread::spawn(move || {
            run_playback_worker(commands_rx, states_tx, ownership_rx, move || {
                test_opens.fetch_add(1, Ordering::Relaxed);
                Ok(Box::new(FakePlayback {
                    stops: Arc::clone(&test_stops),
                }) as Box<dyn PlaybackObserverBackend>)
            });
        });

        commands_tx
            .send(PlaybackWorkerCommand::Demand(true))
            .unwrap();
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(opens.load(Ordering::Relaxed), 1);
        assert!(*states_rx.borrow());

        commands_tx
            .send(PlaybackWorkerCommand::Demand(false))
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(stops.load(Ordering::Relaxed), 1);
        assert!(!*states_rx.borrow());

        commands_tx.send(PlaybackWorkerCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn playback_observer_missing_symbol_is_latched_until_demand_cycles() {
        let (commands_tx, commands_rx) = std::sync::mpsc::channel();
        let (states_tx, states_rx) = watch::channel(false);
        let (_ownership_tx, ownership_rx) = watch::channel(Default::default());
        let attempts = Arc::new(AtomicUsize::new(0));
        let test_attempts = Arc::clone(&attempts);
        let worker = std::thread::spawn(move || {
            run_playback_worker(commands_rx, states_tx, ownership_rx, move || {
                test_attempts.fetch_add(1, Ordering::Relaxed);
                Err(CaptureError {
                    kind: CaptureErrorKind::MissingSymbol,
                    message: "missing symbol".into(),
                })
            });
        });

        commands_tx
            .send(PlaybackWorkerCommand::Demand(true))
            .unwrap();
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        assert!(!*states_rx.borrow());

        commands_tx
            .send(PlaybackWorkerCommand::Demand(false))
            .unwrap();
        commands_tx
            .send(PlaybackWorkerCommand::Demand(true))
            .unwrap();
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        assert!(!*states_rx.borrow());

        commands_tx.send(PlaybackWorkerCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn warm_idle_reuses_then_closes_capture() {
        let (commands_tx, commands_rx) = std::sync::mpsc::channel();
        let (frames_tx, _) = watch::channel(None);
        let (states_tx, states_rx) = watch::channel(AudioServiceState::Closed);
        let opens = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let test_opens = Arc::clone(&opens);
        let test_stops = Arc::clone(&stops);
        let worker = std::thread::spawn(move || {
            run_worker(
                commands_rx,
                frames_tx,
                states_tx,
                Duration::from_millis(40),
                move || {
                    let generation = test_opens.fetch_add(1, Ordering::Relaxed) as u64 + 1;
                    Ok(Box::new(FakeCapture {
                        generation,
                        stops: Arc::clone(&test_stops),
                    }) as Box<dyn AudioCaptureBackend>)
                },
            )
        });

        configure_capture(&commands_tx, true, true);
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(*states_rx.borrow(), AudioServiceState::Active);
        configure_capture(&commands_tx, true, false);
        std::thread::sleep(Duration::from_millis(10));
        configure_capture(&commands_tx, true, true);
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(opens.load(Ordering::Relaxed), 1);

        configure_capture(&commands_tx, true, false);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(*states_rx.borrow(), AudioServiceState::Closed);
        assert_eq!(stops.load(Ordering::Relaxed), 1);
        commands_tx.send(CaptureWorkerCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn disabling_capture_closes_immediately_and_preserves_demand() {
        let (commands_tx, commands_rx) = std::sync::mpsc::channel();
        let (frames_tx, _) = watch::channel(None);
        let (states_tx, states_rx) = watch::channel(AudioServiceState::Closed);
        let opens = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let test_opens = Arc::clone(&opens);
        let test_stops = Arc::clone(&stops);
        let worker = std::thread::spawn(move || {
            run_worker(
                commands_rx,
                frames_tx,
                states_tx,
                Duration::from_secs(1),
                move || {
                    let generation = test_opens.fetch_add(1, Ordering::Relaxed) as u64 + 1;
                    Ok(Box::new(FakeCapture {
                        generation,
                        stops: Arc::clone(&test_stops),
                    }) as Box<dyn AudioCaptureBackend>)
                },
            )
        });

        configure_capture(&commands_tx, true, true);
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(*states_rx.borrow(), AudioServiceState::Active);
        assert_eq!(opens.load(Ordering::Relaxed), 1);

        configure_capture(&commands_tx, false, true);
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(*states_rx.borrow(), AudioServiceState::Closed);
        assert_eq!(stops.load(Ordering::Relaxed), 1);

        configure_capture(&commands_tx, true, true);
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(*states_rx.borrow(), AudioServiceState::Active);
        assert_eq!(opens.load(Ordering::Relaxed), 2);

        commands_tx.send(CaptureWorkerCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn missing_library_is_latched_until_demand_cycles() {
        let (commands_tx, commands_rx) = std::sync::mpsc::channel();
        let (frames_tx, _) = watch::channel(None);
        let (states_tx, _) = watch::channel(AudioServiceState::Closed);
        let attempts = Arc::new(AtomicUsize::new(0));
        let test_attempts = Arc::clone(&attempts);
        let worker = std::thread::spawn(move || {
            run_worker(
                commands_rx,
                frames_tx,
                states_tx,
                Duration::from_millis(10),
                move || {
                    test_attempts.fetch_add(1, Ordering::Relaxed);
                    Err(CaptureError {
                        kind: CaptureErrorKind::LibraryUnavailable,
                        message: "missing".to_string(),
                    })
                },
            )
        });
        configure_capture(&commands_tx, true, true);
        std::thread::sleep(Duration::from_millis(25));
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        configure_capture(&commands_tx, true, false);
        configure_capture(&commands_tx, true, true);
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        commands_tx.send(CaptureWorkerCommand::Shutdown).unwrap();
        worker.join().unwrap();
    }
}
