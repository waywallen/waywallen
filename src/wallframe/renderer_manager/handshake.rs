use super::*;

pub(crate) fn build_init_msg(req: &SpawnRequest, def: &RendererDef) -> ControlMsg {
    let mut settings_kv: HashMap<String, String> = req.settings.clone();

    if def.settings.contains_key("test_pattern") && req.test_pattern {
        settings_kv.insert("test_pattern".to_string(), "1".to_string());
    }

    let mut settings: Vec<(String, String)> = settings_kv.into_iter().collect();
    settings.sort_by(|a, b| a.0.cmp(&b.0));

    // Wire has no optional-scalar encoding; None -> 0,0.
    let (display_width, display_height) = req.display_size.unwrap_or((0, 0));
    ControlMsg::Init {
        config: RendererInit {
            protocol_version: crate::wallframe::ipc::proto::PROTOCOL_VERSION,
            spawn_version: SPAWN_VERSION,
            settings,
            user_properties: crate::catalog::properties::merge_renderer_user_properties(
                &req.default_user_properties,
                &req.user_property_overrides,
            ),
            display_width,
            display_height,
        },
    }
}

pub(super) fn validate_renderer_spawn_version(def: &RendererDef) -> Result<()> {
    let declared = def.spawn_version.unwrap_or(SPAWN_VERSION);
    if declared == SPAWN_VERSION {
        return Ok(());
    }
    Err(Error::RendererSpawnFailed(format!(
        "renderer spawn version mismatch: plugin '{}' renderer '{}' declares {declared}, daemon requires {SPAWN_VERSION}; update the plugin",
        def.plugin_id, def.name
    )))
}

pub(crate) fn run_init_handshake(sock: &StdUnixStream, init: &ControlMsg) -> Result<DrmNode> {
    send_control(sock, init, &[])
        .map_err(|error| Error::RendererSpawnFailed(format!("send Init: {error}")))?;
    let (event, fds) = recv_event(sock)
        .map_err(|error| Error::RendererSpawnFailed(format!("recv Ready: {error}")))?;
    match event {
        EventMsg::Ready { drm_node } => {
            if !fds.is_empty() {
                log::warn!("Ready unexpectedly carried {} fds; dropping", fds.len());
            }
            Ok(DrmNode {
                major: drm_node.major,
                minor: drm_node.minor,
            })
        }
        EventMsg::InitNack { rejection } => Err(Error::RendererSpawnFailed(format!(
            "renderer rejected Init: {} (protocol {} supported {}; spawn {} supported {})",
            rejection.reason,
            rejection.received_protocol_version,
            rejection.supported_protocol_version,
            rejection.received_spawn_version,
            rejection.supported_spawn_version,
        ))),
        other => Err(Error::RendererSpawnFailed(format!(
            "host emitted {other:?} before Ready; aborting spawn"
        ))),
    }
}
