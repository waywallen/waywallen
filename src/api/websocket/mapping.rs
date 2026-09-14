use super::*;

pub(super) fn filter_rule_from_pb(
    rule: &pb::WallpaperFilterRule,
) -> Option<crate::catalog::FilterRule> {
    use crate::catalog::query::{FilterPredicate, FilterRule, IntMatch, StringMatch};
    use pb::wallpaper_filter_rule::Payload;

    let string = |filter: &pb::WallpaperStringFilter| {
        Some((
            filter.value.clone(),
            StringMatch::from_code(filter.condition)?,
        ))
    };
    let int = |filter: &pb::WallpaperIntFilter| {
        Some((filter.value, IntMatch::from_code(filter.condition)?))
    };
    let predicate = match pb::WallpaperFilterType::try_from(rule.r#type).ok()? {
        pb::WallpaperFilterType::Name => {
            let Payload::StringFilter(filter) = rule.payload.as_ref()? else {
                return None;
            };
            let (value, condition) = string(filter)?;
            FilterPredicate::Name { value, condition }
        }
        pb::WallpaperFilterType::WpType => {
            let Payload::StringFilter(filter) = rule.payload.as_ref()? else {
                return None;
            };
            let (value, condition) = string(filter)?;
            FilterPredicate::WallpaperType { value, condition }
        }
        pb::WallpaperFilterType::Library => {
            let Payload::StringFilter(filter) = rule.payload.as_ref()? else {
                return None;
            };
            let (value, condition) = string(filter)?;
            FilterPredicate::Library { value, condition }
        }
        pb::WallpaperFilterType::Width => {
            let Payload::IntFilter(filter) = rule.payload.as_ref()? else {
                return None;
            };
            let (value, condition) = int(filter)?;
            FilterPredicate::Width { value, condition }
        }
        pb::WallpaperFilterType::Height => {
            let Payload::IntFilter(filter) = rule.payload.as_ref()? else {
                return None;
            };
            let (value, condition) = int(filter)?;
            FilterPredicate::Height { value, condition }
        }
        pb::WallpaperFilterType::Size => {
            let Payload::IntFilter(filter) = rule.payload.as_ref()? else {
                return None;
            };
            let (value, condition) = int(filter)?;
            FilterPredicate::Size { value, condition }
        }
        pb::WallpaperFilterType::ContentRating => {
            let Payload::StringFilter(filter) = rule.payload.as_ref()? else {
                return None;
            };
            let (value, condition) = string(filter)?;
            FilterPredicate::ContentRating { value, condition }
        }
        pb::WallpaperFilterType::Tag => match rule.payload.as_ref()? {
            Payload::TagFilter(filter) => FilterPredicate::Tags {
                values: filter.values.clone(),
                condition: StringMatch::from_code(filter.condition)?,
            },
            Payload::StringFilter(filter) => {
                let (value, condition) = string(filter)?;
                FilterPredicate::Tags {
                    values: vec![value],
                    condition,
                }
            }
            Payload::IntFilter(_) => return None,
        },
        pb::WallpaperFilterType::Unspecified => return None,
    };
    Some(FilterRule {
        group: rule.group,
        predicate,
    })
}

pub(super) fn filter_logic_from_pb(logic: &pb::FilterLogic) -> crate::catalog::FilterLogic {
    crate::catalog::FilterLogic {
        operator: crate::catalog::query::LogicOperator::from_code(logic.op),
        group_a: logic.group_a,
        group_b: logic.group_b,
    }
}

pub(super) fn sort_rule_from_pb(rule: &pb::WallpaperSortRule) -> Option<crate::catalog::SortRule> {
    use crate::catalog::query::{SortDirection, SortKey, SortRule};
    let key = match pb::WallpaperSortKey::try_from(rule.key).ok()? {
        pb::WallpaperSortKey::Name => SortKey::Name,
        pb::WallpaperSortKey::WpType => SortKey::WallpaperType,
        pb::WallpaperSortKey::Size => SortKey::Size,
        pb::WallpaperSortKey::LastModified => SortKey::LastModified,
        pb::WallpaperSortKey::Unspecified => return None,
    };
    let direction = if pb::SortDirection::try_from(rule.direction) == Ok(pb::SortDirection::Desc) {
        SortDirection::Descending
    } else {
        SortDirection::Ascending
    };
    Some(SortRule { key, direction })
}

pub(super) fn filter_rule_to_pb(rule: &crate::catalog::FilterRule) -> pb::WallpaperFilterRule {
    use crate::catalog::query::{FilterPredicate, IntMatch, StringMatch};
    let string_code = |condition| match condition {
        StringMatch::Is => 1,
        StringMatch::IsNot => 2,
        StringMatch::Contains => 3,
        StringMatch::ContainsNot => 4,
    };
    let int_code = |condition| match condition {
        IntMatch::Equal => 1,
        IntMatch::NotEqual => 2,
        IntMatch::Less => 3,
        IntMatch::LessEqual => 4,
        IntMatch::Greater => 5,
        IntMatch::GreaterEqual => 6,
    };
    let (ty, payload) = match &rule.predicate {
        FilterPredicate::Name { value, condition } => (
            1,
            pb::wallpaper_filter_rule::Payload::StringFilter(pb::WallpaperStringFilter {
                value: value.clone(),
                condition: string_code(*condition),
            }),
        ),
        FilterPredicate::WallpaperType { value, condition } => (
            2,
            pb::wallpaper_filter_rule::Payload::StringFilter(pb::WallpaperStringFilter {
                value: value.clone(),
                condition: string_code(*condition),
            }),
        ),
        FilterPredicate::Library { value, condition } => (
            3,
            pb::wallpaper_filter_rule::Payload::StringFilter(pb::WallpaperStringFilter {
                value: value.clone(),
                condition: string_code(*condition),
            }),
        ),
        FilterPredicate::Width { value, condition } => (
            5,
            pb::wallpaper_filter_rule::Payload::IntFilter(pb::WallpaperIntFilter {
                value: *value,
                condition: int_code(*condition),
            }),
        ),
        FilterPredicate::Height { value, condition } => (
            6,
            pb::wallpaper_filter_rule::Payload::IntFilter(pb::WallpaperIntFilter {
                value: *value,
                condition: int_code(*condition),
            }),
        ),
        FilterPredicate::Size { value, condition } => (
            7,
            pb::wallpaper_filter_rule::Payload::IntFilter(pb::WallpaperIntFilter {
                value: *value,
                condition: int_code(*condition),
            }),
        ),
        FilterPredicate::ContentRating { value, condition } => (
            8,
            pb::wallpaper_filter_rule::Payload::StringFilter(pb::WallpaperStringFilter {
                value: value.clone(),
                condition: string_code(*condition),
            }),
        ),
        FilterPredicate::Tags { values, condition } => (
            9,
            pb::wallpaper_filter_rule::Payload::TagFilter(pb::WallpaperTagFilter {
                values: values.clone(),
                condition: string_code(*condition),
            }),
        ),
    };
    pb::WallpaperFilterRule {
        r#type: ty,
        group: rule.group,
        payload: Some(payload),
    }
}

