use super::*;

impl Ferese {
    pub fn advance_animations(&mut self, now: Instant) -> bool {
        let delta = crate::presentation::frame_delta(&mut self.last_animation_tick, now);
        self.advance_animations_by(delta)
    }

    pub fn record_drm_presentation(&mut self, node: DrmNode, crtc: crtc::Handle, time: DrmEventTime, sequence: u32) {
        if let Some(backend) = self.direct_backend.as_mut() {
            backend.record_presentation(node, crtc, time, sequence);
        }
    }

    pub(crate) fn animation_fallback_deadline(&self, interval: std::time::Duration) -> Instant {
        self.last_animation_tick + interval.saturating_mul(2)
    }

    pub(crate) fn reset_animation_clock(&mut self) {
        self.last_animation_tick = Instant::now();
    }

    pub(super) fn advance_animations_by(&mut self, delta: std::time::Duration) -> bool {
        let delta = delta.mul_f64(self.animation_speed);
        if self
            .focus_swipe
            .as_ref()
            .is_some_and(|swipe| !self.focus_swipe_is_current(swipe))
        {
            self.focus_swipe = None;
        }

        let visible_workspaces = if self.viewport_animations.is_empty() {
            HashSet::new()
        } else {
            self.space
                .outputs()
                .filter_map(|output| self.output_ids.get(output))
                .filter_map(|output| self.output_workspaces.active_workspace(*output))
                .collect::<HashSet<_>>()
        };

        let windows = self
            .space
            .elements()
            .filter_map(|window| self.windows.ids().get(window).copied().map(|id| (window.clone(), id)))
            .collect::<Vec<_>>();
        let mut active_animation = false;
        let mut completed_slides = Vec::new();
        for (output, slide) in &mut self.workspace_slides {
            if slide.held_progress.is_some() {
                continue;
            }

            if slide.advance(delta) {
                active_animation = true;
            } else {
                completed_slides.push(*output);
            }
        }

        self.dismissing_popups.retain_mut(|(root, popup, motion)| {
            if !smithay::utils::IsAlive::alive(popup.wl_surface()) {
                return false;
            }

            let duration = if self.animations_enabled { 140.0 } else { 0.0 };
            let active = motion.advance_visual(0.0, delta, duration);
            crate::effects::fade_dismissed_surface(popup.wl_surface(), motion.current);
            active_animation |= active;
            if !active {
                let _ = PopupManager::dismiss_popup(root, popup);
            }
            active
        });

        let dim_settings = self.inactive_dim;
        let duration = if self.animations_enabled {
            dim_settings.duration_ms
        } else {
            0.0
        };

        let mut dim_changed = false;
        // Include hidden workspace windows: overview can present them too.
        // Read selection once so these immutable fields can be borrowed alongside
        // each record without allocating a temporary window-ID vector.
        let selected_window = if self.overview.is_active() {
            self.overview.selected()
        } else {
            self.focused_window
        };

        for (&id, record) in self.windows.records_mut() {
            let selected = selected_window == Some(id);
            let target = if selected { 1.0 } else { 0.0 };
            let focus = record
                .focus
                .get_or_insert_with(|| crate::dimming::DimAnimation::new(target));
            let previous = focus.current;
            active_animation |= focus.advance_visual(target, delta, duration);
            dim_changed |= previous != focus.current;
        }

        for (_, id) in &windows {
            let target = crate::dimming::target(dim_settings, self.focused_window, *id, self.overview.is_presenting());
            let Some(record) = self.windows.record_mut(*id) else {
                continue;
            };

            let dim = record
                .dimming
                .get_or_insert_with(|| crate::dimming::DimAnimation::new(target));
            let previous = dim.current;
            active_animation |= dim.advance_visual(target, delta, duration);
            dim_changed |= previous != dim.current;
        }

        let mut ready_to_close = Vec::new();
        for (id, record) in self.windows.records_mut() {
            let Some(animation) = &mut record.closing else { continue };

            if animation.advance(delta, self.animations_enabled) {
                ready_to_close.push(*id);
            } else if !animation.close_sent {
                active_animation = true;
            }
        }

        for id in ready_to_close {
            self.send_window_close(id);
        }

        let now = self.start_time.elapsed();
        self.windows.expire_transactions(now);

        let blocked_workspaces = self
            .windows
            .resizing()
            .filter_map(|id| self.workspaces.workspace_for_window(*id))
            .collect::<HashSet<_>>();
        let animations_enabled = self.animations_enabled;
        let animation_speed = self.animation_speed;
        self.render.retain_snapshots(|id, snapshot| {
            if !animations_enabled {
                return false;
            }

            let waiting_for_client = self
                .workspaces
                .workspace_for_window(*id)
                .is_some_and(|workspace| blocked_workspaces.contains(&workspace));
            // A shrinking client's destination buffer arrives before the
            // animated bounds reach it. Keep the old native pixels covering
            // that strip rather than fading them into the neutral resize fill.
            let uncovered = self.windows.geometry(id).is_some_and(|geometry| {
                geometry.client.committed_size.is_some_and(|size| {
                    crate::presentation::resize_needs_old_frame(
                        geometry.visual.current,
                        geometry.logical,
                        size.width,
                        size.height,
                    )
                })
            });
            let blocked = waiting_for_client || uncovered;
            let active = crate::presentation::advance_handoff(
                &mut snapshot.elapsed,
                &mut snapshot.last_tick,
                now,
                blocked,
                animation_speed,
            );
            if !blocked {
                snapshot.commit.increment();
            }

            if !active {
                tracing::debug!(?id, bytes = snapshot.bytes(), "released resize handoff snapshot");
            }
            active
        });
        active_animation |= self.render.snapshots().next().is_some();
        // Keep scheduling frames while waiting, so the deadline cannot stall.
        active_animation |= !blocked_workspaces.is_empty();
        for (workspace, viewport) in &mut self.viewport_animations {
            if !visible_workspaces.contains(workspace) {
                continue;
            }

            if blocked_workspaces.contains(workspace) {
                continue;
            }

            if let Some(swipe) = self.focus_swipe.as_ref().filter(|swipe| swipe.workspace == *workspace) {
                viewport.current = swipe.position();
                viewport.velocity = 0.0;
            } else if self.animations_enabled {
                active_animation |= viewport.advance(delta, self.viewport_spring_config);
            } else {
                viewport.snap();
            }
        }

        let mut settled_coupled_widths = Vec::new();
        for (window, id) in windows {
            if self
                .workspaces
                .workspace_for_window(id)
                .is_some_and(|workspace| blocked_workspaces.contains(&workspace))
            {
                continue;
            }

            let Some(record) = self.windows.record_mut(id) else {
                continue;
            };

            let natural_pending = record.natural_floating_pending;
            let Some(geometry) = record.geometry.as_mut() else {
                continue;
            };

            let zooming = geometry.is_zooming();

            let coupled_target = record.coupled_width.as_ref().map(|(_, width)| width.target);
            if coupled_target.is_some() {
                geometry.visual.target.width = geometry.visual.current.width;
                geometry.visual.velocity.width = 0.0;
            }
            active_animation |= geometry.advance(delta, self.spring_config, self.animations_enabled);
            if let Some(target) = coupled_target {
                geometry.visual.target.width = target;
            }

            if let Some((workspace, world_x)) = record.world_x.as_mut()
                && let Some(viewport) = self.viewport_animations.get(workspace)
            {
                if zooming {
                    world_x.current = geometry.visual.current.x + viewport.current;
                    world_x.velocity = geometry.visual.velocity.x + viewport.velocity;
                } else if self.animations_enabled {
                    active_animation |= world_x.advance(delta, self.spring_config);
                } else {
                    world_x.snap();
                }

                if !zooming {
                    geometry.visual.current.x = world_x.current - viewport.current;
                    geometry.visual.velocity.x = world_x.velocity - viewport.velocity;
                }
            }

            if let Some((_, width)) = record.coupled_width.as_mut() {
                let width_active = if self.animations_enabled {
                    width.advance(delta, self.viewport_spring_config)
                } else {
                    width.snap();
                    false
                };
                active_animation |= width_active;
                geometry.visual.current.width = width.current;
                geometry.visual.velocity.width = width.velocity;
                if !width_active {
                    settled_coupled_widths.push(id);
                }
            }

            if let Some(size) = geometry.client.expire_wait(now) {
                tracing::warn!(
                    ?id,
                    configured_width = size.width,
                    configured_height = size.height,
                    committed = ?geometry.client.committed_size,
                    "client did not commit the final configured size within 500 ms"
                );
            }

            if let Some(size) = geometry.presentation_size_request(now)
                && !natural_pending
                && let Some(toplevel) = window.toplevel()
            {
                toplevel.with_pending_state(|state| {
                    state.size = Some((size.width, size.height).into());
                });
                toplevel.send_pending_configure();
            }

            let visual = geometry.visual.current;
            self.space
                .map_element(window, (visual.x.round() as i32, visual.y.round() as i32), false);
        }

        for id in settled_coupled_widths {
            self.windows.update(id, |record| record.coupled_width = None);
        }

        if !completed_slides.is_empty() {
            for output in completed_slides {
                self.workspace_slides.remove(&output);
            }

            let visible = self.visible_workspace_ids();
            let visible_windows = self
                .windows
                .ids()
                .values()
                .copied()
                .filter(|id| {
                    self.workspaces
                        .workspace_for_window(*id)
                        .is_none_or(|workspace| visible.contains(&workspace))
                })
                .collect::<HashSet<_>>();
            self.unmap_invisible_windows(&visible_windows);
            self.prune_empty_workspaces(&visible);
            self.send_shell_snapshots();
            active_animation = true;
        }
        active_animation |= self
            .overview
            .advance(delta, self.spring_config, self.animations_enabled);
        self.sync_window_stacking();

        if active_animation || dim_changed {
            self.backdrop_generation = self.backdrop_generation.wrapping_add(1);
        }
        active_animation
    }

