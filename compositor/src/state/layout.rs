use super::*;

impl Ferese {
    pub(super) fn visible_workspace_ids(&self) -> HashSet<WorkspaceId> {
        let mut visible = self
            .output_workspaces
            .connected_outputs()
            .filter_map(|output| self.output_workspaces.active_workspace(output))
            .collect::<HashSet<_>>();
        visible.extend(
            self.workspace_slides
                .values()
                .flat_map(|slide| slide.items.iter().map(|item| item.workspace)),
        );
        visible
    }

    pub(super) fn prune_empty_workspaces(&mut self, visible: &HashSet<WorkspaceId>) {
        let mut protected = visible.clone();
        protected.extend(self.output_workspaces.history_workspaces());
        for workspace in self.workspaces.prune_empty(&protected) {
            self.output_workspaces.forget_workspace(workspace);
            self.viewport_animations.remove(&workspace);
        }
    }

    pub(super) fn unmap_invisible_windows(&mut self, visible: &HashSet<WindowId>) -> bool {
        let mut changed = false;
        for (window, id) in &self.window_ids {
            if visible.contains(id) {
                continue;
            }
            let was_mapped = self.space.element_location(window).is_some();
            changed |= was_mapped;
            if was_mapped && let Some(geometry) = self.window_geometry.get_mut(id) {
                geometry.settle_presentation();
            }
            self.space.unmap_elem(window);
        }
        changed
    }