pub(super) fn filter_logic_to_pb(logic: &crate::catalog::FilterLogic) -> pb::FilterLogic {
    pb::FilterLogic {
        op: match logic.operator {
            crate::catalog::query::LogicOperator::And => 1,
            crate::catalog::query::LogicOperator::Or => 2,
        },
        group_a: logic.group_a,
        group_b: logic.group_b,
    }
}

pub(super) fn sort_rule_to_pb(rule: &crate::catalog::SortRule) -> pb::WallpaperSortRule {
    use crate::catalog::query::{SortDirection, SortKey};
    pb::WallpaperSortRule {
        key: match rule.key {
            SortKey::Name => 1,
            SortKey::WallpaperType => 2,
            SortKey::Size => 3,
            SortKey::LastModified => 4,
        },
        direction: match rule.direction {
            SortDirection::Ascending => 1,
            SortDirection::Descending => 2,
        },
    }
}

pub(super) fn renderer_def_to_pb(
    def: &crate::plugin::renderer_registry::RendererDef,
    plugin_version: &str,
) -> pb::RendererPluginInfo {
    let mut settings: Vec<pb::SettingSchema> = def
        .settings
        .iter()
        .map(|(k, v)| crate::control_proto::setting_def_to_proto(k, v, &def.plugin_id))
        .collect();
    // Stable order so UIs can rely on deterministic layout: by manifest
    // `order` then key name.
    settings.sort_by(|a, b| a.order.cmp(&b.order).then(a.key.cmp(&b.key)));
    pb::RendererPluginInfo {
        name: def.name.clone(),
        bin: def.bin.to_string_lossy().into_owned(),
        types: def.types.iter().map(|t| t.to_string()).collect(),
        priority: def.priority,
        // Renderers no longer carry their own version; they inherit the
        // owning plugin's. Compatibility is `spawn_version` + bridge.
        version: plugin_version.to_string(),
        settings,
        plugin_id: def.plugin_id.clone(),
    }
}

pub(super) fn plugin_update_state_to_pb(state: crate::plugin::update::PluginUpdateState) -> i32 {
    match state {
        crate::plugin::update::PluginUpdateState::Unknown => 1,
        crate::plugin::update::PluginUpdateState::NoUrl => 2,
        crate::plugin::update::PluginUpdateState::Checking => 3,
        crate::plugin::update::PluginUpdateState::UpToDate => 4,
        crate::plugin::update::PluginUpdateState::Available => 5,
        crate::plugin::update::PluginUpdateState::Failed => 6,
        crate::plugin::update::PluginUpdateState::Unsupported => 7,
    }
}

pub(super) fn plugin_update_info_to_pb(
    info: crate::plugin::update::PluginUpdateInfo,
) -> pb::PluginUpdateInfo {
    pb::PluginUpdateInfo {
        plugin_id: info.plugin_id,
        state: plugin_update_state_to_pb(info.state),
        latest_version: info.latest_version,
        zip_url: info.zip_url,
        sha256: info.sha256,
        error: info.error,
        checked_at_ms: info.checked_at_ms,
    }
}

pub(super) fn gpu_info_to_pb(g: &crate::system::GpuInfo) -> pb::GpuInfo {
    pb::GpuInfo {
        render_node: g
            .render_node
            .as_ref()
            .and_then(|p| p.to_str())
            .map(str::to_string)
            .unwrap_or_default(),
        primary_node: g
            .primary_node
            .as_ref()
            .and_then(|p| p.to_str())
            .map(str::to_string)
            .unwrap_or_default(),
        render_major: g.render_major,
        render_minor: g.render_minor,
        primary_major: g.primary_major,
        primary_minor: g.primary_minor,
        pci_bdf: g.pci_bdf.clone().unwrap_or_default(),
        vendor_id: g.vendor_id as u32,
        device_id: g.device_id as u32,
        driver: g.driver.clone(),
        description: g.description.clone(),
        name: g.name.clone(),
    }
}

pub(super) fn display_snapshot_to_pb(
    s: DisplaySnapshot,
    settings: &SettingsStore,
) -> pb::DisplayInfo {
    // Router snapshots carry effective layout; settings are consulted only
    // for persisted display override and alias fields.
    let layout_key: &str = s
        .instance_id
        .as_deref()
        .filter(|iid| settings.display_prefs(iid).is_some())
        .unwrap_or(s.name.as_str());
    let override_prefs = settings.display_prefs(layout_key).unwrap_or_default();
    pb::DisplayInfo {
        display_id: s.id,
        name: s.name,
        width: s.width,
        height: s.height,
        refresh_mhz: s.refresh_mhz,
        links: s
            .links
            .into_iter()
            .map(|l| pb::DisplayLinkInfo {
                renderer_id: l.renderer_id,
                z_order: l.z_order,
                active: l.active,
            })
            .collect(),
        effective_layout: Some(layout_prefs_to_pb_resolved(&s.effective_layout)),
        layout_override: Some(layout_override_to_pb(&override_prefs)),
        drm_render_major: s.drm_render_major,
        drm_render_minor: s.drm_render_minor,
        alias: override_prefs.alias.clone().unwrap_or_default(),
        display_layout: Some(layout_prefs_to_pb_resolved(&s.display_layout)),
        effective_layout_source: layout_source_to_pb(s.effective_layout_source) as i32,
        conditions: s
            .conditions
            .into_iter()
            .map(runtime_condition_to_pb)
            .collect(),
        instance_id: s.instance_id.unwrap_or_default(),
        settings_key: s.settings_key,
        canvas_id: s.canvas_id.unwrap_or_default(),
        canvas_rect: s.canvas_rect.map(canvas_rect_to_pb),
        canvas_overlap_count: s.canvas_overlap_count,
        selectable_target: s.selectable_target,
    }
}

