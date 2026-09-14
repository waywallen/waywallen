#[allow(dead_code, clippy::all)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/display_proto_generated.rs"));
}

pub mod codec;

pub use codec::{
    recv_event, recv_request, send_event, send_request, CodecError, CodecResult, MAX_BODY_BYTES,
    MAX_FDS_PER_MSG,
};
pub use generated::{
    opcode, BlurEffectConfig, BufferImportFailureKind, CompositionConfig, ConsumerCapabilities,
    DecodeError, DisplayErrorCode, DisplayMetrics, Event, PauseEffectConfig, PauseEffectKind,
    PauseEffectState, PointerAxisSource, PointerButtonState, PresentationCapabilities,
    PresentationConfig, PresentationSnapshot, PresentationState, Rect, Request, RgbaColor,
    TransitionConfig, TransitionKind, PROTOCOL_NAME, PROTOCOL_VERSION,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_req(req: Request) {
        let mut buf = Vec::new();
        req.encode(&mut buf);
        let decoded = Request::decode(req.opcode(), &buf).expect("decode");
        assert_eq!(decoded, req);
    }

    fn roundtrip_evt(evt: Event) {
        let mut buf = Vec::new();
        evt.encode(&mut buf);
        let decoded = Event::decode(evt.opcode(), &buf).expect("decode");
        assert_eq!(decoded, evt);
    }

    #[test]
    fn request_register_roundtrip() {
        roundtrip_req(Request::RegisterDisplay {
            name: "DP-1".to_string(),
            instance_id: "f47ac10b-58cc-4372-a567-0e02b2c3d479".to_string(),
            metrics: DisplayMetrics {
                width: 1920,
                height: 1080,
                refresh_mhz: 60000,
            },
            consumer_caps: ConsumerCapabilities {
                fourccs: vec![0x34325258],
                mod_counts: vec![1],
                modifiers: vec![0],
                plane_counts: vec![1],
                device_uuid: vec![0; 4],
                driver_uuid: vec![0; 4],
                drm_render_major: 226,
                drm_render_minor: 128,
                mem_hints: 1,
                sync_caps: 1,
                color_caps: 1,
                extent_max_w: 7680,
                extent_max_h: 4320,
            },
            presentation_caps: PresentationCapabilities { flags: 1 },
            window_state_flags: 8,
        });
    }

    #[test]
    fn event_bind_buffers_roundtrip() {
        let evt = Event::BindBuffers {
            buffer_generation: 1,
            count: 3,
            width: 1920,
            height: 1080,
            fourcc: 0x34325258,
            modifier: 0x0100000000000001,
            planes_per_buffer: 1,
            stride: vec![7680, 7680, 7680],
            plane_offset: vec![0, 0, 0],
            size: vec![8_294_400, 8_294_400, 8_294_400],
            initial_config: CompositionConfig {
                generation: 7,
                buffer_generation: 1,
                source_rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1920.0,
                    h: 1080.0,
                },
                dest_rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1920.0,
                    h: 1080.0,
                },
                transform: 0,
                clear_color: RgbaColor {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
            },
            transition: true,
        };
        // expected fds = count * planes_per_buffer = 3 * 1 = 3
        assert_eq!(evt.expected_fds(), 3);
        roundtrip_evt(evt);
    }

    #[test]
    fn event_set_composition_config_roundtrip() {
        roundtrip_evt(Event::SetCompositionConfig {
            config: CompositionConfig {
                generation: 7,
                buffer_generation: 1,
                source_rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1920.0,
                    h: 1080.0,
                },
                dest_rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1920.0,
                    h: 1080.0,
                },
                transform: 0,
                clear_color: RgbaColor {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
            },
        });
    }

    #[test]
    fn presentation_snapshot_roundtrip_and_noncanonical_bool_rejected() {
        let state = PresentationState {
            generation: 5,
            config_generation: 3,
            pause_effect: PauseEffectState { active: true },
        };
        let accepted = Event::DisplayAccepted {
            display_id: 9,
            presentation: PresentationSnapshot {
                config: PresentationConfig {
                    generation: 3,
                    pause_effect: PauseEffectConfig {
                        kind: PauseEffectKind::Blur,
                        blur: BlurEffectConfig { radius: 40 },
                    },
                    transition: TransitionConfig {
                        kind: TransitionKind::Wipe,
                        duration_ms: 750,
                        angle: 90,
                        origin_x: 0.5,
                        origin_y: 0.25,
                    },
                },
                state,
            },
        };
        roundtrip_evt(accepted.clone());

        let mut accepted_buf = Vec::new();
        accepted.encode(&mut accepted_buf);
        accepted_buf[16..20].copy_from_slice(&7_u32.to_le_bytes());
        assert!(matches!(
            Event::decode(accepted.opcode(), &accepted_buf),
            Err(DecodeError::UnknownEnumValue {
                enum_name: "pause_effect_kind",
                value: 7
            })
        ));

        let mut transition_buf = Vec::new();
        accepted.encode(&mut transition_buf);
        transition_buf[24..28].copy_from_slice(&9_u32.to_le_bytes());
        assert!(matches!(
            Event::decode(accepted.opcode(), &transition_buf),
            Err(DecodeError::UnknownEnumValue {
                enum_name: "transition_kind",
                value: 9
            })
        ));

        let event = Event::SetPresentationState { state };
        let mut buf = Vec::new();
        event.encode(&mut buf);
        buf[16..20].copy_from_slice(&2_u32.to_le_bytes());
        assert!(matches!(
            Event::decode(event.opcode(), &buf),
            Err(DecodeError::BadBool)
        ));
    }

    #[test]
    fn event_frame_ready_fds() {
        let evt = Event::FrameReady {
            buffer_generation: 1,
            buffer_index: 0,
            seq: 100,
        };
        // v1: 2 fds — acquire sync_fd + release_syncobj.
        assert_eq!(evt.expected_fds(), 2);
        roundtrip_evt(evt);
    }

    #[test]
    fn event_error_roundtrip() {
        roundtrip_evt(Event::Error {
            code: DisplayErrorCode::ProtocolViolation,
            message: "protocol violation: unexpected frame_ready".to_string(),
        });
    }

    #[test]
    fn opcodes_match_spec() {
        assert_eq!(opcode::request::HELLO, 1);
        assert_eq!(opcode::request::REGISTER_DISPLAY, 2);
        assert_eq!(opcode::request::SET_DISPLAY_METRICS, 3);
        assert_eq!(opcode::request::BUFFER_IMPORT_FAILED, 7);
        assert_eq!(opcode::request::ACK_UNBIND, 11);
        assert_eq!(opcode::request::SET_WINDOW_STATE, 12);
        assert_eq!(opcode::request::FRAME_RELEASE_ARMED, 13);
        assert_eq!(opcode::event::WELCOME, 1);
        assert_eq!(opcode::event::BIND_BUFFERS, 3);
        assert_eq!(opcode::event::FRAME_READY, 5);
        assert_eq!(opcode::event::ERROR, 7);
        assert_eq!(opcode::event::SET_PRESENTATION_SNAPSHOT, 8);
        assert_eq!(opcode::event::SET_PRESENTATION_STATE, 9);
    }

    #[test]
    fn decode_trailing_bytes_rejected() {
        let mut buf = Vec::new();
        let request = Request::SetWindowState { flags: 0 };
        request.encode(&mut buf);
        buf.push(0xff);
        assert!(matches!(
            Request::decode(opcode::request::SET_WINDOW_STATE, &buf),
            Err(DecodeError::Trailing)
        ));
    }

    #[test]
    fn decode_unknown_opcode_rejected() {
        assert!(matches!(
            Request::decode(99, &[]),
            Err(DecodeError::UnknownOpcode(99))
        ));
    }
}
