use super::*;

#[test]
fn timeline_preview_remains_an_optional_path_on_the_wire() {
    use prost::Message;
    for preview in ["", "/wallpapers/metadata.json", "/backgrounds/entry.xml"] {
        let entry = pb::WallpaperEntry {
            resource: "/backgrounds/entry.xml".into(),
            preview: preview.into(),
            sub_type: pb::WallpaperSubType::Timeline as i32,
            ..Default::default()
        };
        let decoded = pb::WallpaperEntry::decode(entry.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded.preview, preview);
        assert_eq!(decoded.resource, entry.resource);
        assert_eq!(decoded.sub_type, pb::WallpaperSubType::Timeline as i32);
    }
}

#[test]
fn library_edit_protocol_round_trip() {
    use prost::Message;
    let edit = pb::LibraryEditRequest {
        id: 17,
        name: "Desktop wallpapers".into(),
        settings: std::collections::HashMap::from([("scan_mode".into(), "wallpapers".into())]),
    };
    let request = pb::Request {
        request_id: 42,
        payload: Some(pb::request::Payload::LibraryEdit(edit.clone())),
    };
    let decoded = pb::Request::decode(request.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.request_id, 42);
    assert_eq!(
        decoded.payload,
        Some(pb::request::Payload::LibraryEdit(edit))
    );
}

#[test]
fn library_names_protocol_round_trip() {
    use prost::Message;
    let snapshot = LibrarySnapshot {
        settings: Default::default(),
        id: 17,
        name: "My wallpapers".into(),
        path: "/a/wallpapers".into(),
        plugin_name: "image".into(),
    };
    let event = libraries_replace_event(vec![snapshot.clone()]);
    let instance = library_instance_to_pb(snapshot);
    let decoded = pb::LibraryInstance::decode(instance.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.name, "My wallpapers");
    assert_eq!(decoded.id, 17);
    let Some(pb::event::Payload::LibrarySnapshot(event)) = event.payload else {
        panic!("expected library snapshot");
    };
    assert_eq!(event.libraries, [decoded]);
    let add = pb::LibraryAddRequest {
        settings: Default::default(),
        path: "/a".into(),
        plugin_name: "image".into(),
        name: None,
    };
    assert!(
        pb::LibraryAddRequest::decode(add.encode_to_vec().as_slice())
            .unwrap()
            .name
            .is_none()
    );
    let named = pb::LibraryAddRequest {
        name: Some("Same name".into()),
        ..add
    };
    assert_eq!(
        pb::LibraryAddRequest::decode(named.encode_to_vec().as_slice())
            .unwrap()
            .name
            .as_deref(),
        Some("Same name")
    );
}

#[test]
fn library_visibility_presence_and_validation() {
    use prost::Message;
    let absent =
        pb::GlobalSettings::decode(pb::GlobalSettings::default().encode_to_vec().as_slice())
            .unwrap();
    assert_eq!(
        library_visibility_from_pb(absent.wallpaper_library_visibility.as_ref()).unwrap(),
        None
    );
    // The former include-only selection used tag 30, with packed library_ids at tag 1.
    let legacy = pb::GlobalSettings::decode(&[0xf2, 0x01, 0x03, 0x0a, 0x01, 0x02][..]).unwrap();
    assert!(legacy.wallpaper_library_visibility.is_none());
    let mut settings = crate::settings::GlobalSettings::default();
    settings.wallpaper_hidden_library_ids = vec![9, 2, 9];
    let encoded = global_to_pb(&settings).encode_to_vec();
    let decoded = pb::GlobalSettings::decode(encoded.as_slice()).unwrap();
    assert_eq!(
        library_visibility_from_pb(decoded.wallpaper_library_visibility.as_ref()).unwrap(),
        Some(vec![2, 9])
    );
    settings.wallpaper_hidden_library_ids.clear();
    let decoded =
        pb::GlobalSettings::decode(global_to_pb(&settings).encode_to_vec().as_slice()).unwrap();
    assert_eq!(
        library_visibility_from_pb(decoded.wallpaper_library_visibility.as_ref()).unwrap(),
        Some(vec![])
    );
    for id in [0, -1] {
        assert!(
            library_visibility_from_pb(Some(&pb::WallpaperLibraryVisibility {
                hidden_library_ids: vec![id]
            }))
            .is_err()
        );
    }
}

