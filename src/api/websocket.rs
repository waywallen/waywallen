use std::sync::Arc;

use anyhow::{anyhow, Result};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast::error::RecvError, mpsc};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::{tungstenite::protocol::Message, WebSocketStream};
use tokio_util::sync::CancellationToken;

use crate::application;
use crate::catalog::properties::{
    canonical_user_property_key, dedupe_predefined_schema, is_daemon_display_property_key,
    WallpaperLayoutOverride,
};
use crate::control_proto as pb;
use crate::error::{ok_response, Error};
use crate::events::GlobalEvent;
use crate::model::repo;
use crate::playback;
use crate::settings::{
    remote_content_dir, SettingsStore, WallpaperFilterState, WallpaperSortRuleState,
};
use crate::tasks;
use crate::wallframe::renderer_manager;
use crate::wallframe::routing::{
    DisplaySnapshot, LayoutSource, LibrarySnapshot, RendererSnapshot, RouterEvent,
    RuntimeCondition, RuntimeConditionKind, RuntimeConditionOrigin,
};
use crate::DaemonContext;

mod handlers;
mod mapping;
mod wire;

use handlers::*;
use mapping::*;
use wire::*;

pub async fn bind(
    state: Arc<DaemonContext>,
    addr: &str,
) -> Result<(
    std::net::SocketAddr,
    impl std::future::Future<Output = Result<()>>,
)> {
    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    log::info!("ws control plane listening on {local_addr}");
    let fut = accept_loop(state, listener);
    Ok((local_addr, fut))
}

const WS_FRAME_QUEUE_CAP: usize = 512;

type WsStream = WebSocketStream<TcpStream>;
type WsSink = SplitSink<WsStream, Message>;
type WsSource = SplitStream<WsStream>;
type WsWriterTask = JoinHandle<Result<()>>;

#[derive(Clone)]
struct ServerFrameSink {
    tx: mpsc::Sender<pb::ServerFrame>,
}

impl ServerFrameSink {
    fn response(&self, response: pb::Response) -> Result<()> {
        self.frame(wrap_response(response))
    }

    fn event(&self, event: pb::Event) -> Result<()> {
        self.frame(wrap_event(event))
    }

    fn frame(&self, frame: pb::ServerFrame) -> Result<()> {
        self.tx.try_send(frame).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => anyhow!("ws frame queue full"),
            mpsc::error::TrySendError::Closed(_) => anyhow!("ws frame queue closed"),
        })
    }
}

fn spawn_response_task<F>(
    requests: &mut JoinSet<()>,
    peer: std::net::SocketAddr,
    request_id: u64,
    frames: ServerFrameSink,
    cancel: CancellationToken,
    fut: F,
) where
    F: std::future::Future<Output = pb::Response> + Send + 'static,
{
    requests.spawn(async move {
        tokio::select! {
            _ = cancel.cancelled() => {
                log::debug!("ws {peer}: request {request_id} cancelled");
            }
            response = fut => {
                if cancel.is_cancelled() {
                    log::debug!("ws {peer}: request {request_id} completed after cancellation");
                    return;
                }
                if let Err(e) = frames.response(response) {
                    log::warn!("ws {peer}: dropping response {request_id}: {e}");
                }
            }
        }
    });
}

enum ClientFrame {
    Request(pb::Request),
    DecodeError(pb::Response),
    Ignore,
    Close,
}

fn decode_client_frame(msg: Message) -> ClientFrame {
    let bytes = match msg {
        Message::Binary(b) => b,
        Message::Text(t) => t.into_bytes(),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => return ClientFrame::Ignore,
        Message::Close(_) => return ClientFrame::Close,
    };

    match pb::Request::decode(&bytes[..]) {
        Ok(req) => ClientFrame::Request(req),
        Err(error) => {
            log::error!("request decode failed: {error}");
            ClientFrame::DecodeError(Error::Decode(error).to_response(0))
        }
    }
}