fn canvas_rect_to_pb(rect: crate::wallframe::display::placement::CanvasRect) -> pb::CanvasRect {
    pb::CanvasRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn canvas_layout_override_to_pb(layout: crate::settings::CanvasLayoutPrefs) -> pb::LayoutOverride {
    pb::LayoutOverride {
        fillmode_set: layout.fillmode.is_some(),
        fillmode: layout
            .fillmode
            .map(fillmode_to_pb)
            .unwrap_or(pb::FillMode::Unspecified) as i32,
        align_set: false,
        align: pb::Align::Unspecified as i32,
        rotation_set: layout.rotation.is_some(),
        rotation: layout
            .rotation
            .map(rotation_to_pb)
            .unwrap_or(pb::Rotation::Unspecified) as i32,
        location_set: layout.location.is_some(),
        location_x: layout.location.map_or(50, |location| u32::from(location.x)),
        location_y: layout.location.map_or(50, |location| u32::from(location.y)),
    }
}

pub(super) fn canvas_snapshot_to_pb(
    snapshot: crate::wallframe::routing::CanvasSnapshot,
) -> pb::CanvasInfo {
    pb::CanvasInfo {
        canvas_id: snapshot.id,
        name: snapshot.name,
        members: snapshot
            .members
            .into_iter()
            .map(|member| pb::CanvasMemberInfo {
                settings_key: member.settings_key,
                rect: Some(canvas_rect_to_pb(member.rect)),
                display_ids: member.display_ids,
                minimum_scale_to: member.minimum_scale_to.map(|size| pb::CanvasSize {
                    width: size.width,
                    height: size.height,
                }),
                aspect_locked: member.aspect_locked,
            })
            .collect(),
        extent: snapshot.extent.map(canvas_rect_to_pb),
        layout_override: snapshot.layout_override.map(canvas_layout_override_to_pb),
        effective_layout: Some(layout_prefs_to_pb_resolved(&snapshot.effective_layout)),
        wallpaper_id: snapshot.wallpaper_id.unwrap_or_default(),
        revision: snapshot.revision,
    }
}

pub(super) fn canvases_replace_event(
    snapshot: crate::wallframe::routing::CanvasCollectionSnapshot,
) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::CanvasSnapshot(pb::CanvasSnapshot {
            canvases: snapshot
                .canvases
                .into_iter()
                .map(canvas_snapshot_to_pb)
                .collect(),
            revision: snapshot.revision,
        })),
    }
}

pub(super) fn runtime_condition_to_pb(condition: RuntimeCondition) -> pb::RuntimeCondition {
    let kind = match condition.kind {
        RuntimeConditionKind::Loading => pb::RuntimeConditionKind::RuntimeConditionLoading,
        RuntimeConditionKind::Waiting => pb::RuntimeConditionKind::RuntimeConditionWaiting,
        RuntimeConditionKind::Hang => pb::RuntimeConditionKind::RuntimeConditionHang,
    };
    let origin = match condition.origin {
        RuntimeConditionOrigin::Renderer => pb::RuntimeConditionOrigin::Renderer,
        RuntimeConditionOrigin::Display => pb::RuntimeConditionOrigin::Display,
        RuntimeConditionOrigin::Release => pb::RuntimeConditionOrigin::Release,
    };
    pb::RuntimeCondition {
        kind: kind as i32,
        origin: origin as i32,
        reason: condition.reason,
        related_renderer_id: condition.related_renderer_id.unwrap_or_default(),
        related_display_id: condition.related_display_id.unwrap_or_default(),
    }
}

pub(super) fn layout_source_to_pb(source: LayoutSource) -> pb::LayoutSource {
    match source {
        LayoutSource::Global => pb::LayoutSource::Global,
        LayoutSource::Display => pb::LayoutSource::Display,
        LayoutSource::Wallpaper => pb::LayoutSource::Wallpaper,
        LayoutSource::Canvas => pb::LayoutSource::Canvas,
    }
}

pub(super) fn layout_prefs_to_pb_resolved(r: &crate::settings::ResolvedLayout) -> pb::LayoutPrefs {
    pb::LayoutPrefs {
        fillmode: fillmode_to_pb(r.fillmode) as i32,
        align: align_to_pb(r.location.to_align()) as i32,
        rotation: rotation_to_pb(r.rotation) as i32,
        location_x: u32::from(r.location.x.min(100)),
        location_y: u32::from(r.location.y.min(100)),
        location_set: true,
    }
}

pub(super) fn layout_override_to_pb(p: &crate::settings::DisplayPrefs) -> pb::LayoutOverride {
    let location = p.location.or_else(|| {
        p.align
            .map(crate::wallframe::display::layout::Location::from_align)
    });
    pb::LayoutOverride {
        fillmode_set: p.fillmode.is_some(),
        fillmode: p
            .fillmode
            .map(fillmode_to_pb)
            .unwrap_or(pb::FillMode::Unspecified) as i32,
        align_set: p.align.is_some(),
        align: p.align.map(align_to_pb).unwrap_or(pb::Align::Unspecified) as i32,
        rotation_set: p.rotation.is_some(),
        rotation: p
            .rotation
            .map(rotation_to_pb)
            .unwrap_or(pb::Rotation::Unspecified) as i32,
        location_set: location.is_some(),
        location_x: location.map(|v| u32::from(v.x.min(100))).unwrap_or(0),
        location_y: location.map(|v| u32::from(v.y.min(100))).unwrap_or(0),
    }
}

