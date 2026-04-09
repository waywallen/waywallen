//! `waywallen-display-v1` wire protocol bindings.
//!
//! The `generated` submodule is emitted at build time by
//! `build.rs` (via the `wayproto-gen` tool) from
//! `protocol/waywallen_display_v1.xml`. It defines `Request`,
//! `Event`, `Rect`, `DecodeError`, per-opcode constants, and the
//! binary encode/decode implementations.
//!
//! The hand-written `codec` submodule (added in a later step) layers
//! framing + `SCM_RIGHTS` ancillary fd handling on top of the
//! generated code.

#[allow(dead_code, clippy::all)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/display_proto_generated.rs"));
}

pub mod codec;

pub use codec::{
    recv_event, recv_request, send_event, send_request, CodecError, CodecResult,
    MAX_BODY_BYTES, MAX_FDS_PER_MSG,
};
pub use generated::{opcode, DecodeError, Event, Rect, Request, PROTOCOL_NAME, PROTOCOL_VERSION};

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
    fn protocol_identity() {
        assert_eq!(PROTOCOL_NAME, "waywallen-display-v1");
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn request_hello_roundtrip() {
        roundtrip_req(Request::Hello {
            protocol: PROTOCOL_NAME.to_string(),
            client_name: "libwaywallen_display".to_string(),
            client_version: "0.1.0".to_string(),
        });
    }

    #[test]
    fn request_register_roundtrip() {
        roundtrip_req(Request::RegisterDisplay {
            name: "DP-1".to_string(),
            width: 1920,
            height: 1080,
            refresh_mhz: 60000,
            properties: vec![
                ("scale".to_string(), "1.0".to_string()),
                ("hdr".to_string(), "false".to_string()),
            ],
        });
    }

    #[test]
    fn request_release_roundtrip() {
        roundtrip_req(Request::BufferRelease {
            buffer_generation: 42,
            buffer_index: 1,
            seq: 12345,
        });
    }

    #[test]
    fn request_bye_roundtrip() {
        roundtrip_req(Request::Bye);
    }

    #[test]
    fn event_welcome_roundtrip() {
        roundtrip_evt(Event::Welcome {
            server_version: "waywallen 0.1.0".to_string(),
            features: vec!["explicit_sync_fd".to_string()],
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
        };
        // expected fds = count * planes_per_buffer = 3 * 1 = 3
        assert_eq!(evt.expected_fds(), 3);
        roundtrip_evt(evt);
    }

    #[test]
    fn event_set_config_roundtrip() {
        roundtrip_evt(Event::SetConfig {
            config_generation: 7,
            source_rect: Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 },
            dest_rect: Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 },
            transform: 0,
            clear_r: 0.0,
            clear_g: 0.0,
            clear_b: 0.0,
            clear_a: 1.0,
        });
    }

    #[test]
    fn event_frame_ready_fds() {
        let evt = Event::FrameReady {
            buffer_generation: 1,
            buffer_index: 0,
            seq: 100,
        };
        assert_eq!(evt.expected_fds(), 1);
        roundtrip_evt(evt);
    }

    #[test]
    fn event_error_roundtrip() {
        roundtrip_evt(Event::Error {
            code: 42,
            message: "protocol violation: unexpected frame_ready".to_string(),
        });
    }

    #[test]
    fn opcodes_match_spec() {
        assert_eq!(opcode::request::HELLO, 1);
        assert_eq!(opcode::request::REGISTER_DISPLAY, 2);
        assert_eq!(opcode::request::UPDATE_DISPLAY, 3);
        assert_eq!(opcode::request::BUFFER_RELEASE, 4);
        assert_eq!(opcode::request::BYE, 5);
        assert_eq!(opcode::event::WELCOME, 1);
        assert_eq!(opcode::event::BIND_BUFFERS, 3);
        assert_eq!(opcode::event::FRAME_READY, 5);
        assert_eq!(opcode::event::ERROR, 7);
    }

    #[test]
    fn decode_trailing_bytes_rejected() {
        let mut buf = Vec::new();
        Request::Bye.encode(&mut buf);
        buf.push(0xff);
        assert!(matches!(
            Request::decode(opcode::request::BYE, &buf),
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
