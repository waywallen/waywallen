pub(crate) mod websocket;

/// Revision of the control plane a client has to speak to be served:
/// the `Daemon1` D-Bus interface plus the protobuf WebSocket API
/// behind `WsPort`. Bumped only when a client built against the
/// previous revision can no longer be served. Adding an optional
/// request does not bump it — it gets a capability name instead.
pub const CONTROL_REVISION: &str = "control.v1";

/// Advertised on `Daemon1.Capabilities`: the revision first, then the
/// optional features this daemon serves, checked by name before use.
/// `Version` stays what it always was, a build identity; this is the
/// contract. A name is added here only when the feature is genuinely
/// optional, i.e. when an older daemon answers a request for it with
/// `ERROR_CODE_UNEXPECTED_PAYLOAD` or with a default-valued field.
pub const CONTROL_CAPABILITIES: &[&str] = &[
    CONTROL_REVISION,
    // `LogReadRequest` / `SettingsGetResponse.log_dir`, since 0.3.8.
    "log-read",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_lead_with_the_revision() {
        assert_eq!(CONTROL_CAPABILITIES.first(), Some(&CONTROL_REVISION));
    }

    #[test]
    fn capability_names_are_unique_and_wire_safe() {
        let mut seen = std::collections::HashSet::new();
        for name in CONTROL_CAPABILITIES {
            assert!(seen.insert(*name), "duplicate capability name: {name}");
            assert!(!name.is_empty(), "empty capability name");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-'),
                "capability name is not [a-z0-9.-]: {name}"
            );
        }
    }
}