pub(super) fn fillmode_to_pb(fm: crate::wallframe::display::layout::FillMode) -> pb::FillMode {
    use crate::wallframe::display::layout::FillMode as F;
    match fm {
        F::Stretched => pb::FillMode::Stretched,
        F::PreserveAspectFit => pb::FillMode::PreserveAspectFit,
        F::PreserveAspectCrop => pb::FillMode::PreserveAspectCrop,
        F::Centered => pb::FillMode::Centered,
    }
}

pub(super) fn fillmode_from_pb(v: i32) -> Option<crate::wallframe::display::layout::FillMode> {
    use crate::wallframe::display::layout::FillMode as F;
    match pb::FillMode::try_from(v).ok()? {
        pb::FillMode::Unspecified => None,
        pb::FillMode::Stretched => Some(F::Stretched),
        pb::FillMode::PreserveAspectFit => Some(F::PreserveAspectFit),
        pb::FillMode::PreserveAspectCrop => Some(F::PreserveAspectCrop),
        pb::FillMode::Centered => Some(F::Centered),
    }
}

pub(super) fn rotation_to_pb(r: crate::wallframe::display::layout::Rotation) -> pb::Rotation {
    use crate::wallframe::display::layout::Rotation as R;
    match r {
        R::Normal => pb::Rotation::Normal,
        R::Cw90 => pb::Rotation::Cw90,
        R::Cw180 => pb::Rotation::Cw180,
        R::Cw270 => pb::Rotation::Cw270,
    }
}

pub(super) fn rotation_from_pb(v: i32) -> Option<crate::wallframe::display::layout::Rotation> {
    use crate::wallframe::display::layout::Rotation as R;
    match pb::Rotation::try_from(v).ok()? {
        pb::Rotation::Unspecified => None,
        pb::Rotation::Normal => Some(R::Normal),
        pb::Rotation::Cw90 => Some(R::Cw90),
        pb::Rotation::Cw180 => Some(R::Cw180),
        pb::Rotation::Cw270 => Some(R::Cw270),
    }
}

pub(super) fn align_to_pb(a: crate::wallframe::display::layout::Align) -> pb::Align {
    use crate::wallframe::display::layout::Align as A;
    match a {
        A::TopLeft => pb::Align::TopLeft,
        A::Top => pb::Align::Top,
        A::TopRight => pb::Align::TopRight,
        A::Left => pb::Align::Left,
        A::Center => pb::Align::Center,
        A::Right => pb::Align::Right,
        A::BottomLeft => pb::Align::BottomLeft,
        A::Bottom => pb::Align::Bottom,
        A::BottomRight => pb::Align::BottomRight,
    }
}

pub(super) fn align_from_pb(v: i32) -> Option<crate::wallframe::display::layout::Align> {
    use crate::wallframe::display::layout::Align as A;
    match pb::Align::try_from(v).ok()? {
        pb::Align::Unspecified => None,
        pb::Align::TopLeft => Some(A::TopLeft),
        pb::Align::Top => Some(A::Top),
        pb::Align::TopRight => Some(A::TopRight),
        pb::Align::Left => Some(A::Left),
        pb::Align::Center => Some(A::Center),
        pb::Align::Right => Some(A::Right),
        pb::Align::BottomLeft => Some(A::BottomLeft),
        pb::Align::Bottom => Some(A::Bottom),
        pb::Align::BottomRight => Some(A::BottomRight),
    }
}

pub(super) fn location_from_pb(x: u32, y: u32) -> crate::wallframe::display::layout::Location {
    crate::wallframe::display::layout::Location::new(x.min(100) as u8, y.min(100) as u8)
}

pub(super) fn resolved_layout_from_pb(p: &pb::LayoutPrefs) -> crate::settings::ResolvedLayout {
    crate::settings::ResolvedLayout {
        fillmode: fillmode_from_pb(p.fillmode).unwrap_or_default(),
        location: if p.location_set {
            location_from_pb(p.location_x, p.location_y)
        } else {
            align_from_pb(p.align)
                .map(crate::wallframe::display::layout::Location::from_align)
                .unwrap_or_default()
        },
        rotation: rotation_from_pb(p.rotation).unwrap_or_default(),
    }
}

pub(super) fn auto_action_to_pb(v: crate::settings::AutoAction) -> pb::AutoAction {
    use crate::settings::AutoAction as A;
    match v {
        A::None => pb::AutoAction::None,
        A::Mute => pb::AutoAction::Mute,
        A::Pause => pb::AutoAction::Pause,
        A::Stop => pb::AutoAction::Stop,
    }
}

pub(super) fn auto_action_from_pb(v: i32) -> crate::settings::AutoAction {
    use crate::settings::AutoAction as A;
    match pb::AutoAction::try_from(v).unwrap_or(pb::AutoAction::None) {
        pb::AutoAction::None => A::None,
        pb::AutoAction::Mute => A::Mute,
        pb::AutoAction::Pause => A::Pause,
        pb::AutoAction::Stop => A::Stop,
    }
}

pub(super) fn auto_replay_to_pb(p: &crate::settings::AutoReplayPolicy) -> pb::AutoReplayPolicy {
    pb::AutoReplayPolicy {
        any_window: auto_action_to_pb(p.any_window) as i32,
        focused: auto_action_to_pb(p.focused) as i32,
        maximized: auto_action_to_pb(p.maximized) as i32,
        fullscreen: auto_action_to_pb(p.fullscreen) as i32,
        session_locked: auto_action_to_pb(p.session_locked) as i32,
        session_inactive: auto_action_to_pb(p.session_inactive) as i32,
        resume_delay_ms: Some(p.effective_resume_delay_ms()),
    }
}

