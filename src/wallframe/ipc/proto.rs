pub use crate::wallframe::ipc::generated::{
    AudioStreamFormat, AudioWindow, BindFailure, BufferAllocationFailureKind, BufferDirective,
    BufferFormat, BufferMemorySource, BufferPath, BufferPool, ControlTransition, DecodeError,
    DrmNode as WireDrmNode, Event as EventMsg, EventIn as ControlMsg, EventSubscription,
    EventSubscriptionResult, EventSubscriptionStatus, Extent, Frame, InitRejection, LogLevel,
    MediaPlaybackState, MprisSnapshot as WireMprisSnapshot, MprisTimeline, MprisV2Snapshot,
    PointerAxis, PointerAxisSource, PointerButton, PointerButtonState, PointerMotion,
    ProducerCapabilities, RenderTargetCapabilities, RenderTargetConfig, RenderTargetOutcome,
    RendererInit, RendererState, RgbaColor, PROTOCOL_NAME, PROTOCOL_VERSION,
};

pub const RENDERER_STATE_FIELD_CLEAR_COLOR: u32 = 1 << 0;
pub const RENDERER_STATE_FIELD_RUNTIME_TAGS: u32 = 1 << 1;
pub const RENDERER_STATE_KNOWN_FIELDS: u32 =
    RENDERER_STATE_FIELD_CLEAR_COLOR | RENDERER_STATE_FIELD_RUNTIME_TAGS;