struct WsSession {
    state: Arc<DaemonContext>,
    peer: std::net::SocketAddr,
    frames: ServerFrameSink,
    cancel: CancellationToken,
    src: WsSource,
    writer_task: WsWriterTask,
    writer_completed: bool,
    events_rx: tokio::sync::broadcast::Receiver<RouterEvent>,
    global_rx: tokio::sync::broadcast::Receiver<GlobalEvent>,
    task_rx: tokio::sync::broadcast::Receiver<tasks::TaskEvent>,
    requests: JoinSet<()>,
    last_wallpaper_presentations: Option<Vec<crate::wallframe::routing::WallpaperPresentationInfo>>,
}

impl WsSession {
    fn new(
        state: Arc<DaemonContext>,
        peer: std::net::SocketAddr,
        frames: ServerFrameSink,
        cancel: CancellationToken,
        src: WsSource,
        writer_task: WsWriterTask,
    ) -> Self {
        // Subscribe to router events before snapshotting so no updates get
        // dropped between the snapshot and the live stream starting.
        let events_rx = state.router.subscribe_events();
        let global_rx = state.events.subscribe();
        let task_rx = state.tasks.subscribe();
        Self {
            state,
            peer,
            frames,
            cancel,
            src,
            writer_task,
            writer_completed: false,
            events_rx,
            global_rx,
            task_rx,
            requests: JoinSet::new(),
            last_wallpaper_presentations: None,
        }
    }

    async fn run(&mut self) -> Result<()> {
        self.send_initial_events().await?;

        let mut global_events_open = true;
        let mut task_events_open = true;
        let mut router_events_open = true;
        loop {
            tokio::select! {
                writer = &mut self.writer_task => {
                    self.writer_completed = true;
                    return match writer {
                        Ok(result) => result,
                        Err(e) => Err(e.into()),
                    };
                }
                msg = self.src.next() => {
                    let Some(msg) = msg else { return Ok(()) };
                    if !self.handle_client_frame(msg?)? {
                        return Ok(());
                    }
                }
                gevt = self.global_rx.recv(), if global_events_open => {
                    match gevt {
                        Ok(e) => self.handle_global_event(e).await?,
                        Err(RecvError::Lagged(n)) => self.handle_global_lag(n).await?,
                        Err(RecvError::Closed) => global_events_open = false,
                    }
                }
                tevt = self.task_rx.recv(), if task_events_open => {
                    match tevt {
                        Ok(_) => self.frames.event(status_sync_event(&self.state).await)?,
                        Err(RecvError::Lagged(n)) => {
                            log::warn!("ws {}: task event lag {n}", self.peer);
                            self.frames.event(status_sync_event(&self.state).await)?;
                        }
                        Err(RecvError::Closed) => task_events_open = false,
                    }
                }
                evt = self.events_rx.recv(), if router_events_open => {
                    match evt {
                        Ok(e) => {
                            let affects_presentations =
                                router_event_affects_wallpaper_presentations(&e);
                            let pe = router_event_to_pb(e, &self.state.settings);
                            self.frames.event(pe)?;
                            if affects_presentations {
                                self.send_wallpaper_presentations_if_changed().await?;
                            }
                        }
                        Err(RecvError::Lagged(n)) => self.handle_router_lag(n).await?,
                        Err(RecvError::Closed) => {
                            log::info!("ws {}: router event channel closed", self.peer);
                            router_events_open = false;
                        }
                    }
                }
                joined = self.requests.join_next(), if !self.requests.is_empty() => {
                    if let Some(Err(error)) = joined {
                        log::warn!("ws {}: request task join failed: {error}", self.peer);
                    }
                }
            }
        }
    }

    async fn shutdown(mut self) {
        self.cancel.cancel();
        drop(self.frames);
        if !self.writer_completed {
            self.writer_task.abort();
            let _ = self.writer_task.await;
        }
        self.requests.abort_all();
        while self.requests.join_next().await.is_some() {}
    }

    async fn send_initial_events(&mut self) -> Result<()> {
        self.send_router_snapshot().await?;

        let libraries = application::list_library_snapshots(&self.state.db).await;
        self.frames.event(libraries_replace_event(libraries))?;

        self.send_global_snapshot().await
    }

    async fn send_router_snapshot(&mut self) -> Result<()> {
        let snap = self.state.router.snapshot_displays().await;
        self.frames
            .event(displays_replace_event(snap, &self.state.settings))?;

        self.frames.event(canvases_replace_event(
            self.state.router.snapshot_canvases().await,
        ))?;

        let snap = self.state.router.snapshot_renderers().await;
        self.frames
            .event(renderers_replace_event(snap, &self.state.settings))?;
        self.send_wallpaper_presentations(true).await?;
        Ok(())
    }