pub(super) fn auto_replay_from_pb(p: &pb::AutoReplayPolicy) -> crate::settings::AutoReplayPolicy {
    crate::settings::AutoReplayPolicy {
        any_window: auto_action_from_pb(p.any_window),
        focused: auto_action_from_pb(p.focused),
        maximized: auto_action_from_pb(p.maximized),
        fullscreen: auto_action_from_pb(p.fullscreen),
        session_locked: auto_action_from_pb(p.session_locked),
        session_inactive: auto_action_from_pb(p.session_inactive),
        resume_delay_ms: p
            .resume_delay_ms
            .unwrap_or(crate::settings::DEFAULT_AUTO_REPLAY_RESUME_DELAY_MS)
            .min(crate::settings::MAX_AUTO_REPLAY_RESUME_DELAY_MS),
    }
}

pub(super) fn pause_effect_kind_to_pb(
    kind: crate::settings::PauseEffectKind,
) -> pb::PauseEffectKind {
    match kind {
        crate::settings::PauseEffectKind::None => pb::PauseEffectKind::None,
        crate::settings::PauseEffectKind::Blur => pb::PauseEffectKind::Blur,
    }
}

pub(super) fn pause_effect_kind_from_pb(value: i32) -> crate::settings::PauseEffectKind {
    match pb::PauseEffectKind::try_from(value).unwrap_or_default() {
        pb::PauseEffectKind::None => crate::settings::PauseEffectKind::None,
        pb::PauseEffectKind::Blur => crate::settings::PauseEffectKind::Blur,
    }
}

pub(super) fn pause_effect_to_pb(
    config: crate::settings::PauseEffectConfig,
) -> pb::PauseEffectConfig {
    let config = config.effective();
    pb::PauseEffectConfig {
        kind: pause_effect_kind_to_pb(config.kind) as i32,
        blur: Some(pb::BlurEffectConfig {
            radius: config.blur.radius,
        }),
    }
}

pub(super) fn pause_effect_from_pb(
    p: &pb::PauseEffectConfig,
) -> crate::settings::PauseEffectConfig {
    let radius = p
        .blur
        .as_ref()
        .map(|blur| blur.radius)
        .unwrap_or(crate::settings::DEFAULT_BLUR_EFFECT_RADIUS);
    crate::settings::PauseEffectConfig {
        kind: pause_effect_kind_from_pb(p.kind),
        blur: crate::settings::BlurEffectConfig {
            radius: radius.clamp(
                crate::settings::MIN_BLUR_EFFECT_RADIUS,
                crate::settings::MAX_BLUR_EFFECT_RADIUS,
            ),
        },
    }
}

pub(super) fn transition_kind_to_pb(kind: crate::settings::TransitionKind) -> pb::TransitionKind {
    match kind {
        crate::settings::TransitionKind::None => pb::TransitionKind::None,
        crate::settings::TransitionKind::Fade => pb::TransitionKind::Fade,
        crate::settings::TransitionKind::Wipe => pb::TransitionKind::Wipe,
        crate::settings::TransitionKind::Grow => pb::TransitionKind::Grow,
    }
}

pub(super) fn transition_kind_from_pb(value: i32) -> crate::settings::TransitionKind {
    match pb::TransitionKind::try_from(value).unwrap_or_default() {
        pb::TransitionKind::None => crate::settings::TransitionKind::None,
        pb::TransitionKind::Fade => crate::settings::TransitionKind::Fade,
        pb::TransitionKind::Wipe => crate::settings::TransitionKind::Wipe,
        pb::TransitionKind::Grow => crate::settings::TransitionKind::Grow,
    }
}

pub(super) fn transition_to_pb(config: crate::settings::TransitionConfig) -> pb::TransitionConfig {
    let config = config.effective();
    pb::TransitionConfig {
        kind: transition_kind_to_pb(config.kind) as i32,
        duration_ms: config.duration_ms,
        angle: config.angle,
        origin_x: config.origin.x,
        origin_y: config.origin.y,
    }
}

pub(super) fn transition_from_pb(p: &pb::TransitionConfig) -> crate::settings::TransitionConfig {
    crate::settings::TransitionConfig {
        kind: transition_kind_from_pb(p.kind),
        duration_ms: p.duration_ms,
        angle: p.angle,
        origin: crate::settings::TransitionOrigin {
            x: p.origin_x,
            y: p.origin_y,
        },
    }
    .effective()
}

pub(super) fn global_to_pb(g: &crate::settings::GlobalSettings) -> pb::GlobalSettings {
    let (wallpaper_filters, wallpaper_filter_logics) = g.wallpaper_filter.to_catalog();
    let wallpaper_filters = wallpaper_filters.iter().map(filter_rule_to_pb).collect();
    let wallpaper_filter_logics = wallpaper_filter_logics
        .iter()
        .map(filter_logic_to_pb)
        .collect();
    let wallpaper_sorts = WallpaperSortRuleState::vec_to_catalog(&g.wallpaper_sorts)
        .iter()
        .map(sort_rule_to_pb)
        .collect();
    pb::GlobalSettings {
        wallpaper_filters,
        wallpaper_filter_logics,
        wallpaper_sorts,
        layout_defaults: Some(pb::LayoutPrefs {
            fillmode: fillmode_to_pb(g.layout.fillmode) as i32,
            align: align_to_pb(
                g.layout
                    .location
                    .unwrap_or_else(|| {
                        crate::wallframe::display::layout::Location::from_align(g.layout.align)
                    })
                    .to_align(),
            ) as i32,
            rotation: rotation_to_pb(g.layout.rotation) as i32,
            location_x: u32::from(
                g.layout
                    .location
                    .unwrap_or_else(|| {
                        crate::wallframe::display::layout::Location::from_align(g.layout.align)
                    })
                    .x
                    .min(100),
            ),
            location_y: u32::from(
                g.layout
                    .location
                    .unwrap_or_else(|| {
                        crate::wallframe::display::layout::Location::from_align(g.layout.align)
                    })
                    .y
                    .min(100),
            ),
            location_set: true,
        }),
        auto_replay: Some(auto_replay_to_pb(&g.effective_auto_replay())),
        pause_effect: Some(pause_effect_to_pb(g.pause_effect)),
        transition: Some(transition_to_pb(g.transition)),
        queue_mode: g.queue_mode.clone(),
        rotation_secs: g.rotation_secs,
        audio_fade_ms: g.effective_audio_fade_ms(),
        mute_when_other_audio: Some(g.mute_when_other_audio),
        audio_capture_enabled: g.audio_capture_enabled,
        pointer_forwarding_enabled: g.pointer_forwarding_enabled,
        wallpaper_skip_types: g.wallpaper_skip_types.clone(),
        wallpaper_filter_tags: g.wallpaper_filter_tags.clone(),
        wallpaper_skip_content_ratings: g.wallpaper_skip_content_ratings.clone(),
        disable_plugin_update_notifications: !g.plugin_update_notifications,
        duplicate_renderers_for_same_wallpaper: g.duplicate_renderers_for_same_wallpaper,
        renderer: Some(pb::GlobalRendererSettings {
            enable_audio: Some(g.renderer.enable_audio),
            volume: Some(g.renderer.effective_volume()),
        }),
        hide_tray_icon: g.hide_tray_icon,
        debug_logging_enabled: g.debug_logging_enabled,
    }
}