    pub fn relayout(&mut self) {
        self.refresh_input_capture_zones();
        let visible_workspaces = self.visible_workspace_ids();
        self.prune_empty_workspaces(&visible_workspaces);
        if self.session_lock.active {
            self.configure_lock_surfaces();
        }
        // Consume idle time before setting new targets; it must not become a
        // large first animation step after a keypress on an idle desktop.
        self.advance_animations(Instant::now());
        self.arrange_layers();
        let mut previous_scrolling_world_x = std::mem::take(&mut self.scrolling_world_x);
        let pending_column_width_cycles = std::mem::take(&mut self.pending_column_width_cycles);
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();
        let constraints = self.window_constraints();
        let mut visible = HashSet::new();
        let mut placements = Vec::new();
        let mut visible_workspace_pairs = Vec::new();

        for output in outputs {
            let Some(output_id) = self.output_ids.get(&output).copied() else {
                continue;
            };
            let Some(active_workspace) = self.output_workspaces.active_workspace(output_id) else {
                continue;
            };
            visible_workspace_pairs.push((output.clone(), active_workspace));
            if let Some(slide) = self.workspace_slides.get(&output_id) {
                for item in &slide.items {
                    if item.workspace != active_workspace {
                        visible_workspace_pairs.push((output.clone(), item.workspace));
                    }
                }
            }
        }

        for (output, workspace_id) in visible_workspace_pairs {
            let Some(bounds) = self.output_bounds_for(&output) else {
                continue;
            };
            let fullscreen_bounds = self.full_output_bounds_for(&output).unwrap_or(bounds);
            for layer in layer_map_for_output(&output).layers() {
                crate::handlers::set_surface_tree_output(layer.wl_surface(), &output);
            }
            let Some(workspace) = self.workspaces.workspace(workspace_id) else {
                continue;
            };
            let workspace_fullscreen = workspace.fullscreen;
            let is_scrolling_layout = matches!(workspace.layout, WorkspaceLayout::Scrolling(_));
            let workspace_focus = workspace.last_focused;
            let focused = self
                .focused_window
                .filter(|window| self.workspaces.workspace_for_window(*window) == Some(workspace_id))
                .or(workspace_focus);
            let Some(workspace) = self.workspaces.workspace_mut(workspace_id) else {
                continue;
            };
            let layout =
                match workspace
                    .layout
                    .geometry_with_constraints(bounds, self.gap_config, &constraints, focused)
                {
                    Ok(layout) => layout,
                    Err(error) => {
                        tracing::error!(%error, ?workspace_id, "failed to compute tiled geometry");
                        continue;
                    }
                };
            let viewport_target = workspace.layout.viewport_x();
            let viewport_motion = viewport_target.map(|target| {
                let viewport = self
                    .viewport_animations
                    .entry(workspace_id)
                    .or_insert_with(|| AnimatedValue::new(target));
                let target_changed = (viewport.target - target).abs() > 0.001;
                viewport.retarget_preserving_motion(target);
                if !self.animations_enabled {
                    viewport.snap();
                }
                (viewport.current, target_changed)
            });
            let viewport_current = viewport_motion.map(|(current, _)| current);
            let viewport_target_changed = viewport_motion.is_some_and(|(_, changed)| changed);

            for warning in layout.warnings {
                tracing::warn!(
                    window = ?warning.window,
                    kind = ?warning.kind,
                    requested = warning.requested,
                    assigned = warning.assigned,
                    "window size constraint could not be satisfied exactly"
                );
            }

            for (window, id) in &self.window_ids {
                if self.workspaces.workspace_for_window(*id) != Some(workspace_id) {
                    continue;
                }
                if let Some(toplevel) = window.toplevel() {
                    crate::handlers::set_surface_tree_output(toplevel.wl_surface(), &output);
                }

                let is_maximized = self.maximized_windows.contains(id) && workspace_fullscreen != Some(*id);
                let rect = if workspace_fullscreen == Some(*id) {
                    fullscreen_bounds
                } else if is_maximized && !is_scrolling_layout {
                    maximized_rect(bounds, self.gap_config.outer)
                } else {
                    match self.workspaces.placement(*id) {
                        Some(WindowPlacement::Tiled) => {
                            let Some(rect) = layout.geometry.get(id).copied() else {
                                continue;
                            };
                            rect
                        }
                        Some(WindowPlacement::Floating { rect }) => {
                            let constrained =
                                constrained_floating_rect(rect, constraints.get(id).copied().unwrap_or_default());
                            if constrained != rect {
                                let _ = self.workspaces.set_floating_rect(*id, constrained);
                            }
                            constrained
                        }
                        None => continue,
                    }
                };
                let is_fullscreen = workspace_fullscreen == Some(*id);
                let is_floating = matches!(self.workspaces.placement(*id), Some(WindowPlacement::Floating { .. }));

                visible.insert(*id);
                let scrolling = if !is_fullscreen && !is_floating && (!is_maximized || is_scrolling_layout) {
                    viewport_target
                        .zip(viewport_current)
                        .map(|(target, current)| (workspace_id, rect.x + target, current))
                } else {
                    None
                };

                placements.push((
                    window.clone(),
                    *id,
                    rect,
                    is_fullscreen,
                    is_maximized,
                    is_floating,
                    scrolling,
                    pending_column_width_cycles.contains(id) && viewport_target_changed,
                ));
            }
        }

        let mut layout_changed = self.unmap_invisible_windows(&visible);

        let now = self.start_time.elapsed();

        let mut scrolling_world_x = HashMap::new();
        placements.sort_by_key(|(_, id, ..)| self.window_stack.rank(*id));

        for (window, id, rect, is_fullscreen, is_maximized, is_floating, scrolling, couple_width) in placements {
            let was_mapped = self.space.element_location(&window).is_some();
            // A viewport-coupled width must never override fullscreen/floating geometry.
            if scrolling.is_none() {
                self.viewport_coupled_widths.remove(&id);
            }
            let (geometry, had_geometry) = match self.window_geometry.entry(id) {
                Entry::Occupied(entry) => (entry.into_mut(), true),
                Entry::Vacant(entry) => (entry.insert(WindowGeometry::new(rect, client_size(&window))), false),
            };
            let mode = if is_fullscreen {
                PresentationMode::Fullscreen
            } else if is_maximized {
                PresentationMode::Maximized
            } else {
                PresentationMode::Normal
            };
            layout_changed |= !had_geometry || geometry.logical != rect;
            let mut requested_size = geometry.set_presentation_mode(rect, mode, now);
            if !was_mapped && (had_geometry || is_fullscreen) {
                geometry.settle_presentation();
            }
            if !self.animations_enabled {
                geometry.advance(Duration::ZERO, self.spring_config, false);
                requested_size = geometry.presentation_size_request(now).or(requested_size);
            }
            if geometry.is_zooming() {
                self.viewport_coupled_widths.remove(&id);
            }
            if let Some((workspace, world_x, viewport_x)) = scrolling {
                let restored_world_x =
                    restored_scrolling_world_x(had_geometry, geometry.visual.current.x, viewport_x, world_x);
                let mut animated_world_x = previous_scrolling_world_x
                    .remove(&id)
                    .filter(|(previous_workspace, _)| *previous_workspace == workspace)
                    .map(|(_, world_x)| world_x)
                    .unwrap_or_else(|| AnimatedValue::new(restored_world_x));
                animated_world_x.set_target(world_x);
                if !self.animations_enabled {
                    animated_world_x.snap();
                }
                if !geometry.is_zooming() {
                    geometry.visual.current.x = animated_world_x.current - viewport_x;
                    geometry.visual.velocity.x = animated_world_x.velocity;
                }
                scrolling_world_x.insert(id, (workspace, animated_world_x));

                let coupled = !geometry.is_zooming()
                    && (couple_width
                        || self
                            .viewport_coupled_widths
                            .get(&id)
                            .is_some_and(|(previous_workspace, _)| *previous_workspace == workspace));
                if coupled {
                    let width = self
                        .viewport_coupled_widths
                        .entry(id)
                        .or_insert_with(|| (workspace, AnimatedValue::new(geometry.visual.current.width)));
                    if width.0 != workspace {
                        *width = (workspace, AnimatedValue::new(geometry.visual.current.width));
                    }
                    width.1.retarget_preserving_motion(rect.width);
                    if !self.animations_enabled {
                        width.1.snap();
                    }
                    geometry.visual.current.width = width.1.current;
                    geometry.visual.velocity.width = width.1.velocity;
                } else if pending_column_width_cycles.contains(&id) {
                    self.viewport_coupled_widths.remove(&id);
                }
            }
            let visual = geometry.visual.current;
            let natural_pending =
                self.natural_floating_pending.contains(&id) && is_floating && !is_fullscreen && !is_maximized;
            if natural_pending {
                requested_size = None;
            }
            let location = (visual.x.round() as i32, visual.y.round() as i32);

            self.space.map_element(window.clone(), location, false);
            if let Some(toplevel) = window.toplevel() {
                let state_changed = toplevel.with_pending_state(|state| {
                    if natural_pending {
                        state.size = None;
                    } else if let Some(size) = requested_size {
                        state.size = Some((size.width, size.height).into());
                    }

                    let fullscreen_changed = if is_fullscreen {
                        state.states.set(xdg_toplevel::State::Fullscreen)
                    } else {
                        state.states.unset(xdg_toplevel::State::Fullscreen)
                    };

                    let maximized_changed = if is_maximized {
                        state.states.set(xdg_toplevel::State::Maximized)
                    } else {
                        state.states.unset(xdg_toplevel::State::Maximized)
                    };
                    let tiled = !is_floating && !is_fullscreen;
                    let tiled_changed = if tiled {
                        state.states.set(xdg_toplevel::State::TiledLeft)
                            | state.states.set(xdg_toplevel::State::TiledRight)
                            | state.states.set(xdg_toplevel::State::TiledTop)
                            | state.states.set(xdg_toplevel::State::TiledBottom)
                    } else {
                        state.states.unset(xdg_toplevel::State::TiledLeft)
                            | state.states.unset(xdg_toplevel::State::TiledRight)
                            | state.states.unset(xdg_toplevel::State::TiledTop)
                            | state.states.unset(xdg_toplevel::State::TiledBottom)
                    };

                    let decoration_mode = if is_floating && !is_fullscreen && !is_maximized {
                        DecorationMode::ClientSide
                    } else {
                        DecorationMode::ServerSide
                    };
                    let decoration_changed = state.decoration_mode != Some(decoration_mode);
                    state.decoration_mode = Some(decoration_mode);

                    fullscreen_changed || maximized_changed || tiled_changed || decoration_changed
                });

                if (natural_pending || requested_size.is_some() || state_changed)
                    && let Some(serial) = toplevel.send_pending_configure()
                    && requested_size.is_some()
                    && had_geometry
                    && self.animations_enabled
                {
                    self.resize_transactions.insert(
                        id,
                        crate::resize_transaction::ResizeTransaction::new(serial, now)
                            .with_source_geometry(window.geometry()),
                    );
                }
            }
        }
        self.scrolling_world_x = scrolling_world_x;
        if layout_changed {
            self.backdrop_generation = self.backdrop_generation.wrapping_add(1);
        }
        self.sync_window_stacking();
        self.retarget_overview();
        self.send_shell_snapshots();

        crate::backends::direct::render_all(self);
    }