    async fn send_wallpaper_presentations_if_changed(&mut self) -> Result<()> {
        self.send_wallpaper_presentations(false).await
    }

    async fn send_wallpaper_presentations(&mut self, force: bool) -> Result<()> {
        let snapshot = self.state.router.snapshot_wallpaper_presentations().await;
        if !force
            && self
                .last_wallpaper_presentations
                .as_ref()
                .is_some_and(|previous| previous == &snapshot)
        {
            return Ok(());
        }
        self.frames
            .event(wallpaper_presentations_event(&snapshot))?;
        self.last_wallpaper_presentations = Some(snapshot);
        Ok(())
    }

    async fn send_global_snapshot(&self) -> Result<()> {
        self.frames.event(status_sync_event(&self.state).await)?;
        self.frames
            .event(playlist_changed_event(&self.state).await)?;
        Ok(())
    }

    fn handle_client_frame(&mut self, msg: Message) -> Result<bool> {
        match decode_client_frame(msg) {
            ClientFrame::Request(req) => {
                let request_id = req.request_id;
                let state = self.state.clone();
                spawn_response_task(
                    &mut self.requests,
                    self.peer,
                    request_id,
                    self.frames.clone(),
                    self.cancel.clone(),
                    async move { dispatch(&state, req).await },
                );
            }
            ClientFrame::DecodeError(resp) => self.frames.response(resp)?,
            ClientFrame::Ignore => {}
            ClientFrame::Close => return Ok(false),
        }
        Ok(true)
    }

    async fn handle_global_event(&self, e: GlobalEvent) -> Result<()> {
        if matches!(e, GlobalEvent::PlaylistChanged) {
            self.frames
                .event(playlist_changed_event(&self.state).await)?;
        } else if let Some(pe) = global_event_to_pb(&e, &self.state) {
            self.frames.event(pe)?;
        }
        if matches!(e, GlobalEvent::StatusChanged) {
            self.frames.event(status_sync_event(&self.state).await)?;
        }
        Ok(())
    }

    async fn handle_global_lag(&self, n: u64) -> Result<()> {
        log::warn!("ws {}: global event lag {n}", self.peer);
        self.send_global_snapshot().await
    }

    async fn handle_router_lag(&mut self, n: u64) -> Result<()> {
        log::warn!("ws {}: event lag {n}; resending full snapshot", self.peer);
        self.send_router_snapshot().await
    }
}

async fn accept_loop(state: Arc<DaemonContext>, listener: TcpListener) -> Result<()> {
    let mut shutdown = state.shutdown_subscribe();
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let state = state.clone();
                connections.spawn(async move {
                    if let Err(e) = handle_conn(state, stream, peer).await {
                        log::warn!("ws conn {peer} ended: {e}");
                    }
                });
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = joined {
                    log::warn!("ws connection task join failed: {error}");
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
        }
    }

    connections.abort_all();
    while connections.join_next().await.is_some() {}
    Ok(())
}

async fn handle_conn(
    state: Arc<DaemonContext>,
    stream: TcpStream,
    peer: std::net::SocketAddr,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    log::debug!("ws conn {peer} open");
    let (sink, src) = ws.split();
    let (frame_tx, frame_rx) = mpsc::channel::<pb::ServerFrame>(WS_FRAME_QUEUE_CAP);
    let frames = ServerFrameSink { tx: frame_tx };
    let writer_task = spawn_ws_writer(sink, frame_rx);
    let cancel = CancellationToken::new();
    let mut session = WsSession::new(state, peer, frames, cancel, src, writer_task);
    let result = session.run().await;
    session.shutdown().await;
    result?;
    log::debug!("ws conn {peer} closed");
    Ok(())
}

fn spawn_ws_writer(
    mut sink: WsSink,
    mut frame_rx: mpsc::Receiver<pb::ServerFrame>,
) -> WsWriterTask {
    tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            sink.send(Message::Binary(frame.encode_to_vec())).await?;
        }
        Ok::<(), anyhow::Error>(())
    })
}

#[cfg(test)]
mod tests;