pub(super) fn displays_replace_event(
    snap: Vec<DisplaySnapshot>,
    settings: &SettingsStore,
) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::DisplaySnapshot(pb::DisplaySnapshot {
            displays: snap
                .into_iter()
                .map(|s| display_snapshot_to_pb(s, settings))
                .collect(),
        })),
    }
}

pub(super) fn renderer_snapshot_to_pb(
    s: RendererSnapshot,
    settings: &SettingsStore,
) -> pb::RendererInstance {
    fn exit_to_pb(exit: &crate::wallframe::routing::RendererExitSnapshot) -> pb::RendererExit {
        pb::RendererExit {
            code: exit.code.unwrap_or_default(),
            signal: exit.signal.unwrap_or_default(),
            has_code: exit.code.is_some(),
            has_signal: exit.signal.is_some(),
            reason: exit.reason.clone(),
        }
    }

    let fps: u32 = settings
        .plugin(&s.name)
        .and_then(|kv| kv.get("fps").and_then(|v| v.parse().ok()))
        .unwrap_or(0);
    let state = match &s.state {
        crate::wallframe::routing::RendererLifecycleState::Starting { generation } => {
            pb::renderer_state::Kind::Starting(pb::RendererStartingState {
                generation: *generation,
            })
        }
        crate::wallframe::routing::RendererLifecycleState::Running {
            generation,
            activity,
        } => pb::renderer_state::Kind::Running(pb::RendererRunningState {
            generation: *generation,
            activity: match activity {
                crate::wallframe::routing::RendererActivity::Playing => {
                    pb::RendererActivity::Playing as i32
                }
                crate::wallframe::routing::RendererActivity::Paused => {
                    pb::RendererActivity::Paused as i32
                }
                crate::wallframe::routing::RendererActivity::Muted => {
                    pb::RendererActivity::Muted as i32
                }
            },
        }),
        crate::wallframe::routing::RendererLifecycleState::Stopping { generation, keep } => {
            pb::renderer_state::Kind::Stopping(pb::RendererStoppingState {
                generation: *generation,
                keep: *keep,
            })
        }
        crate::wallframe::routing::RendererLifecycleState::Stopped { keep, last_exit } => {
            pb::renderer_state::Kind::Stopped(pb::RendererStoppedState {
                keep: *keep,
                last_exit: last_exit.as_ref().map(exit_to_pb),
            })
        }
        crate::wallframe::routing::RendererLifecycleState::Killed { keep, last_exit } => {
            pb::renderer_state::Kind::Killed(pb::RendererKilledState {
                keep: *keep,
                last_exit: Some(exit_to_pb(last_exit)),
            })
        }
        crate::wallframe::routing::RendererLifecycleState::Failed { failure } => {
            pb::renderer_state::Kind::Failed(pb::RendererFailedState {
                failure: Some(exit_to_pb(failure)),
            })
        }
    };
    pb::RendererInstance {
        renderer_id: s.id,
        fps,
        name: s.name,
        pid: s.pid,
        drm_render_major: s.drm_render_major,
        drm_render_minor: s.drm_render_minor,
        texture_width: s.texture_width,
        texture_height: s.texture_height,
        runtime_tags: s
            .runtime_tags
            .into_iter()
            .map(|tag| pb::RendererRuntimeTag {
                key: tag.key,
                value: tag.value,
            })
            .collect(),
        conditions: s
            .conditions
            .into_iter()
            .map(runtime_condition_to_pb)
            .collect(),
        state: Some(pb::RendererState { kind: Some(state) }),
    }
}

pub(super) fn renderers_replace_event(
    snap: Vec<RendererSnapshot>,
    settings: &SettingsStore,
) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::RendererSnapshot(pb::RendererSnapshot {
            renderers: snap
                .into_iter()
                .map(|s| renderer_snapshot_to_pb(s, settings))
                .collect(),
        })),
    }
}

pub(super) fn library_instance_to_pb(s: LibrarySnapshot) -> pb::LibraryInstance {
    pb::LibraryInstance {
        id: s.id,
        path: s.path,
        plugin_name: s.plugin_name,
    }
}

pub(super) fn libraries_replace_event(snap: Vec<LibrarySnapshot>) -> pb::Event {
    pb::Event {
        payload: Some(pb::event::Payload::LibrarySnapshot(pb::LibrarySnapshot {
            libraries: snap.into_iter().map(library_instance_to_pb).collect(),
        })),
    }
}

