use super::*;

pub(super) fn query_error(query: &str, error: impl std::fmt::Display) -> String {
    let error = error.to_string();
    log::error!("{query} failed: {error}");
    error
}

pub(super) async fn dispatch(state: &Arc<DaemonContext>, req: pb::Request) -> pb::Response {
    let rid = req.request_id;
    build_response(rid, dispatch_inner(state, req).await)
}

pub(super) async fn dispatch_inner(
    state: &Arc<DaemonContext>,
    req: pb::Request,
) -> Result<pb::response::Payload, Error> {
    let payload = req
        .payload
        .ok_or(Error::UnexpectedPayload("empty request payload"))?;

    use pb::request::Payload as Req;
    use pb::response::Payload as Res;

    Ok(match payload {
        Req::Health(_) => Res::Health(pb::HealthResponse {
            service: "waywallen".into(),
            state: "healthy".into(),
            os_name: state.system_info.os_name().to_owned(),
        }),

        Req::RendererSpawn(r) => {
            // Low-level RPC: caller hands in a single `metadata` map.
            // Treat it as both CLI extras and Init.settings for manual use.
            let wp_type = if r.wp_type.is_empty() {
                "scene".into()
            } else {
                r.wp_type
            };
            let mut settings = r.metadata.clone();
            if r.fps != 0 {
                settings.insert("fps".to_string(), r.fps.to_string());
            }
            let registry = state.renderer_manager.registry_snapshot();
            let renderer = registry
                .resolve(&wp_type)
                .ok_or_else(|| Error::NoRendererForType(wp_type.clone()))?;
            state
                .settings
                .apply_global_renderer_settings(renderer, &mut settings);
            let renderer_name = renderer.name.clone();
            let spawn_req = renderer_manager::SpawnRequest {
                wp_type,
                extras: r.metadata,
                settings,
                test_pattern: false,
                renderer_name: Some(renderer_name),
                user_property_overrides: Default::default(),
                default_user_properties: Default::default(),
                display_size: None,
            };
            // Wallframe returns typed spawn errors directly.
            let id = state.router.spawn_renderer(spawn_req).await?;
            Res::RendererSpawn(pb::RendererSpawnResponse { renderer_id: id })
        }

        Req::RendererList(_) => {
            let snapshots = state.router.snapshot_renderers().await;
            let ids = snapshots
                .iter()
                .map(|snapshot| snapshot.id.clone())
                .collect();
            let instances = snapshots
                .into_iter()
                .map(|snapshot| renderer_snapshot_to_pb(snapshot, &state.settings))
                .collect();
            Res::RendererList(pb::RendererListResponse {
                renderers: ids,
                instances,
            })
        }

        Req::RendererPlay(r) => {
            if !state
                .router
                .set_renderer_paused(&r.renderer_id, false)
                .await
            {
                return Err(Error::RendererNotFound(r.renderer_id));
            }
            Res::RendererPlay(pb::Empty {})
        }

        Req::RendererPause(r) => {
            if !state.router.set_renderer_paused(&r.renderer_id, true).await {
                return Err(Error::RendererNotFound(r.renderer_id));
            }
            Res::RendererPause(pb::Empty {})
        }

        Req::GlobalPauseToggle(_) => {
            let paused = application::toggle_pause_all(state).await?;
            Res::GlobalPauseToggle(pb::GlobalPauseToggleResponse { paused })
        }

        Req::GlobalPauseSet(r) => {
            let paused = application::set_pause_all(state, r.paused).await?;
            Res::GlobalPauseSet(pb::GlobalPauseSetResponse { paused })
        }

        Req::GlobalMuteSet(r) => {
            let muted = application::set_mute_all(state, r.muted).await?;
            Res::GlobalMuteSet(pb::GlobalMuteSetResponse { muted })
        }

        Req::GlobalStopSet(r) => {
            let stopped = application::set_stop_all(state, r.stopped).await?;
            Res::GlobalStopSet(pb::GlobalStopSetResponse { stopped })
        }

        Req::RendererMouse(r) => {
            // Subscription-gated: skipped silently when the renderer's
            // renderer has not registered the pointer event.
            state
                .renderer_manager
                .send_pointer_motion(
                    &r.renderer_id,
                    crate::wallframe::ipc::proto::PointerMotion {
                        x: r.x as f32,
                        y: r.y as f32,
                        timestamp_us: 0,
                        modifiers: 0,
                    },
                )
                .await?;
            Res::RendererMouse(pb::Empty {})
        }

        Req::RendererFps(r) => {
            if !state
                .router
                .update_renderer_assignment_fps(&r.renderer_id, r.fps)
                .await
            {
                return Err(Error::RendererNotFound(r.renderer_id));
            }
            if state.renderer_manager.get(&r.renderer_id).await.is_none() {
                return Ok(Res::RendererFps(pb::Empty {}));
            }
            state
                .renderer_manager
                .send_setting_changed(&r.renderer_id, Vec::new(), Some(r.fps))
                .await?;
            Res::RendererFps(pb::Empty {})
        }

        Req::RendererKill(r) => {
            state.router.kill_renderer_drop(&r.renderer_id).await?;
            Res::RendererKill(pb::Empty {})
        }

        Req::RendererPluginList(_) => {
            let registry = state.renderer_manager.registry_snapshot();
            // Renderer version = owning plugin's version, by plugin_id.
            let plugins = state.plugins.read().await;
            let plugin_versions: std::collections::HashMap<&str, &str> = plugins
                .iter()
                .map(|p| (p.id.as_str(), p.version.as_str()))
                .collect();
            let renderers = registry
                .all_renderers()
                .iter()
                .map(|def| {
                    renderer_def_to_pb(
                        def,
                        plugin_versions
                            .get(def.plugin_id.as_str())
                            .copied()
                            .unwrap_or(""),
                    )
                })
                .collect();
            // `supported_types` comes from a HashMap; sort so the UI's
            // type chips/menus keep a stable alphabetical order.
            let mut supported_types: Vec<_> =
                registry.supported_types().into_iter().cloned().collect();
            supported_types.sort();
            Res::RendererPluginList(pb::RendererPluginListResponse {
                renderers,
                supported_types,
            })
        }

        Req::PluginList(_) => {
            let registry = state.renderer_manager.registry_snapshot();
            let renderer_defs: Vec<_> = registry.all_renderers().into_iter().cloned().collect();
            let packages = state.plugins.read().await.clone();
            let inactive_system = state.inactive_system.read().await.clone();
            let inactive_user = state.inactive_user.read().await.clone();
            let mut plugins = Vec::new();
            for pkg in packages {
                let renderers = renderer_defs
                    .iter()
                    .filter(|def| def.plugin_id == pkg.id)
                    .map(|def| renderer_def_to_pb(def, &pkg.version))
                    .collect();
                let update_info =
                    crate::plugin::update::snapshot_for_package(&state.plugin_updates, &pkg).await;
                plugins.push(pb::PluginInfo {
                    id: pkg.id.clone(),
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                    has_source: pkg.has_entry,
                    renderers,
                    system: pkg.system,
                    update: pkg.update.clone().unwrap_or_default(),
                    update_info: Some(plugin_update_info_to_pb(update_info)),
                });
            }
            Res::PluginList(pb::PluginListResponse {
                plugins,
                inactive_system,
                inactive_user,
            })
        }

        Req::PluginDelete(r) => {
            let plugin_id = r.plugin_id.clone();
            let plugin_id = tokio::task::spawn_blocking(move || {
                crate::plugin::installer::delete_user_plugin(&plugin_id)
            })
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("plugin delete join: {e}")))??;
            Res::PluginDelete(pb::PluginDeleteResponse {
                plugin_id,
                needs_restart: true,
            })
        }

        Req::PluginInspect(r) => {
            let zip_path = r.zip_path.clone();
            let (info, existing_user) = tokio::task::spawn_blocking(move || {
                let info = crate::plugin::installer::inspect_zip(&zip_path)?;
                let existing = crate::plugin::installer::inspect_user_plugin(&info.id)?;
                Ok::<_, Error>((info, existing))
            })
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("plugin inspect join: {e}")))??;

            let active_existing = {
                let plugins = state.plugins.read().await;
                plugins.iter().find(|p| p.id == info.id).cloned()
            };
            let overwrite = existing_user.is_some();
            let existing_version = existing_user
                .as_ref()
                .map(|p| p.version.clone())
                .or_else(|| active_existing.as_ref().map(|p| p.version.clone()))
                .unwrap_or_default();
            let existing_name = existing_user
                .as_ref()
                .map(|p| p.name.clone())
                .or_else(|| active_existing.as_ref().map(|p| p.name.clone()))
                .unwrap_or_default();
            let existing_system = if existing_user.is_some() {
                false
            } else {
                active_existing.as_ref().is_some_and(|p| p.system)
            };

            Res::PluginInspect(pb::PluginInspectResponse {
                plugin_id: info.id,
                name: info.name,
                version: info.version,
                has_source: info.has_source,
                renderers: info.renderers,
                overwrite,
                existing_version,
                existing_name,
                existing_system,
                update: info.update.unwrap_or_default(),
            })
        }

        Req::TagList(_) => {
            let tags = repo::list_tags(&state.db)
                .await?
                .into_iter()
                .map(|t| t.name)
                .collect();
            Res::TagList(pb::TagListResponse { tags })
        }

        Req::ContentRatingList(_) => {
            let ratings = repo::list_content_ratings(&state.db).await?;
            Res::ContentRatingList(pb::ContentRatingListResponse { ratings })
        }

        Req::WallpaperList(r) => {
            log::info!(
                "WallpaperList: page={} page_size={} wp_type={:?} filters={} search={:?}",
                r.page,
                r.page_size,
                r.wp_type,
                r.filters.len(),
                r.search_text
            );
            // Entries come straight from the DB (the read source of
            // truth), fully populated — no in-memory snapshot.
            let all_entries = repo::load_entries(&state.db).await?;

            let mut raw_entries: Vec<&crate::catalog::entry::WallpaperEntry> = all_entries
                .iter()
                .filter(|e| r.wp_type.is_empty() || e.wp_type == r.wp_type)
                .collect();
            if !r.skip_types.is_empty() {
                raw_entries.retain(|e| !r.skip_types.iter().any(|t| t == &e.wp_type));
            }

            let mut effective_filters: Vec<_> =
                r.filters.iter().filter_map(filter_rule_from_pb).collect();
            let filter_logics: Vec<_> = r.filter_logics.iter().map(filter_logic_from_pb).collect();
            let sorts: Vec<_> = r.sorts.iter().filter_map(sort_rule_from_pb).collect();
            let search_text = r.search_text.trim();

            // Quick tag filter: keep only wallpapers having any of the
            // selected tags, AND-ed in via its own fresh group.
            if !r.filter_tags.is_empty() {
                let next_group = effective_filters
                    .iter()
                    .map(|f| f.group)
                    .max()
                    .map(|g| g + 1)
                    .unwrap_or(0);
                effective_filters.push(crate::catalog::FilterRule {
                    group: next_group,
                    predicate: crate::catalog::query::FilterPredicate::Tags {
                        values: r.filter_tags.clone(),
                        condition: crate::catalog::query::StringMatch::Is,
                    },
                });
            }

            // Quick content-rating toggles: drop the unselected ratings,
            // each as its own AND-ed group.
            for rating in &r.skip_content_ratings {
                let next_group = effective_filters
                    .iter()
                    .map(|f| f.group)
                    .max()
                    .map(|g| g + 1)
                    .unwrap_or(0);
                effective_filters.push(crate::catalog::FilterRule {
                    group: next_group,
                    predicate: crate::catalog::query::FilterPredicate::ContentRating {
                        value: rating.clone(),
                        condition: crate::catalog::query::StringMatch::IsNot,
                    },
                });
            }

            let matched_keys = if effective_filters.is_empty() && search_text.is_empty() {
                None
            } else {
                Some(
                    repo::list_item_keys_by_wallpaper_query(
                        &state.db,
                        &crate::catalog::CatalogQuery {
                            filters: effective_filters.clone(),
                            logics: filter_logics.clone(),
                            sorts: sorts.clone(),
                            search_text: search_text.to_owned(),
                        },
                    )
                    .await?
                    .into_iter()
                    .collect::<std::collections::HashSet<(String, String)>>(),
                )
            };

            let mut filtered_entries: Vec<&crate::catalog::entry::WallpaperEntry> =
                if let Some(matched_keys) = matched_keys.as_ref() {
                    raw_entries
                        .into_iter()
                        .filter(|e| {
                            crate::catalog::path::relative_under_root(&e.library_root, &e.resource)
                                .map(|rel| matched_keys.contains(&(e.library_root.clone(), rel)))
                                .unwrap_or(false)
                        })
                        .collect()
                } else {
                    raw_entries
                };

            crate::catalog::query::sort_entries(&mut filtered_entries, &sorts);

            let total = filtered_entries.len() as u32;
            let page_size = r.page_size as usize;
            let (offset, take) = if page_size == 0 {
                (0usize, filtered_entries.len())
            } else {
                ((r.page as usize) * page_size, page_size)
            };
            log::info!("WallpaperList: total={total} returning offset={offset} take={take}");

            let page_entries: Vec<&crate::catalog::entry::WallpaperEntry> = filtered_entries
                .into_iter()
                .skip(offset)
                .take(take)
                .collect();

            // Batch-load tags for just the items on this page (avoid
            // an N+1 round-trip when paginating large libraries).
            let page_item_ids: Vec<i64> = page_entries.iter().map(|e| e.item_id).collect();
            let tag_map = repo::list_tags_for_items(&state.db, &page_item_ids).await?;
            let capabilities_by_item = page_entries
                .iter()
                .map(|e| {
                    (
                        e.item_id,
                        (
                            state.source_manager.supports_item_remove(&e.plugin_name),
                            state.source_manager.supports_item_unsubscribe(e),
                        ),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>();

            // WallpaperList skips property schema/overrides; WallpaperGet
            // loads those on demand per item.
            let entries: Vec<pb::WallpaperEntry> = page_entries
                .into_iter()
                .map(|e| {
                    let tags = tag_map.get(&e.item_id).cloned().unwrap_or_default();
                    entry_to_pb(
                        e,
                        tags,
                        String::new(),
                        String::new(),
                        None,
                        capabilities_by_item
                            .get(&e.item_id)
                            .map(|capabilities| capabilities.0)
                            .unwrap_or(false),
                        capabilities_by_item
                            .get(&e.item_id)
                            .map(|capabilities| capabilities.1)
                            .unwrap_or(false),
                    )
                })
                .collect();

            Res::WallpaperList(pb::WallpaperListResponse {
                wallpapers: entries,
                count: total,
            })
        }

        Req::WallpaperGet(r) => {
            let entry = match r.wallpaper_id.parse::<i64>() {
                Ok(iid) => repo::get_entry(&state.db, iid).await?,
                Err(_) => None,
            };
            let entry = entry.ok_or_else(|| Error::WallpaperNotFound(r.wallpaper_id.clone()))?;
            let tags = entry.tags.clone();
            // Source plugin owns the property schema; empty string means
            // the plugin exposes no properties for this item.
            let schema = state
                .source_manager
                .call_properties(&entry.plugin_name, &entry)
                .await
                .ok()
                .flatten()
                .map(|schema| dedupe_predefined_schema(&schema))
                .unwrap_or_default();
            let supports_item_remove = state
                .source_manager
                .supports_item_remove(&entry.plugin_name);
            let supports_item_unsubscribe = state.source_manager.supports_item_unsubscribe(&entry);
            let overrides = repo::get_user_property_overrides_raw(&state.db, entry.item_id)
                .await?
                .unwrap_or_default();
            let layout_override =
                repo::get_wallpaper_layout_override_with_legacy(&state.db, entry.item_id).await?;

            Res::WallpaperGet(pb::WallpaperGetResponse {
                entry: Some(entry_to_pb(
                    &entry,
                    tags,
                    schema,
                    overrides,
                    layout_override,
                    supports_item_remove,
                    supports_item_unsubscribe,
                )),
            })
        }

        Req::WallpaperRemove(r) => {
            let mut wallpaper_ids = Vec::new();
            if !r.wallpaper_id.trim().is_empty() {
                wallpaper_ids.push(r.wallpaper_id);
            }
            wallpaper_ids.extend(
                r.wallpaper_ids
                    .into_iter()
                    .filter(|id| !id.trim().is_empty()),
            );
            wallpaper_ids.sort();
            wallpaper_ids.dedup();
            if wallpaper_ids.is_empty() {
                return Err(Error::InvalidArgument("wallpaper_id is required".into()));
            }

            let mut removed_count = 0;
            for wallpaper_id in wallpaper_ids {
                let entry = match wallpaper_id.parse::<i64>() {
                    Ok(iid) => repo::get_entry(&state.db, iid).await?,
                    Err(_) => None,
                };
                let entry = entry.ok_or_else(|| Error::WallpaperNotFound(wallpaper_id.clone()))?;
                application::remove_wallpaper_entry_files_and_db(state, &entry).await?;
                removed_count += 1;
            }
            application::notify_wallpaper_db_changed(state, 0).await;
            Res::WallpaperRemove(pb::WallpaperRemoveResponse { removed_count })
        }

        Req::WallpaperUnsubscribe(r) => {
            let entry = match r.wallpaper_id.parse::<i64>() {
                Ok(iid) => repo::get_entry(&state.db, iid).await?,
                Err(_) => None,
            };
            let entry = entry.ok_or_else(|| Error::WallpaperNotFound(r.wallpaper_id.clone()))?;
            if !state.source_manager.supports_item_unsubscribe(&entry) {
                return Err(Error::SourceItemUnsubscribeUnsupported(entry.plugin_name));
            }
            let external_id = entry
                .external_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    Error::SourceItemUnsubscribeUnsupported(entry.plugin_name.clone())
                })?;
            state
                .source_manager
                .set_subscription(&entry.plugin_name, external_id, false)
                .await?;
            Res::WallpaperUnsubscribe(pb::WallpaperUnsubscribeResponse {})
        }

        Req::WallpaperPropertySet(r) => {
            let entry = match r.wallpaper_id.parse::<i64>() {
                Ok(iid) => repo::get_entry(&state.db, iid).await?,
                Err(_) => None,
            };
            let entry = entry.ok_or_else(|| Error::WallpaperNotFound(r.wallpaper_id.clone()))?;
            use pb::wallpaper_property_set_request::Operation;
            let value = match r.operation {
                Some(Operation::Value(value)) => Some(value),
                // Older clients encoded an empty reset value as no field.
                Some(Operation::Reset(_)) | None => None,
            };
            repo::set_user_property_override(&state.db, entry.item_id, &r.key, value.as_deref())
                .await?;
            let retained_renderer_ids =
                state.router.renderer_ids_by_resource(&entry.resource).await;
            let persist_tag = format!("item={}", entry.item_id);
            let push_tag = if is_daemon_display_property_key(&r.key) {
                if retained_renderer_ids.is_empty() {
                    String::from("offline")
                } else {
                    let (_, wallpaper_layout_override) =
                        repo::get_wallpaper_render_properties(&state.db, entry.item_id).await?;
                    for renderer_id in &retained_renderer_ids {
                        state
                            .router
                            .update_renderer_assignment_layout(
                                renderer_id,
                                wallpaper_layout_override,
                            )
                            .await;
                    }
                    format!("display-layout={}", retained_renderer_ids.join(","))
                }
            } else {
                for renderer_id in &retained_renderer_ids {
                    state
                        .router
                        .update_renderer_assignment_property(renderer_id, &r.key, value.as_deref())
                        .await;
                }
                let mut pushed = Vec::new();
                let mut schema_default = None;
                for renderer_id in &retained_renderer_ids {
                    let Some(handle) = state.renderer_manager.get(renderer_id).await else {
                        continue;
                    };
                    let effective_value = if let Some(value) = value.as_ref() {
                        value.clone()
                    } else if let Some(default) = handle.default_user_property(&r.key) {
                        default
                    } else {
                        if schema_default.is_none() {
                            let schema = state
                                .source_manager
                                .call_properties(&entry.plugin_name, &entry)
                                .await
                                .ok()
                                .flatten();
                            schema_default = schema.as_deref().and_then(|schema| {
                                user_property_default_wire_value(schema, &r.key)
                            });
                        }
                        schema_default.clone().unwrap_or_else(|| {
                            log::warn!(
                                "WallpaperPropertySet: reset {} on {} has no default value",
                                r.key,
                                r.wallpaper_id
                            );
                            String::new()
                        })
                    };
                    let settings = vec![(
                        canonical_user_property_key(&r.key).to_string(),
                        effective_value,
                    )];
                    state
                        .renderer_manager
                        .send_control(renderer_id, ControlMsg::SettingChanged { settings })
                        .await
                        .map_err(|error| {
                            Error::Internal(anyhow::anyhow!(
                                "send setting_changed to renderer {renderer_id}: {error}"
                            ))
                        })?;
                    pushed.push(renderer_id.clone());
                }
                if pushed.is_empty() {
                    String::from("offline")
                } else {
                    format!("renderer={}", pushed.join(","))
                }
            };
            let operation = value.as_deref().unwrap_or("<reset>");
            log::debug!(
                "WallpaperPropertySet: {}={} on {} persist={} push={}",
                r.key,
                operation,
                r.wallpaper_id,
                persist_tag,
                push_tag
            );
            Res::WallpaperPropertySet(pb::WallpaperPropertySetResponse {})
        }

        Req::WallpaperLayoutSet(r) => {
            let entry = match r.wallpaper_id.parse::<i64>() {
                Ok(iid) => repo::get_entry(&state.db, iid).await?,
                Err(_) => None,
            };
            let entry = entry.ok_or_else(|| Error::WallpaperNotFound(r.wallpaper_id.clone()))?;
            let layout = if r.clear {
                None
            } else {
                let Some(layout) = r.layout.as_ref() else {
                    return Err(Error::InvalidArgument(
                        "wallpaper_layout_set requires layout unless clear=true".to_string(),
                    ));
                };
                Some(resolved_layout_from_pb(layout))
            };
            repo::set_wallpaper_layout_override(&state.db, entry.item_id, layout).await?;

            let override_layout = layout
                .map(WallpaperLayoutOverride::from_resolved)
                .unwrap_or_default();
            for renderer_id in state.router.renderer_ids_by_resource(&entry.resource).await {
                state
                    .router
                    .update_renderer_assignment_layout(&renderer_id, override_layout)
                    .await;
            }

            let layout_override = layout.map(WallpaperLayoutOverride::from_resolved);
            let supports_item_remove = state
                .source_manager
                .supports_item_remove(&entry.plugin_name);
            let supports_item_unsubscribe = state.source_manager.supports_item_unsubscribe(&entry);
            Res::WallpaperLayoutSet(pb::WallpaperLayoutSetResponse {
                entry: Some(entry_to_pb(
                    &entry,
                    entry.tags.clone(),
                    String::new(),
                    String::new(),
                    layout_override,
                    supports_item_remove,
                    supports_item_unsubscribe,
                )),
            })
        }

        Req::WallpaperScan(_) => {
            // Fire-and-forget: kick the rescan onto the TaskManager and
            // return immediately; completion arrives via server events.
            let scan_state = state.clone();
            state.tasks.spawn_async_unique(
                tasks::TaskKind::Generic,
                "scan/refresh",
                "scan/refresh",
                async move {
                    application::refresh_sources(&scan_state)
                        .await
                        .map(|_| ())
                        .map_err(anyhow::Error::from)
                },
            );
            Res::WallpaperScan(pb::WallpaperScanResponse { count: 0 })
        }

        Req::SourceList(_) => {
            let plugins = state.source_plugins.read().await;
            let sources = plugins
                .iter()
                .cloned()
                .map(|p| pb::SourcePluginInfo {
                    name: p.name,
                    types: p.types,
                    version: p.version,
                    library_label: p.library_label,
                    library_hint: p.library_hint,
                    plugin_id: p.plugin_id,
                    settings: p
                        .settings
                        .iter()
                        .map(crate::control_proto::source_setting_to_proto)
                        .collect(),
                })
                .collect();
            Res::SourceList(pb::SourceListResponse { sources })
        }

        Req::DisplayList(_) => {
            let snap = state.router.snapshot_displays().await;
            let displays = snap
                .into_iter()
                .map(|d| display_snapshot_to_pb(d, &state.settings))
                .collect();
            Res::DisplayList(pb::DisplayListResponse { displays })
        }

        Req::GpuList(_) => {
            let gpus = state
                .system_info
                .gpus()
                .iter()
                .map(gpu_info_to_pb)
                .collect();
            Res::GpuList(pb::GpuListResponse { gpus })
        }

        Req::PluginInstall(r) => {
            let result =
                crate::application::install_plugin_archive(state, r.zip_path.clone()).await?;
            Res::PluginInstall(pb::PluginInstallResponse {
                plugin_id: result.plugin_id,
                needs_restart: result.needs_restart,
            })
        }

        Req::PluginUpdateCheck(r) => {
            let plugin_id = (!r.plugin_id.is_empty()).then_some(r.plugin_id);
            let submission =
                crate::application::spawn_plugin_update_check(state, plugin_id.clone());
            let updates = crate::application::plugin_update_snapshots(state, plugin_id.as_deref())
                .await
                .into_iter()
                .map(plugin_update_info_to_pb)
                .collect();
            Res::PluginUpdateCheck(pb::PluginUpdateCheckResponse {
                updates,
                query_id: submission.query_id,
            })
        }

        Req::PluginUpdateInstall(r) => {
            let submission = crate::application::spawn_plugin_update_install(state, r.plugin_id)?;
            Res::PluginUpdateInstall(pb::PluginUpdateInstallResponse {
                query_id: submission.query_id,
            })
        }

        Req::DisplayLayoutSet(r) => {
            let new_fillmode = if r.clear_fillmode {
                None
            } else {
                r.r#override
                    .as_ref()
                    .filter(|o| o.fillmode_set)
                    .and_then(|o| fillmode_from_pb(o.fillmode))
            };
            let new_align = if r.clear_align {
                None
            } else {
                r.r#override
                    .as_ref()
                    .filter(|o| o.align_set)
                    .and_then(|o| align_from_pb(o.align))
            };
            let new_location = if r.clear_location || r.clear_align {
                None
            } else {
                r.r#override
                    .as_ref()
                    .filter(|o| o.location_set)
                    .map(|o| location_from_pb(o.location_x, o.location_y))
                    .or_else(|| {
                        new_align.map(crate::wallframe::display::layout::Location::from_align)
                    })
            };
            let new_rotation = if r.clear_rotation {
                None
            } else {
                r.r#override
                    .as_ref()
                    .filter(|o| o.rotation_set)
                    .and_then(|o| rotation_from_pb(o.rotation))
            };
            let target_id = state
                .router
                .set_display_layout(
                    (r.display_id != 0).then_some(r.display_id),
                    r.name.clone(),
                    new_fillmode,
                    new_location,
                    new_align,
                    new_rotation,
                    r.clear_fillmode,
                    r.clear_align || r.clear_location,
                    r.clear_rotation,
                )
                .await;
            let display = match target_id {
                Some(id) => state
                    .router
                    .snapshot_display(id)
                    .await
                    .map(|d| display_snapshot_to_pb(d, &state.settings)),
                None => None,
            };
            Res::DisplayLayoutSet(pb::DisplayLayoutSetResponse { display })
        }

        Req::DisplayRename(r) => {
            let new_alias = if r.clear || r.alias.trim().is_empty() {
                None
            } else {
                Some(r.alias.clone())
            };
            let target_id = state
                .router
                .set_display_alias(
                    (r.display_id != 0).then_some(r.display_id),
                    r.name.clone(),
                    new_alias,
                    r.clear,
                )
                .await;
            let display = match target_id {
                Some(id) => state
                    .router
                    .snapshot_display(id)
                    .await
                    .map(|d| display_snapshot_to_pb(d, &state.settings)),
                None => None,
            };
            Res::DisplayRename(pb::DisplayRenameResponse { display })
        }

        Req::RemoteAvailability(_) => {
            let sources = state.source_manager.discover_sources_with_status().await?;
            let default_source_id = sources
                .first()
                .map(|s| s.plugin_id.clone())
                .unwrap_or_default();
            Res::RemoteAvailability(pb::RemoteAvailabilityResponse {
                sources: sources
                    .into_iter()
                    .map(|s| {
                        let tags = s
                            .filters
                            .iter()
                            .flat_map(|filter| filter.values.iter().cloned())
                            .collect();
                        let settings = s
                            .settings
                            .iter()
                            .map(crate::control_proto::source_setting_to_proto)
                            .collect();
                        let actions = s
                            .actions
                            .iter()
                            .map(crate::control_proto::source_action_to_proto)
                            .collect();
                        let status = s
                            .status
                            .iter()
                            .map(crate::control_proto::source_status_to_proto)
                            .collect();
                        let remote_capability = match s.remote_capability {
                            Some(crate::plugin::source::RemoteCapability::Download) => {
                                pb::RemoteCapability::Download
                            }
                            Some(crate::plugin::source::RemoteCapability::Subscription) => {
                                pb::RemoteCapability::Subscription
                            }
                            None => pb::RemoteCapability::Unspecified,
                        };
                        let content_dir = if remote_capability == pb::RemoteCapability::Download {
                            remote_content_dir(&s.plugin_id)
                                .to_string_lossy()
                                .to_string()
                        } else {
                            String::new()
                        };
                        pb::RemoteSourceInfo {
                            id: s.plugin_id,
                            name: s.name,
                            supports_search: s.supports_search,
                            sorts: s
                                .sorts
                                .into_iter()
                                .map(|sort| pb::RemoteSortOption {
                                    key: sort.key,
                                    label: sort.label,
                                })
                                .collect(),
                            tags,
                            content_dir,
                            owner_plugin_id: s.owner_plugin_id,
                            settings,
                            display_name: s.display_name,
                            actions,
                            status,
                            remote_capability: remote_capability as i32,
                            remote_hint: s.remote_hint,
                            avatar_url: s.avatar_url,
                            filters: s
                                .filters
                                .into_iter()
                                .map(|filter| pb::RemoteFilterDef {
                                    id: filter.id,
                                    title: filter.title,
                                    r#type: match filter.ty {
                                        crate::plugin::source::DiscoverFilterType::Select => {
                                            pb::RemoteFilterType::Select as i32
                                        }
                                        crate::plugin::source::DiscoverFilterType::MultiSelect => {
                                            pb::RemoteFilterType::MultiSelect as i32
                                        }
                                        crate::plugin::source::DiscoverFilterType::Toggle => {
                                            pb::RemoteFilterType::Toggle as i32
                                        }
                                    },
                                    values: filter.values,
                                    description: filter.description,
                                    confirmation: filter.confirmation,
                                })
                                .collect(),
                        }
                    })
                    .collect(),
                default_source_id,
            })
        }

        Req::RemoteSearch(r) => {
            let source_id = match application::resolve_remote_source_id(state, &r.source_id).await {
                Ok(v) => v,
                Err(e) => {
                    return Ok(Res::RemoteSearch(pb::RemoteSearchResponse {
                        items: Vec::new(),
                        has_more: false,
                        error: query_error("remote search", e),
                    }));
                }
            };
            let sort_key = if r.sort_key.trim().is_empty() {
                state
                    .source_manager
                    .discover_sources()?
                    .into_iter()
                    .find(|s| s.plugin_id == source_id)
                    .and_then(|s| s.sorts.into_iter().next())
                    .map(|s| s.key)
                    .unwrap_or_default()
            } else {
                r.sort_key.clone()
            };
            let result = state
                .source_manager
                .call_discover(&source_id, &r.query, &sort_key, r.page, &r.required_tags)
                .await;
            match result {
                Ok(result) => {
                    let downloaded_capability = state
                        .source_manager
                        .discover_sources()?
                        .into_iter()
                        .find(|source| source.plugin_id == source_id)
                        .and_then(|source| source.remote_capability)
                        == Some(crate::plugin::source::RemoteCapability::Download);
                    let mut items = Vec::with_capacity(result.items.len());
                    for item in result.items {
                        let downloaded = if downloaded_capability {
                            repo::has_item_by_plugin_external_id(&state.db, &source_id, &item.id)
                                .await?
                        } else {
                            false
                        };
                        items.push(pb::RemoteItem {
                            id: item.id,
                            title: item.title,
                            preview_url: item.preview_url,
                            author: item.author,
                            downloaded,
                            source_id: source_id.clone(),
                            wp_type: item.wp_type,
                        });
                    }
                    Res::RemoteSearch(pb::RemoteSearchResponse {
                        items,
                        has_more: result.has_more,
                        error: String::new(),
                    })
                }
                Err(e) => Res::RemoteSearch(pb::RemoteSearchResponse {
                    items: Vec::new(),
                    has_more: false,
                    error: query_error("remote search", e),
                }),
            }
        }

        Req::RemoteDownload(r) => {
            let source_id = match application::resolve_remote_source_id(state, &r.source_id).await {
                Ok(v) => v,
                Err(e) => {
                    return Ok(Res::RemoteDownload(pb::RemoteDownloadResponse {
                        accepted: false,
                        error: query_error("remote download", e),
                    }));
                }
            };
            if r.id.trim().is_empty() {
                return Ok(Res::RemoteDownload(pb::RemoteDownloadResponse {
                    accepted: false,
                    error: query_error("remote download", "remote id is empty"),
                }));
            }
            if application::remote_capability(state, &source_id)?
                != Some(crate::plugin::source::RemoteCapability::Download)
            {
                return Ok(Res::RemoteDownload(pb::RemoteDownloadResponse {
                    accepted: false,
                    error: query_error(
                        "remote download",
                        "remote source does not support downloads",
                    ),
                }));
            }
            let task_state = state.clone();
            let task_source_id = source_id.clone();
            let task_id = r.id.clone();
            state.tasks.spawn_async_unique(
                tasks::TaskKind::Generic,
                format!("remote/download/{task_source_id}/{task_id}"),
                format!("remote/download {task_source_id}:{task_id}"),
                async move {
                    let result = application::download_remote(
                        task_state.clone(),
                        task_source_id.clone(),
                        task_id.clone(),
                    )
                    .await;
                    if let Err(e) = &result {
                        application::publish_remote_download_progress(
                            &task_state,
                            &task_source_id,
                            &task_id,
                            crate::events::RemoteDownloadState::Error,
                            e.to_string(),
                        );
                    }
                    result
                },
            );
            Res::RemoteDownload(pb::RemoteDownloadResponse {
                accepted: true,
                error: String::new(),
            })
        }

        Req::RemoteUninstall(r) => {
            let source_id = match application::resolve_remote_source_id(state, &r.source_id).await {
                Ok(v) => v,
                Err(e) => {
                    return Ok(Res::RemoteUninstall(pb::RemoteUninstallResponse {
                        removed: false,
                        error: query_error("remote remove", e),
                    }));
                }
            };
            if application::remote_capability(state, &source_id)?
                != Some(crate::plugin::source::RemoteCapability::Download)
            {
                return Ok(Res::RemoteUninstall(pb::RemoteUninstallResponse {
                    removed: false,
                    error: query_error(
                        "remote remove",
                        "remote source does not own downloaded content",
                    ),
                }));
            }
            let rows = repo::list_items_by_plugin_external_id(&state.db, &source_id, &r.id).await?;
            if rows.is_empty() {
                Res::RemoteUninstall(pb::RemoteUninstallResponse {
                    removed: false,
                    error: query_error("remote remove", "remote item is not downloaded"),
                })
            } else {
                for (item, lib) in rows {
                    let Some(mut entry) = repo::get_entry(&state.db, item.id).await? else {
                        continue;
                    };
                    if entry.library_root.is_empty() {
                        entry.library_root = lib.path;
                    }
                    application::remove_wallpaper_entry_files_and_db(state, &entry).await?;
                }
                application::notify_wallpaper_db_changed(state, 0).await;
                Res::RemoteUninstall(pb::RemoteUninstallResponse {
                    removed: true,
                    error: String::new(),
                })
            }
        }

        Req::RemoteDetails(r) => {
            let source_id = match application::resolve_remote_source_id(state, &r.source_id).await {
                Ok(v) => v,
                Err(e) => {
                    return Ok(Res::RemoteDetails(pb::RemoteDetailsResponse {
                        description: String::new(),
                        size: String::new(),
                        tags: Vec::new(),
                        error: query_error("remote details", e),
                        width: 0,
                        height: 0,
                        web_url: String::new(),
                        author: String::new(),
                    }));
                }
            };
            let result = state.source_manager.call_details(&source_id, &r.id).await;
            match result {
                Ok(details) => Res::RemoteDetails(pb::RemoteDetailsResponse {
                    description: details.description,
                    size: details.size,
                    tags: details.tags,
                    error: String::new(),
                    width: details.width.unwrap_or(0),
                    height: details.height.unwrap_or(0),
                    web_url: details.web_url,
                    author: details.author,
                }),
                Err(e) => Res::RemoteDetails(pb::RemoteDetailsResponse {
                    description: String::new(),
                    size: String::new(),
                    tags: Vec::new(),
                    error: query_error("remote details", e),
                    width: 0,
                    height: 0,
                    web_url: String::new(),
                    author: String::new(),
                }),
            }
        }

        Req::WallpaperApply(r) => {
            let res = application::apply_wallpaper(
                state,
                &r.wallpaper_id,
                application::ApplyRequest {
                    source: application::ApplySource::UserWallpaper,
                    display_ids: (!r.display_ids.is_empty()).then_some(r.display_ids),
                    renderer_name: (!r.renderer_name.is_empty()).then_some(r.renderer_name),
                    first_frame_timeout: Some(application::APPLY_FIRST_FRAME_TIMEOUT),
                    require_display: true,
                    sharing: application::RendererSharingPolicy::UseSettings,
                },
            )
            .await?;
            // Reset the rotator deadline so a manual apply gets the
            // full quiet window before the next auto tick.
            state.rotation.kick();

            Res::WallpaperApply(pb::WallpaperApplyResponse {
                renderer_id: res.renderer_id,
                wallpaper_id: res.entry.item_id.to_string(),
                wp_type: res.entry.wp_type,
                name: res.entry.name,
                deferred: res.activation == application::ApplyActivation::Deferred,
                stopped_playlists: res
                    .stopped_playlists
                    .into_iter()
                    .map(|playlist| pb::WallpaperApplyStoppedPlaylist {
                        playlist_id: playlist.id,
                        playlist_name: playlist.name,
                        display_ids: playlist.display_ids,
                        all_displays: playlist.all_displays,
                    })
                    .collect(),
            })
        }

        Req::WallpaperApplyViaPortal(r) => {
            let res = crate::application::apply_wallpaper_via_portal(
                state,
                &r.wallpaper_id,
                application::ApplySource::UserWallpaper,
            )
            .await?;
            Res::WallpaperApplyViaPortal(pb::WallpaperApplyViaPortalResponse {
                wallpaper_id: res.wallpaper_id,
                uri: res.uri,
            })
        }

        Req::AutostartGet(_) => Res::AutostartGet(pb::AutostartGetResponse {
            enabled: state.autostart.enabled(&state.settings)?,
        }),

        Req::AutostartSet(r) => Res::AutostartSet(pb::AutostartSetResponse {
            enabled: state
                .autostart
                .set_enabled(&state.settings, r.enabled)
                .await?,
        }),

        Req::PluginAction(r) => {
            use crate::plugin::source::SourceActionKind;
            match state.source_manager.action_kind(&r.plugin_id, &r.action_id) {
                Some(SourceActionKind::Invoke | SourceActionKind::Form) => {
                    match state
                        .source_manager
                        .invoke_action(&r.plugin_id, &r.action_id, &r.values)
                        .await
                    {
                        Ok(()) => {
                            state.events.publish(GlobalEvent::PluginStateChanged);
                            Res::PluginAction(pb::PluginActionResponse {
                                accepted: true,
                                error: String::new(),
                                session_id: String::new(),
                            })
                        }
                        Err(error) => Res::PluginAction(pb::PluginActionResponse {
                            accepted: false,
                            error: query_error("plugin action", error),
                            session_id: String::new(),
                        }),
                    }
                }
                Some(SourceActionKind::QrLogin) => {
                    match state.qr_login.start(&r.plugin_id, &r.action_id).await {
                        Ok(session_id) => Res::PluginAction(pb::PluginActionResponse {
                            accepted: true,
                            error: String::new(),
                            session_id,
                        }),
                        Err(error) => Res::PluginAction(pb::PluginActionResponse {
                            accepted: false,
                            error: query_error("plugin action", error),
                            session_id: String::new(),
                        }),
                    }
                }
                None => Res::PluginAction(pb::PluginActionResponse {
                    accepted: false,
                    error: query_error(
                        "plugin action",
                        format!("unknown action {}.{}", r.plugin_id, r.action_id),
                    ),
                    session_id: String::new(),
                }),
            }
        }

        Req::QrLoginCancel(r) => {
            let cancelled = state.qr_login.cancel(&r.session_id).await;
            Res::QrLoginCancel(pb::QrLoginCancelResponse { cancelled })
        }

        Req::RemoteSettingsPatch(r) => {
            let source_id = application::resolve_remote_source_id(state, &r.source_id).await?;
            let values = state
                .source_manager
                .validate_remote_settings_patch(&source_id, r.values)?;
            if !values.is_empty() {
                state.settings.update(|settings| {
                    settings
                        .plugins
                        .entry(source_id)
                        .or_default()
                        .extend(values);
                });
                state.events.publish(GlobalEvent::SettingsChanged);
            }
            Res::RemoteSettingsPatch(pb::Empty {})
        }

        Req::SubscriptionStatus(r) => {
            let source_id = match application::resolve_remote_source_id(state, &r.source_id).await {
                Ok(source_id) => source_id,
                Err(error) => {
                    return Ok(Res::SubscriptionStatus(pb::SubscriptionStatusResponse {
                        items: Vec::new(),
                        error: query_error("subscription status", error),
                    }));
                }
            };
            if r.item_ids.len() > 200 {
                return Ok(Res::SubscriptionStatus(pb::SubscriptionStatusResponse {
                    items: Vec::new(),
                    error: query_error(
                        "subscription status",
                        "subscription status accepts at most 200 item ids",
                    ),
                }));
            }
            match state
                .source_manager
                .subscription_status(&source_id, &r.item_ids)
                .await
            {
                Ok(items) => Res::SubscriptionStatus(pb::SubscriptionStatusResponse {
                    items: items
                        .into_iter()
                        .map(|item| pb::SubscriptionItemState {
                            id: item.id,
                            state: match item.state {
                                crate::plugin::source::SubscriptionState::Unknown => {
                                    pb::SubscriptionState::Unknown
                                }
                                crate::plugin::source::SubscriptionState::Unsubscribed => {
                                    pb::SubscriptionState::Unsubscribed
                                }
                                crate::plugin::source::SubscriptionState::Subscribed => {
                                    pb::SubscriptionState::Subscribed
                                }
                            } as i32,
                        })
                        .collect(),
                    error: String::new(),
                }),
                Err(error) => Res::SubscriptionStatus(pb::SubscriptionStatusResponse {
                    items: Vec::new(),
                    error: query_error("subscription status", error),
                }),
            }
        }

        Req::SubscriptionSet(r) => {
            let source_id = match application::resolve_remote_source_id(state, &r.source_id).await {
                Ok(source_id) => source_id,
                Err(error) => {
                    return Ok(Res::SubscriptionSet(pb::SubscriptionSetResponse {
                        accepted: false,
                        error: query_error("subscription update", error),
                    }));
                }
            };
            if r.item_id.trim().is_empty() {
                return Ok(Res::SubscriptionSet(pb::SubscriptionSetResponse {
                    accepted: false,
                    error: query_error("subscription update", "subscription item id is empty"),
                }));
            }
            match state
                .source_manager
                .set_subscription(&source_id, &r.item_id, r.subscribed)
                .await
            {
                Ok(()) => Res::SubscriptionSet(pb::SubscriptionSetResponse {
                    accepted: true,
                    error: String::new(),
                }),
                Err(error) => Res::SubscriptionSet(pb::SubscriptionSetResponse {
                    accepted: false,
                    error: query_error("subscription update", error),
                }),
            }
        }

        Req::SettingsGet(_) => {
            let snap = state.settings.snapshot();
            Res::SettingsGet(pb::SettingsGetResponse {
                global: Some(global_to_pb(&snap.global)),
                plugins: snap
                    .plugins
                    .into_iter()
                    .map(|(k, v)| (k, pb::PluginSettings { values: v }))
                    .collect(),
            })
        }

        Req::SettingsSet(r) => {
            // Full replace. Missing `global` falls back to current
            // values so callers can update only plugin settings.
            let mut new_plugins: std::collections::HashMap<
                String,
                std::collections::HashMap<String, String>,
            > = r.plugins.into_iter().map(|(k, v)| (k, v.values)).collect();

            // Schema validation up-front. Reject the entire RPC if any
            // declared key fails type, bounds, or choices.
            {
                let registry = state.renderer_manager.registry_snapshot();
                for (plugin_name, kv) in new_plugins.iter_mut() {
                    let Some(def) = registry
                        .all_renderers()
                        .into_iter()
                        .find(|d| &d.name == plugin_name)
                    else {
                        continue;
                    };
                    if def.settings.is_empty() {
                        continue;
                    }
                    for (k, v) in kv.iter_mut() {
                        let Some(schema) = def.settings.get(k) else {
                            continue;
                        };
                        let coerced =
                            crate::plugin::renderer_registry::coerce_and_validate(k, v, schema)
                                .map_err(|e| {
                                    Error::SettingsValidationFailed(format!("{plugin_name}.{e}"))
                                })?;
                        *v = coerced;
                    }
                }
            }

            let previous_settings = state.settings.snapshot();
            let previous_filter = previous_settings.global.wallpaper_filter.clone();
            let prev_layout = previous_settings.global.layout.clone();
            let prev_auto_replay = previous_settings.global.auto_replay;
            let prev_pause_effect = previous_settings.global.pause_effect;
            let prev_queue_mode = previous_settings.global.queue_mode.clone();
            let prev_rotation_secs = previous_settings.global.rotation_secs;
            let prev_hide_tray = previous_settings.global.hide_tray_icon;
            state.settings.update(|s| {
                if let Some(g) = r.global.as_ref() {
                    let filters: Vec<_> = g
                        .wallpaper_filters
                        .iter()
                        .filter_map(filter_rule_from_pb)
                        .collect();
                    let filter_logics: Vec<_> = g
                        .wallpaper_filter_logics
                        .iter()
                        .map(filter_logic_from_pb)
                        .collect();
                    let sorts: Vec<_> = g
                        .wallpaper_sorts
                        .iter()
                        .filter_map(sort_rule_from_pb)
                        .collect();
                    s.global.wallpaper_filter =
                        WallpaperFilterState::from_catalog(&filters, &filter_logics);
                    s.global.wallpaper_sorts = WallpaperSortRuleState::vec_from_catalog(&sorts);
                    s.global.wallpaper_skip_types = g.wallpaper_skip_types.clone();
                    s.global.wallpaper_filter_tags = g.wallpaper_filter_tags.clone();
                    s.global.wallpaper_skip_content_ratings =
                        g.wallpaper_skip_content_ratings.clone();
                    if let Some(ld) = g.layout_defaults.as_ref() {
                        if let Some(fm) = fillmode_from_pb(ld.fillmode) {
                            s.global.layout.fillmode = fm;
                        }
                        if let Some(al) = align_from_pb(ld.align) {
                            s.global.layout.align = al;
                        }
                        if ld.location_set {
                            s.global.layout.location =
                                Some(location_from_pb(ld.location_x, ld.location_y));
                        }
                        if let Some(rt) = rotation_from_pb(ld.rotation) {
                            s.global.layout.rotation = rt;
                        }
                    }
                    if let Some(policy) = g.auto_replay.as_ref() {
                        s.global.auto_replay = Some(auto_replay_from_pb(policy));
                    }
                    if let Some(config) = g.pause_effect.as_ref() {
                        s.global.pause_effect = pause_effect_from_pb(config);
                    }
                    if !g.queue_mode.is_empty() {
                        s.global.queue_mode = g.queue_mode.clone();
                    }
                    s.global.rotation_secs = g.rotation_secs;
                    s.global.audio_fade_ms =
                        g.audio_fade_ms.min(crate::settings::MAX_AUDIO_FADE_MS);
                    if let Some(mute_when_other_audio) = g.mute_when_other_audio {
                        s.global.mute_when_other_audio = mute_when_other_audio;
                    }
                    s.global.audio_capture_enabled = g.audio_capture_enabled;
                    s.global.pointer_forwarding_enabled = g.pointer_forwarding_enabled;
                    s.global.plugin_update_notifications = !g.disable_plugin_update_notifications;
                    s.global.duplicate_renderers_for_same_wallpaper =
                        g.duplicate_renderers_for_same_wallpaper;
                    if let Some(renderer) = g.renderer.as_ref() {
                        if let Some(enable_audio) = renderer.enable_audio {
                            s.global.renderer.enable_audio = enable_audio;
                        }
                        if let Some(volume) = renderer.volume {
                            s.global.renderer.volume =
                                volume.min(crate::settings::MAX_RENDERER_VOLUME);
                        }
                    }
                    s.global.hide_tray_icon = g.hide_tray_icon;
                }
                s.plugins = new_plugins.clone();
            });
            let current_settings = state.settings.snapshot();
            let new_filter = current_settings.global.wallpaper_filter.clone();
            if new_filter != previous_filter {
                log::debug!(
                    "wallpaper filter updated: old={:?}, new={:?}",
                    previous_filter,
                    new_filter
                );
                // Filter change invalidates the queue's shuffle round;
                // the next pick materializes the new candidate set.
                state.queue.lock().await.reset_shuffle_round();
            }
            let new_layout = current_settings.global.layout.clone();
            if new_layout != prev_layout {
                state.router.resync_all_compositions().await;
                // Push fresh DisplaySnapshot so subscribers see new
                // effective_layout values.
                let snap = state.router.snapshot_displays().await;
                state.router.emit_displays_replace_for_settings_change(snap);
            }
            if current_settings.global.auto_replay != prev_auto_replay {
                state.router.resync_auto_replay().await;
            }
            if current_settings.global.pause_effect != prev_pause_effect {
                state.router.resync_presentation_configs().await;
            }
            // Hot-apply queue mode and rotation interval; auto replay re-reads
            // settings on every display state event.
            let new_queue_mode = current_settings.global.queue_mode.clone();
            if new_queue_mode != prev_queue_mode {
                if let Some(m) = playback::Mode::from_str(&new_queue_mode) {
                    state.queue.lock().await.set_mode(m);
                }
            }
            let new_rotation_secs = current_settings.global.rotation_secs;
            if new_rotation_secs != prev_rotation_secs {
                state.rotation.set_interval(new_rotation_secs);
            }
            let new_hide_tray = current_settings.global.hide_tray_icon;
            if new_hide_tray != prev_hide_tray {
                if state.no_tray {
                    log::info!(
                        "--no-tray active; hide_tray_icon={new_hide_tray} takes effect next run"
                    );
                } else if new_hide_tray {
                    crate::system::tray::ensure_stopped(state).await;
                } else {
                    crate::system::tray::ensure_started(state.clone()).await;
                }
            }
            let mut apply_failures: Vec<String> = Vec::new();
            let registry = state.renderer_manager.registry_snapshot();
            let live_ids = state.renderer_manager.list().await;
            for def in registry.all_renderers() {
                let old_kv = previous_settings.resolved_renderer_settings(def);
                let new_kv = current_settings.resolved_renderer_settings(def);
                let kv: Vec<(String, String)> = def
                    .settings
                    .keys()
                    .filter_map(|key| {
                        let value = new_kv.get(key)?;
                        (old_kv.get(key) != Some(value)).then(|| (key.clone(), value.clone()))
                    })
                    .collect();
                if kv.is_empty() {
                    continue;
                }
                state
                    .router
                    .update_renderer_assignment_settings(&def.name, &kv)
                    .await;
                for id in &live_ids {
                    let Some(handle) = state.renderer_manager.get(id).await else {
                        continue;
                    };
                    if handle.name != def.name {
                        continue;
                    }
                    if let Err(e) = state
                        .renderer_manager
                        .send_setting_changed(id, kv.clone(), None)
                        .await
                    {
                        apply_failures.push(format!("{id} ({}): {e}", def.name));
                    }
                }
            }
            // Push the merged post-write state to all WS subscribers so
            // a second UI bound to the same daemon stays in sync.
            state.events.publish(GlobalEvent::SettingsChanged);
            if !apply_failures.is_empty() {
                return Err(Error::SettingsApplyFailed(format!(
                    "{} renderer(s): {}",
                    apply_failures.len(),
                    apply_failures.join("; ")
                )));
            }
            Res::SettingsSet(pb::Empty {})
        }

        Req::LibraryList(_) => {
            let snap = application::list_library_snapshots(&state.db).await;
            Res::LibraryList(pb::LibraryListResponse {
                libraries: snap.into_iter().map(library_instance_to_pb).collect(),
            })
        }

        Req::LibraryAdd(r) => {
            let plugin = repo::find_plugin_by_name(&state.db, &r.plugin_name)
                .await?
                .ok_or_else(|| Error::SourcePluginNotFound(r.plugin_name.clone()))?;
            let lib = repo::add_library(&state.db, plugin.id, &r.path).await?;
            let snap = LibrarySnapshot {
                id: lib.id,
                path: lib.path,
                plugin_name: r.plugin_name,
            };
            let added_path = snap.path.clone();
            state.router.upsert_library(snap);
            state.events.publish(GlobalEvent::LibrariesAdded {
                paths: vec![added_path],
            });
            // Rescan immediately so the new library reaches the DB and UI
            // without waiting for restart.
            let rescan_state = state.clone();
            state.tasks.spawn_async_unique(
                tasks::TaskKind::Generic,
                "scan/refresh",
                "scan/refresh-after-library-add",
                async move {
                    application::refresh_sources(&rescan_state)
                        .await
                        .map(|_| ())
                        .map_err(anyhow::Error::from)
                },
            );
            Res::LibraryAdd(pb::Empty {})
        }

        Req::LibraryAutoDetect(_) => {
            let added = application::auto_detect_libraries(&state).await?;
            Res::LibraryAutoDetect(pb::LibraryAutoDetectResponse {
                added: added.into_iter().map(library_instance_to_pb).collect(),
            })
        }

        Req::LibraryRemove(r) => {
            repo::remove_library(&state.db, r.id).await?;
            state.router.remove_library(r.id);
            let rescan_state = state.clone();
            state.tasks.spawn_async_unique(
                tasks::TaskKind::Generic,
                "scan/refresh",
                "scan/refresh-after-library-remove",
                async move {
                    application::refresh_sources(&rescan_state)
                        .await
                        .map(|_| ())
                        .map_err(anyhow::Error::from)
                },
            );
            Res::LibraryRemove(pb::Empty {})
        }

        // ---- queue status (user-saved playlists removed) -----------------
        Req::PlaylistList(_) => {
            let items = crate::model::repo::playlists::list(&state.db).await?;
            let mut playlists = Vec::with_capacity(items.len());
            for s in items {
                let entry_ids = crate::model::repo::playlists::entry_ids(&state.db, s.id)
                    .await?
                    .into_iter()
                    .map(|e| e.to_string())
                    .collect();
                playlists.push(pb::PlaylistSummary {
                    id: s.id,
                    name: s.name,
                    source_kind: "curated".into(),
                    mode: queue_mode_to_pb_playlist(s.mode),
                    interval_secs: s.interval_secs,
                    item_count: s.item_count,
                    entry_ids,
                });
            }
            Res::PlaylistList(pb::PlaylistListResponse { playlists })
        }

        Req::PlaylistCreate(r) => {
            let mode = pb_playlist_mode_to_queue(r.mode);
            let id = crate::model::repo::playlists::create(
                &state.db,
                &r.name,
                mode,
                r.interval_secs,
                tasks::now_ms(),
                &parse_entry_ids(&r.entry_ids),
            )
            .await?;
            state.events.publish(GlobalEvent::PlaylistChanged);
            Res::PlaylistCreate(pb::PlaylistCreateResponse { id })
        }

        Req::PlaylistDelete(r) => {
            application::deactivate_for_playlist(&state, r.id).await;
            crate::model::repo::playlists::delete(&state.db, r.id).await?;
            state.events.publish(GlobalEvent::PlaylistChanged);
            Res::PlaylistDelete(pb::Empty {})
        }

        Req::PlaylistRename(r) => {
            crate::model::repo::playlists::rename(&state.db, r.id, &r.name, tasks::now_ms())
                .await?;
            state.events.publish(GlobalEvent::PlaylistChanged);
            Res::PlaylistRename(pb::Empty {})
        }

        Req::PlaylistSetItems(r) => {
            crate::model::repo::playlists::set_items(
                &state.db,
                r.id,
                &parse_entry_ids(&r.entry_ids),
                tasks::now_ms(),
            )
            .await?;
            application::rebuild_for_playlist(&state, r.id).await;
            state.events.publish(GlobalEvent::PlaylistChanged);
            Res::PlaylistSetItems(pb::Empty {})
        }

        Req::PlaylistSetMode(r) => {
            let mode = pb_playlist_mode_to_queue(r.mode);
            crate::model::repo::playlists::set_mode(&state.db, r.id, mode, tasks::now_ms()).await?;
            application::rebuild_for_playlist(&state, r.id).await;
            state.events.publish(GlobalEvent::PlaylistChanged);
            Res::PlaylistSetMode(pb::Empty {})
        }

        Req::PlaylistSetInterval(r) => {
            crate::model::repo::playlists::set_interval(
                &state.db,
                r.id,
                r.interval_secs,
                tasks::now_ms(),
            )
            .await?;
            application::set_interval_for_playlist(&state, r.id, r.interval_secs).await;
            state.events.publish(GlobalEvent::PlaylistChanged);
            Res::PlaylistSetInterval(pb::Empty {})
        }

        Req::PlaylistActivate(r) => {
            application::activate_playlist(&state, &r.display_ids, r.id).await?;
            if r.auto_attach {
                let id = r.id;
                state.settings.update(|s| {
                    s.global.auto_attach_playlist_id = Some(id);
                });
                state.settings.flush_now().await;
            }
            Res::PlaylistActivate(pb::Empty {})
        }

        Req::PlaylistDeactivate(r) => {
            application::deactivate_playlist(&state, &r.display_ids).await?;
            if r.clear_auto_attach > 0 {
                let id = r.clear_auto_attach;
                state.settings.update(|s| {
                    if s.global.auto_attach_playlist_id == Some(id) {
                        s.global.auto_attach_playlist_id = None;
                    }
                });
                state.settings.flush_now().await;
            }
            Res::PlaylistDeactivate(pb::Empty {})
        }

        Req::PlaylistStatus(_) => {
            let st = state.playlists.status().await;
            let auto_attach_id = state.settings.global().auto_attach_playlist_id.unwrap_or(0);
            Res::PlaylistStatus(pb::PlaylistStatusResponse {
                auto_attach_id,
                displays: st.into_iter().map(playlist_display_status_to_pb).collect(),
            })
        }

        Req::PlaylistJumpTo(r) => {
            application::jump_to_playlist(&state, r.id, &r.entry_id).await?;
            Res::PlaylistJumpTo(pb::Empty {})
        }
    })
}

pub(super) fn parse_entry_ids(v: &[String]) -> Vec<i64> {
    v.iter().filter_map(|s| s.parse::<i64>().ok()).collect()
}

pub(super) fn pb_playlist_mode_to_queue(m: i32) -> crate::playback::Mode {
    match m {
        2 => crate::playback::Mode::Shuffle,
        3 => crate::playback::Mode::Random,
        _ => crate::playback::Mode::Sequential,
    }
}

pub(super) fn queue_mode_to_pb_playlist(m: crate::playback::Mode) -> i32 {
    match m {
        crate::playback::Mode::Sequential => 1,
        crate::playback::Mode::Shuffle => 2,
        crate::playback::Mode::Random => 3,
    }
}
