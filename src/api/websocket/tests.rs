use super::*;

#[test]
fn auto_replay_resume_delay_round_trips_defaults_and_clamps() {
    let policy = crate::settings::AutoReplayPolicy {
        resume_delay_ms: 750,
        ..Default::default()
    };
    assert_eq!(auto_replay_from_pb(&auto_replay_to_pb(&policy)), policy);

    let defaulted = auto_replay_from_pb(&pb::AutoReplayPolicy::default());
    assert_eq!(
        defaulted.resume_delay_ms,
        crate::settings::DEFAULT_AUTO_REPLAY_RESUME_DELAY_MS
    );

    let clamped = auto_replay_from_pb(&pb::AutoReplayPolicy {
        resume_delay_ms: Some(u32::MAX),
        ..Default::default()
    });
    assert_eq!(
        clamped.resume_delay_ms,
        crate::settings::MAX_AUTO_REPLAY_RESUME_DELAY_MS
    );
}

#[test]
fn pause_effect_settings_round_trip_and_clamp() {
    use crate::settings::{BlurEffectConfig, PauseEffectConfig, PauseEffectKind};

    for kind in [PauseEffectKind::None, PauseEffectKind::Blur] {
        let config = PauseEffectConfig {
            kind,
            blur: BlurEffectConfig { radius: 48 },
        };
        assert_eq!(pause_effect_from_pb(&pause_effect_to_pb(config)), config);
    }

    let clamped = pause_effect_from_pb(&pb::PauseEffectConfig {
        kind: pb::PauseEffectKind::Blur as i32,
        blur: Some(pb::BlurEffectConfig { radius: u32::MAX }),
    });
    assert_eq!(clamped.blur.radius, crate::settings::MAX_BLUR_EFFECT_RADIUS);

    let defaulted = pause_effect_from_pb(&pb::PauseEffectConfig {
        kind: pb::PauseEffectKind::Blur as i32,
        blur: None,
    });
    assert_eq!(
        defaulted.blur.radius,
        crate::settings::DEFAULT_BLUR_EFFECT_RADIUS
    );

    let minimum = pause_effect_from_pb(&pb::PauseEffectConfig {
        kind: pb::PauseEffectKind::Blur as i32,
        blur: Some(pb::BlurEffectConfig { radius: 0 }),
    });
    assert_eq!(minimum.blur.radius, crate::settings::MIN_BLUR_EFFECT_RADIUS);
}

#[test]
fn transition_settings_round_trip_and_clamp() {
    use crate::settings::{TransitionConfig, TransitionKind, TransitionOrigin};

    for kind in [
        TransitionKind::None,
        TransitionKind::Fade,
        TransitionKind::Wipe,
        TransitionKind::Grow,
    ] {
        let config = TransitionConfig {
            kind,
            duration_ms: 1200,
            angle: 270,
            origin: TransitionOrigin { x: 10, y: 90 },
        };
        assert_eq!(transition_from_pb(&transition_to_pb(config)), config);
    }

    let clamped = transition_from_pb(&pb::TransitionConfig {
        kind: pb::TransitionKind::Grow as i32,
        duration_ms: u32::MAX,
        angle: 450,
        origin_x: 250,
        origin_y: 101,
    });
    assert_eq!(
        clamped.duration_ms,
        crate::settings::MAX_TRANSITION_DURATION_MS
    );
    assert_eq!(clamped.angle, 90);
    assert_eq!(clamped.origin, TransitionOrigin { x: 100, y: 100 });

    let minimum = transition_from_pb(&pb::TransitionConfig {
        kind: pb::TransitionKind::Fade as i32,
        duration_ms: 0,
        ..Default::default()
    });
    assert_eq!(
        minimum.duration_ms,
        crate::settings::MIN_TRANSITION_DURATION_MS
    );

    let unknown = transition_from_pb(&pb::TransitionConfig {
        kind: 99,
        ..Default::default()
    });
    assert_eq!(unknown.kind, TransitionKind::None);
}

#[test]
fn catalog_filter_mapping_preserves_legacy_tag_payload() {
    let wire = pb::WallpaperFilterRule {
        r#type: pb::WallpaperFilterType::Tag as i32,
        group: 4,
        payload: Some(pb::wallpaper_filter_rule::Payload::StringFilter(
            pb::WallpaperStringFilter {
                value: "Nature".into(),
                condition: pb::StringCondition::Contains as i32,
            },
        )),
    };
    let mapped = filter_rule_from_pb(&wire).unwrap();
    assert_eq!(mapped.group, 4);
    assert_eq!(
        mapped.predicate,
        crate::catalog::query::FilterPredicate::Tags {
            values: vec!["Nature".into()],
            condition: crate::catalog::query::StringMatch::Contains,
        }
    );
    assert!(filter_rule_from_pb(&pb::WallpaperFilterRule {
        r#type: pb::WallpaperFilterType::Width as i32,
        group: 0,
        payload: None,
    })
    .is_none());
}