    pub(crate) fn raise_window(&mut self, window: &Window, activate: bool) {
        if let Some(id) = self.window_ids.get(window) {
            self.window_stack.raise(*id);
        }
        self.space.raise_element(window, activate);
        self.sync_window_stacking();
    }

    pub(crate) fn sync_window_stacking(&mut self) {
        let mut windows = self.space.elements().cloned().collect::<Vec<_>>();
        windows.sort_by_key(|window| {
            let id = self.window_ids.get(window).copied();
            let geometry = id.and_then(|id| self.window_geometry.get(&id));
            let floating =
                id.is_some_and(|id| matches!(self.workspaces.placement(id), Some(WindowPlacement::Floating { .. })));
            let priority = crate::stacking::layer_priority(
                floating,
                geometry.is_some_and(|geometry| geometry.is_zooming()),
                geometry.is_some_and(|geometry| geometry.is_fullscreen()),
                floating
                    && id.is_some_and(|id| {
                        self.floating_above_fullscreen.get(&id).is_some_and(|parent| {
                            self.workspaces
                                .workspace_for_window(id)
                                .and_then(|workspace| self.workspaces.workspace(workspace))
                                .is_some_and(|workspace| workspace.fullscreen == Some(*parent))
                        })
                    }),
            );
            (priority, id.map_or(usize::MAX, |id| self.window_stack.rank(id)))
        });
        if stacking_order_settled(self.space.elements(), &windows, |window| window.z_index()) {
            return;
        }
        for window in windows {
            self.space.raise_element(&window, false);
        }
    }