    pub(crate) fn animations_enabled(&self) -> bool {
        self.animations_enabled
    }

    pub(crate) fn animation_duration(&self, duration: Duration) -> Duration {
        scaled_animation_duration(duration, self.animations_enabled, self.animation_speed)
    }

    pub(crate) fn workspace_slide_offset(&self, window: WindowId) -> (f64, f64) {
        self.workspace_slide_offset_at(window, Duration::ZERO)
    }

    pub(super) fn workspace_slide_offset_at(&self, window: WindowId, delta: Duration) -> (f64, f64) {
        let Some(workspace) = self.workspaces.workspace_for_window(window) else {
            return (0.0, 0.0);
        };

        let Some(output_id) = self
            .workspace_slides
            .iter()
            .find_map(|(output, slide)| slide.contains(workspace).then_some(*output))
            .or_else(|| self.output_workspaces.output_for_workspace(workspace))
        else {
            return (0.0, 0.0);
        };

        let Some(slide) = self.workspace_slides.get(&output_id) else {
            return (0.0, 0.0);
        };

        let Some(size) = self
            .output_ids
            .iter()
            .find(|(_, id)| **id == output_id)
            .and_then(|(output, _)| self.space.output_geometry(output))
            .map(|geometry| geometry.size)
        else {
            return (0.0, 0.0);
        };

        let mut slide = slide.clone();
        if slide.held_progress.is_none() && !delta.is_zero() {
            slide.advance(delta);
        }

        slide.offset(workspace, f64::from(size.w), f64::from(size.h))
    }

    pub(crate) fn cancel_workspace_slides(&mut self) -> bool {
        let active = !self.workspace_slides.is_empty();
        self.workspace_slides.clear();
        active
    }
}