#[test]
fn auto_replay_scope_roundtrip_and_invalid_writes() {
    for scope in [
        crate::settings::AutoScope::CurrentDisplay,
        crate::settings::AutoScope::AllDisplays,
    ] {
        let policy = crate::settings::AutoReplayPolicy {
            fullscreen: crate::settings::AutoAction::Stop,
            fullscreen_scope: scope,
            ..Default::default()
        };
        assert_eq!(
            auto_replay_from_pb(&auto_replay_to_pb(&policy)).unwrap(),
            policy
        );
    }
    let policy = crate::settings::AutoReplayPolicy {
        fullscreen_scope: crate::settings::AutoScope::AllDisplays,
        ..Default::default()
    };
    assert_eq!(
        auto_replay_from_pb(&auto_replay_to_pb(&policy)).unwrap(),
        policy
    );
    for invalid in [
        pb::AutoReplayPolicy {
            focused: pb::AutoAction::Stop as i32,
            ..Default::default()
        },
        pb::AutoReplayPolicy {
            any_window: pb::AutoAction::Stop as i32,
            ..Default::default()
        },
        pb::AutoReplayPolicy {
            fullscreen_scope: 99,
            ..Default::default()
        },
        pb::AutoReplayPolicy {
            fullscreen: 99,
            ..Default::default()
        },
    ] {
        assert!(auto_replay_from_pb(&invalid).is_err());
    }
    let normalized = auto_replay_from_pb(&pb::AutoReplayPolicy {
        fullscreen: pb::AutoAction::Mute as i32,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        normalized.fullscreen_scope,
        crate::settings::AutoScope::AllDisplays
    );
}

#[test]
fn auto_replay_resume_delay_round_trips_defaults_and_clamps() {
    let policy = crate::settings::AutoReplayPolicy {
        resume_delay_ms: 750,
        ..Default::default()
    };
    assert_eq!(
        auto_replay_from_pb(&auto_replay_to_pb(&policy)).unwrap(),
        policy
    );

    let defaulted = auto_replay_from_pb(&pb::AutoReplayPolicy::default()).unwrap();
    assert_eq!(
        defaulted.resume_delay_ms,
        crate::settings::DEFAULT_AUTO_REPLAY_RESUME_DELAY_MS
    );

    let clamped = auto_replay_from_pb(&pb::AutoReplayPolicy {
        resume_delay_ms: Some(u32::MAX),
        ..Default::default()
    })
    .unwrap();
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

#[test]
fn library_filter_rules_round_trip_and_reject_invalid() {
    use crate::catalog::query::{FilterPredicate, FilterRule, WallpaperSelection};
    use crate::model::playlist_source::PlaylistSource;
    use crate::settings::WallpaperFilterState;

    for exclude in [false, true] {
        let rule = FilterRule {
            group: 0,
            predicate: FilterPredicate::Library { value: 42, exclude },
        };
        let wire = filter_rule_to_pb(&rule);
        assert_eq!(filter_rule_from_pb(&wire), Some(rule.clone()));
        let state = WallpaperFilterState::from_catalog(&[rule.clone()], &[]);
        assert_eq!(state.to_catalog().0, vec![rule.clone()]);
        let stored = serde_json::to_string(&state).unwrap();
        let loaded: WallpaperFilterState = serde_json::from_str(&stored).unwrap();
        assert_eq!(loaded.to_catalog().0, vec![rule.clone()]);
        let source = PlaylistSource::Smart(WallpaperSelection {
            filters: vec![rule],
            ..Default::default()
        });
        assert_eq!(
            PlaylistSource::decode_selection(&source.encode_selection().unwrap().unwrap()).unwrap(),
            source
        );
    }

    for (value, condition) in [(0, 1), (-1, 1), (42, 0), (42, 3), (42, 99)] {
        let wire = pb::WallpaperFilterRule {
            r#type: pb::WallpaperFilterType::Library as i32,
            group: 0,
            payload: Some(pb::wallpaper_filter_rule::Payload::IntFilter(
                pb::WallpaperIntFilter { value, condition },
            )),
        };
        assert!(filter_rule_from_pb(&wire).is_none());
    }
    let legacy = pb::WallpaperFilterRule {
        r#type: pb::WallpaperFilterType::Library as i32,
        group: 0,
        payload: Some(pb::wallpaper_filter_rule::Payload::StringFilter(
            pb::WallpaperStringFilter {
                value: "/same".into(),
                condition: pb::StringCondition::Is as i32,
            },
        )),
    };
    assert!(filter_rule_from_pb(&legacy).is_none());
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
#[test]
fn smart_playlist_source_validation_and_roundtrip() {
    use crate::catalog::query::{HiddenFilter, WallpaperSelection};
    use crate::model::playlist_source::PlaylistSource;
    for hidden in [
        HiddenFilter::Exclude,
        HiddenFilter::Only,
        HiddenFilter::Include,
    ] {
        let source = PlaylistSource::Smart(WallpaperSelection {
            hidden,
            ..Default::default()
        });
        let wire = playlist_source_to_pb(&source);
        assert_eq!(playlist_source_from_pb(Some(&wire), &[]).unwrap(), source);
        assert!(playlist_source_from_pb(Some(&wire), &["1".into()]).is_err());
    }
    assert!(playlist_source_from_pb(Some(&pb::PlaylistSource::default()), &[]).is_err());
    assert!(playlist_source_from_pb(None, &["invalid".into()]).is_err());
    assert!(playlist_source_from_pb(None, &["0".into()]).is_err());
    let mut request = pb::WallpaperListRequest::default();
    request.filters.push(pb::WallpaperFilterRule {
        r#type: 1000,
        ..Default::default()
    });
    assert!(selection_from_request(&request).is_err());
    request.filters.clear();
    request.filter_logics.push(pb::FilterLogic {
        op: 1,
        group_a: 0,
        group_b: 1,
    });
    assert!(selection_from_request(&request).is_err());
    request.filter_logics.clear();
    request.sorts.push(pb::WallpaperSortRule {
        key: 1,
        direction: 99,
    });
    assert!(selection_from_request(&request).is_err());
}