#[test]
fn catalog_query_wire_mapping_round_trips() {
    let rule = crate::catalog::FilterRule {
        group: 2,
        predicate: crate::catalog::query::FilterPredicate::Size {
            value: 4096,
            condition: crate::catalog::query::IntMatch::GreaterEqual,
        },
    };
    assert_eq!(filter_rule_from_pb(&filter_rule_to_pb(&rule)), Some(rule));

    let sort = crate::catalog::SortRule {
        key: crate::catalog::query::SortKey::LastModified,
        direction: crate::catalog::query::SortDirection::Descending,
    };
    assert_eq!(sort_rule_from_pb(&sort_rule_to_pb(&sort)), Some(sort));
}

fn health_response(request_id: u64) -> pb::Response {
    ok_response(
        request_id,
        pb::response::Payload::Health(pb::HealthResponse {
            service: "waywallen".to_string(),
            state: "ok".to_string(),
            os_name: "Test Linux".to_string(),
        }),
    )
}

fn health_request(request_id: u64) -> pb::Request {
    pb::Request {
        request_id,
        payload: Some(pb::request::Payload::Health(pb::HealthRequest {})),
    }
}

fn response_request_id(frame: pb::ServerFrame) -> Option<u64> {
    match frame.kind {
        Some(pb::server_frame::Kind::Response(response)) => Some(response.request_id),
        _ => None,
    }
}

#[test]
fn decode_client_frame_extracts_request() {
    let bytes = health_request(7).encode_to_vec();

    match decode_client_frame(Message::Binary(bytes)) {
        ClientFrame::Request(req) => {
            assert_eq!(req.request_id, 7);
            assert!(matches!(req.payload, Some(pb::request::Payload::Health(_))));
        }
        _ => panic!("expected decoded request"),
    }
}

#[test]
fn decode_client_frame_reports_decode_error() {
    match decode_client_frame(Message::Binary(vec![0x80])) {
        ClientFrame::DecodeError(response) => {
            assert_eq!(response.request_id, 0);
            assert_eq!(response.error_code, pb::ErrorCode::Decode as i32);
        }
        _ => panic!("expected decode error response"),
    }
}

#[test]
fn decode_client_frame_ignores_ping_and_tracks_close() {
    assert!(matches!(
        decode_client_frame(Message::Ping(Vec::new())),
        ClientFrame::Ignore
    ));
    assert!(matches!(
        decode_client_frame(Message::Close(None)),
        ClientFrame::Close
    ));
}

#[tokio::test]
async fn response_tasks_complete_independently() {
    let (tx, mut rx) = mpsc::channel::<pb::ServerFrame>(WS_FRAME_QUEUE_CAP);
    let frames = ServerFrameSink { tx };
    let cancel = CancellationToken::new();
    let peer = "127.0.0.1:0".parse().unwrap();
    let mut requests = JoinSet::new();
    let (release_slow, slow_released) = tokio::sync::oneshot::channel::<()>();

    spawn_response_task(
        &mut requests,
        peer,
        1,
        frames.clone(),
        cancel.clone(),
        async move {
            let _ = slow_released.await;
            health_response(1)
        },
    );
    spawn_response_task(&mut requests, peer, 2, frames, cancel, async move {
        health_response(2)
    });

    let first = rx.recv().await.expect("fast response");
    assert_eq!(response_request_id(first), Some(2));

    release_slow.send(()).expect("release slow response");
    let second = rx.recv().await.expect("slow response");
    assert_eq!(response_request_id(second), Some(1));
    while requests.join_next().await.is_some() {}
}

#[test]
fn server_frame_sink_reports_full_queue() {
    let (tx, mut rx) = mpsc::channel::<pb::ServerFrame>(1);
    let frames = ServerFrameSink { tx };

    frames.response(health_response(1)).expect("first response");
    let err = frames.response(health_response(2)).unwrap_err();
    assert!(err.to_string().contains("queue full"));

    let first = rx.try_recv().expect("queued response");
    assert_eq!(response_request_id(first), Some(1));
}

#[tokio::test]
async fn response_task_cancels_pending_dispatch() {
    let (tx, mut rx) = mpsc::channel::<pb::ServerFrame>(WS_FRAME_QUEUE_CAP);
    let frames = ServerFrameSink { tx };
    let cancel = CancellationToken::new();
    let peer = "127.0.0.1:0".parse().unwrap();
    let mut requests = JoinSet::new();
    let (release_slow, slow_released) = tokio::sync::oneshot::channel::<()>();

    spawn_response_task(&mut requests, peer, 1, frames, cancel.clone(), async move {
        let _ = slow_released.await;
        health_response(1)
    });

    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !release_slow.is_closed() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled response task");

    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
    ));
    while requests.join_next().await.is_some() {}
}
