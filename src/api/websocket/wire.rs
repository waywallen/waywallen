use super::*;

/// Encode a dispatch result onto the wire. Thin wrapper around
/// `Error::to_response` / `ok_response` from `crate::error`.
pub(super) fn build_response(
    request_id: u64,
    result: Result<pb::response::Payload, Error>,
) -> pb::Response {
    match result {
        Ok(payload) => ok_response(request_id, payload),
        Err(error) => {
            log::error!("request {request_id} failed: {error}");
            error.to_response(request_id)
        }
    }
}

pub(super) fn wrap_response(resp: pb::Response) -> pb::ServerFrame {
    pb::ServerFrame {
        kind: Some(pb::server_frame::Kind::Response(resp)),
    }
}

#[allow(dead_code)]
pub fn wrap_event(evt: pb::Event) -> pb::ServerFrame {
    pb::ServerFrame {
        kind: Some(pb::server_frame::Kind::Event(evt)),
    }
}

pub(super) fn entry_to_pb(
    e: &crate::catalog::entry::WallpaperEntry,
    tags: Vec<String>,
    user_properties_schema: String,
    user_property_overrides: String,
    wallpaper_layout_override: Option<WallpaperLayoutOverride>,
    supports_item_remove: bool,
    supports_item_unsubscribe: bool,
) -> pb::WallpaperEntry {
    // `e` is reconstructed from the DB (the source of truth), so its
    // fields are already the freshest values — no overlay needed.
    let wallpaper_layout_override_set = wallpaper_layout_override.is_some();
    pb::WallpaperEntry {
        id: e.item_id.to_string(),
        name: e.name.clone(),
        wp_type: e.wp_type.clone(),
        resource: e.resource.clone(),
        preview: e.preview.clone().unwrap_or_default(),
        // Per-entry metadata is no longer carried (extras() decouples
        // the renderer launch args); the wire field stays for compat.
        metadata: Default::default(),
        size: e.size.unwrap_or(0),
        width: e.width.unwrap_or(0),
        height: e.height.unwrap_or(0),
        content_rating: e.content_rating.clone().unwrap_or_default(),
        tags,
        user_properties_schema,
        user_property_overrides,
        description: e.description.clone().unwrap_or_default(),
        external_id: e.external_id.clone().unwrap_or_default(),
        wallpaper_layout_override: wallpaper_layout_override
            .map(|layout| layout_prefs_to_pb_resolved(&layout.materialize())),
        wallpaper_layout_override_set,
        supports_item_remove,
        supports_item_unsubscribe,
        web_url: e.web_url.clone().unwrap_or_default(),
        hidden: e.hidden,
        update_available: e.update_available,
    }
}