pub(super) fn router_event_to_pb(e: RouterEvent, settings: &SettingsStore) -> pb::Event {
    match e {
        RouterEvent::DisplayUpsert(s) => pb::Event {
            payload: Some(pb::event::Payload::DisplayChanged(pb::DisplayChanged {
                display: Some(display_snapshot_to_pb(s, settings)),
            })),
        },
        RouterEvent::DisplayRemoved(id) => pb::Event {
            payload: Some(pb::event::Payload::DisplayRemoved(pb::DisplayRemoved {
                display_id: id,
            })),
        },
        RouterEvent::DisplaysReplace(list) => displays_replace_event(list, settings),
        RouterEvent::CanvasesReplace(snapshot) => canvases_replace_event(snapshot),
        RouterEvent::RendererUpsert(s) => pb::Event {
            payload: Some(pb::event::Payload::RendererChanged(pb::RendererChanged {
                renderer: Some(renderer_snapshot_to_pb(s, settings)),
            })),
        },
        RouterEvent::RendererRemoved(id) => pb::Event {
            payload: Some(pb::event::Payload::RendererRemoved(pb::RendererRemoved {
                renderer_id: id,
            })),
        },
        RouterEvent::RenderersReplace(list) => renderers_replace_event(list, settings),
        RouterEvent::LibraryUpsert(s) => pb::Event {
            payload: Some(pb::event::Payload::LibraryChanged(pb::LibraryChanged {
                library: Some(library_instance_to_pb(s)),
            })),
        },
        RouterEvent::LibraryRemoved(id) => pb::Event {
            payload: Some(pb::event::Payload::LibraryRemoved(pb::LibraryRemoved {
                id,
            })),
        },
        RouterEvent::LibrariesReplace(list) => libraries_replace_event(list),
    }
}

pub(super) fn wallpaper_presentations_event(
    presentations: &[crate::wallframe::routing::WallpaperPresentationInfo],
) -> pb::Event {
    use crate::wallframe::routing::{
        WallpaperPresentationState as State, WallpaperPresentationTarget as Target,
    };

    pb::Event {
        payload: Some(pb::event::Payload::WallpaperPresentationSnapshot(
            pb::WallpaperPresentationSnapshot {
                presentations: presentations
                    .iter()
                    .map(|presentation| pb::WallpaperPresentationInfo {
                        wallpaper_id: presentation.wallpaper_id.clone(),
                        targets: presentation
                            .targets
                            .iter()
                            .map(|target| pb::WallpaperPresentationTarget {
                                target: Some(match target {
                                    Target::Display(display_id) => {
                                        pb::wallpaper_presentation_target::Target::DisplayId(
                                            *display_id,
                                        )
                                    }
                                    Target::Canvas(canvas_id) => {
                                        pb::wallpaper_presentation_target::Target::CanvasId(
                                            canvas_id.clone(),
                                        )
                                    }
                                }),
                            })
                            .collect(),
                        state: match presentation.state {
                            State::Starting => pb::WallpaperPresentationState::Starting as i32,
                            State::Playing => pb::WallpaperPresentationState::Playing as i32,
                            State::Paused => pb::WallpaperPresentationState::Paused as i32,
                            State::Stopped => pb::WallpaperPresentationState::Stopped as i32,
                        },
                    })
                    .collect(),
            },
        )),
    }
}

pub(super) fn router_event_affects_wallpaper_presentations(event: &RouterEvent) -> bool {
    !matches!(
        event,
        RouterEvent::LibraryUpsert(_)
            | RouterEvent::LibraryRemoved(_)
            | RouterEvent::LibrariesReplace(_)
    )
}

/// Snapshot daemon-side runtime state into a `StatusSync` server event.
/// Pushed on WS connect, status changes, and task lifecycle events.
pub(super) async fn status_sync_event(state: &Arc<DaemonContext>) -> pb::Event {
    use std::sync::atomic::Ordering;
    let scan_in_progress = state.scan_in_progress.load(Ordering::SeqCst);
    let active_task_count = state
        .tasks
        .list()
        .into_iter()
        .filter(|r| matches!(r.state, tasks::TaskState::Running))
        .count() as u32;
    let phase = if state.events.is_daemon_ready() {
        pb::DaemonPhase::Ready
    } else {
        pb::DaemonPhase::Starting
    };
    let display_backend = state.display_backend_status.read().unwrap().clone();
    let lifecycle = state.router.manual_lifecycle_state().await;
    pb::Event {
        payload: Some(pb::event::Payload::StatusSync(pb::StatusSync {
            scan_in_progress,
            active_task_count,
            phase: phase as i32,
            display_backend: Some(display_backend_status_to_pb(display_backend)),
            global_paused: lifecycle.paused,
            global_muted: lifecycle.muted,
            global_stopped: lifecycle.stopped,
        })),
    }
}

pub(super) fn display_backend_status_to_pb(
    s: crate::wallframe::display::spawner::DisplayBackendStatus,
) -> pb::DisplayBackendStatus {
    pb::DisplayBackendStatus {
        name: s.name,
        state: s.state,
        desktop: s.desktop,
        binary: s.binary,
        reason: s.reason,
        flatpak_id: s.flatpak_id,
    }
}

pub(super) fn playlist_display_status_to_pb(
    d: crate::playback::playlist::DisplayStatus,
) -> pb::PlaylistDisplayStatus {
    pb::PlaylistDisplayStatus {
        display_id: d.display_id,
        active_id: d.active_id,
        mode: queue_mode_to_pb_playlist(d.mode),
        interval_secs: d.interval_secs,
        current_id: d.current_id.unwrap_or_default(),
        position: d.position,
        count: d.count,
        remaining_secs: d.remaining_secs,
    }
}

pub(super) async fn playlist_changed_event(state: &Arc<DaemonContext>) -> pb::Event {
    let auto_attach_id = state.settings.global().auto_attach_playlist_id.unwrap_or(0);
    let displays = state
        .playlists
        .status()
        .await
        .into_iter()
        .map(playlist_display_status_to_pb)
        .collect();
    pb::Event {
        payload: Some(pb::event::Payload::PlaylistChanged(pb::PlaylistChanged {
            displays,
            auto_attach_id,
        })),
    }
}