    pub(super) fn tiled_layout(&mut self, bounds: Rect) -> Result<LayoutResult, ferese_layout::LayoutError> {
        let constraints = self.window_constraints();
        let focused = self.focused_window;
        self.workspaces
            .active_mut()
            .layout
            .geometry_with_constraints(bounds, self.gap_config, &constraints, focused)
    }

    pub(super) fn window_constraints(&self) -> HashMap<WindowId, SizeConstraints> {
        self.window_ids
            .iter()
            .filter_map(|(window, id)| {
                let toplevel = window.toplevel()?;
                let (minimum, maximum) = with_states(toplevel.wl_surface(), |states| {
                    let mut cached = states.cached_state.get::<SurfaceCachedState>();
                    let state = cached.current();
                    (state.min_size, state.max_size)
                });

                Some((
                    *id,
                    SizeConstraints {
                        min_width: minimum.w.max(1) as f64,
                        min_height: minimum.h.max(1) as f64,
                        max_width: (maximum.w > 0).then_some(maximum.w as f64),
                        max_height: (maximum.h > 0).then_some(maximum.h as f64),
                    },
                ))
            })
            .collect()
    }

    pub(super) fn arrange_layers(&self) {
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();

        for output in outputs {
            layer_map_for_output(&output).arrange();
        }
    }
}