/// Translate the subset of `GlobalEvent` variants the UI cares about
/// into wire events. Returns `None` for daemon-internal events.
pub(super) fn global_event_to_pb(e: &GlobalEvent, state: &Arc<DaemonContext>) -> Option<pb::Event> {
    match e {
        GlobalEvent::SyncFinished { count } => Some(pb::Event {
            payload: Some(pb::event::Payload::WallpaperSyncFinished(
                pb::WallpaperSyncFinished {
                    count: *count as u32,
                    error: String::new(),
                },
            )),
        }),
        GlobalEvent::SyncFailed(msg) => Some(pb::Event {
            payload: Some(pb::event::Payload::WallpaperSyncFinished(
                pb::WallpaperSyncFinished {
                    count: 0,
                    error: msg.clone(),
                },
            )),
        }),
        GlobalEvent::LibrariesAdded { paths } => Some(pb::Event {
            payload: Some(pb::event::Payload::LibrariesAdded(pb::LibrariesAdded {
                paths: paths.clone(),
            })),
        }),
        GlobalEvent::DisplayConnectionFailed {
            client_name,
            client_protocol_version,
            error_code,
            reason,
        } => Some(pb::Event {
            payload: Some(pb::event::Payload::DisplayConnectionFailed(
                pb::DisplayConnectionFailed {
                    client_name: client_name.clone(),
                    client_protocol_version: *client_protocol_version,
                    error_code: *error_code,
                    reason: reason.clone(),
                },
            )),
        }),
        GlobalEvent::RemoteDownloadProgress {
            source_id,
            id,
            state,
            error,
        } => Some(pb::Event {
            payload: Some(pb::event::Payload::RemoteDownloadProgress(
                pb::RemoteDownloadProgress {
                    source_id: source_id.clone(),
                    id: id.clone(),
                    state: match state {
                        crate::events::RemoteDownloadState::Pending => {
                            pb::RemoteDownloadState::Pending as i32
                        }
                        crate::events::RemoteDownloadState::Downloading => {
                            pb::RemoteDownloadState::Downloading as i32
                        }
                        crate::events::RemoteDownloadState::Done => {
                            pb::RemoteDownloadState::Done as i32
                        }
                        crate::events::RemoteDownloadState::Timeout => {
                            pb::RemoteDownloadState::Timeout as i32
                        }
                        crate::events::RemoteDownloadState::Error => {
                            pb::RemoteDownloadState::Error as i32
                        }
                    },
                    error: error.clone(),
                },
            )),
        }),
        GlobalEvent::QrLoginProgress {
            session_id,
            plugin_id,
            action_id,
            state,
            qr_image,
            display_value,
            error,
            title,
            instruction,
        } => Some(pb::Event {
            payload: Some(pb::event::Payload::QrLoginProgress(pb::QrLoginProgress {
                session_id: session_id.clone(),
                plugin_id: plugin_id.clone(),
                action_id: action_id.clone(),
                state: *state,
                qr_image: qr_image.clone(),
                display_value: display_value.text().to_string(),
                error: error.text().to_string(),
                title: title.text().to_string(),
                instruction: instruction.text().to_string(),
                display_value_text: crate::control_proto::plugin_message_to_proto(display_value),
                error_text: crate::control_proto::plugin_message_to_proto(error),
                title_text: crate::control_proto::plugin_message_to_proto(title),
                instruction_text: crate::control_proto::plugin_message_to_proto(instruction),
            })),
        }),
        GlobalEvent::SettingsChanged => {
            let snap = state.settings.snapshot();
            Some(pb::Event {
                payload: Some(pb::event::Payload::SettingsChanged(pb::SettingsChanged {
                    global: Some(global_to_pb(&snap.global)),
                    plugins: snap
                        .plugins
                        .into_iter()
                        .map(|(k, v)| (k, pb::PluginSettings { values: v }))
                        .collect(),
                })),
            })
        }
        GlobalEvent::PluginUpdateChanged => Some(pb::Event {
            payload: Some(pb::event::Payload::PluginUpdateChanged(pb::Empty {})),
        }),
        GlobalEvent::PluginChanged => Some(pb::Event {
            payload: Some(pb::event::Payload::PluginChanged(pb::Empty {})),
        }),
        GlobalEvent::PluginStateChanged => Some(pb::Event {
            payload: Some(pb::event::Payload::PluginStateChanged(pb::Empty {})),
        }),
        GlobalEvent::TaskProgress(progress) => Some(pb::Event {
            payload: Some(pb::event::Payload::TaskProgress(pb::TaskProgress {
                query_id: progress.query_id.clone(),
                progress: progress.progress,
                progressing: progress.progressing,
                ended: progress.ended,
                error: progress.error,
                message: progress.message.clone(),
            })),
        }),
        GlobalEvent::PluginRestartFailed { plugin_id, error } => Some(pb::Event {
            payload: Some(pb::event::Payload::PluginRestartFailed(
                pb::PluginRestartFailed {
                    plugin_id: plugin_id.clone(),
                    error: error.clone(),
                },
            )),
        }),
        GlobalEvent::SourcesReady
        | GlobalEvent::DisplayReady
        | GlobalEvent::DaemonReady
        | GlobalEvent::RestoreApplied(_)
        | GlobalEvent::RestoreFailed(_)
        | GlobalEvent::StatusChanged
        | GlobalEvent::PlaylistChanged => None,
    }
}

// ---------------------------------------------------------------------------
// Dispatch

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn gpu_info_mapping_keeps_name_and_description() {
        let gpu = crate::system::GpuInfo {
            render_node: Some(PathBuf::from("/dev/dri/renderD128")),
            render_major: 226,
            render_minor: 128,
            vendor_id: 0x1002,
            device_id: 0x7550,
            driver: "amdgpu".to_string(),
            name: "Navi 48".to_string(),
            description: "Navi 48 — amdgpu 0x1002:0x7550".to_string(),
            ..Default::default()
        };

        let mapped = gpu_info_to_pb(&gpu);

        assert_eq!(mapped.name, gpu.name);
        assert_eq!(mapped.description, gpu.description);
        assert_eq!(mapped.render_node, "/dev/dri/renderD128");
    }
}
