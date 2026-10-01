use super::*;

/// Output-local presentation values. Sampling never changes authoritative
/// geometry, configure barriers, animation clocks, or another output's view.
pub(crate) struct FrameScene {
    pub windows: HashMap<WindowId, WindowFrame>,
    pub overview: crate::overview::OverviewMotion,
    // Plane selection and scheduling reuse the activity sampled with this scene.
    pub animating: bool,
}

pub(crate) struct WindowFrame {
    pub geometry: WindowGeometry,
    pub rect: Rect,
    pub close_scale: f64,
    pub close_alpha: f32,
    pub focus: f64,
    pub dim: f64,
}

impl Ferese {
    fn output_window_records<'a>(
        &'a self,
        output: &'a Output,
        block_resizes: bool,
    ) -> impl Iterator<Item = (WindowId, &'a super::window_registry::WindowRecord, bool)> + 'a {
        self.output_id(output)
            .into_iter()
            .flat_map(move |output| self.output_workspaces.assigned_workspaces(output))
            .filter_map(move |id| self.workspaces.workspace(id))
            .flat_map(move |workspace| {
                let blocked = block_resizes
                    && workspace
                        .layout
                        .window_ids()
                        .chain(workspace.floating.iter().copied())
                        .any(|id| self.windows.record(id).is_some_and(|record| record.resize.is_some()));

                workspace
                    .layout
                    .window_ids()
                    .chain(workspace.floating.iter().copied())
                    .filter_map(move |id| Some((id, self.windows.record(id)?, blocked)))
            })
    }

    pub(crate) fn output_has_animations(&self, output: &Output) -> bool {
        if self.overview.is_animating(self.spring_config) || !self.dismissing_popups.is_empty() {
            return true;
        }

        if self
            .output_id(output)
            .and_then(|id| self.workspace_slides.get(&id))
            .is_some_and(|slide| slide.held_progress.is_none() && slide.elapsed < WORKSPACE_SLIDE_DURATION)
        {
            return true;
        }

        self.output_window_records(output, false).any(|(id, record, _)| {
            let Some(mut geometry) = record.geometry else {
                return false;
            };

            if !self.window_belongs_to_output(id, output) {
                return false;
            }

            if record.resize.is_some()
                || self.render.snapshot(&id).is_some()
                || record.closing.as_ref().is_some_and(|close| !close.close_sent)
                || record.focus.as_ref().is_some_and(|focus| focus.is_animating())
                || record.dimming.as_ref().is_some_and(|dim| dim.is_animating())
            {
                return true;
            }

            if geometry.advance(Duration::ZERO, self.spring_config, self.animations_enabled) {
                return true;
            }

            let Some(workspace) = self.workspaces.workspace_for_window(id) else {
                return false;
            };

            self.viewport_animations.get(&workspace).is_some_and(|viewport| {
                let mut viewport = *viewport;
                let held = self
                    .focus_swipe
                    .as_ref()
                    .is_some_and(|swipe| swipe.workspace == workspace);
                !held && viewport.advance(Duration::ZERO, self.viewport_spring_config)
            }) || record.coupled_width.as_ref().is_some_and(|(_, width)| {
                let mut width = *width;
                width.advance(Duration::ZERO, self.viewport_spring_config)
            })
        })
    }

    pub(crate) fn sample_frame(&self, output: &Output, horizon: Duration) -> FrameScene {
        let animating = self.output_has_animations(output);
        let delta = if self.animations_enabled && animating {
            horizon.min(Duration::from_millis(100)).mul_f64(self.animation_speed)
        } else {
            Duration::ZERO
        };

        let overview = self.overview.sample(delta, self.spring_config);
        let predicted_slide_offsets =
            (!delta.is_zero()).then(|| self.sample_workspace_slide_offsets(self.output_id(output), delta));
        let slide_offsets = predicted_slide_offsets
            .as_ref()
            .unwrap_or(&self.workspace_slide_offsets);
        let mut windows = HashMap::new();

        for (id, record, blocked) in self.output_window_records(output, !delta.is_zero()) {
            let Some(original) = record.geometry else { continue };

            if !self.window_belongs_to_output(id, output) {
                continue;
            }

            let world = record.world_x.as_ref().and_then(|(workspace, world)| {
                let viewport = self.viewport_animations.get(workspace)?;
                let held = self
                    .focus_swipe
                    .as_ref()
                    .is_some_and(|swipe| swipe.workspace == *workspace);

                Some((*world, *viewport, held))
            });
            let width = record.coupled_width.as_ref().map(|(_, width)| *width);
            let geometry = if delta.is_zero() || blocked {
                original
            } else {
                predict_geometry(
                    original,
                    delta,
                    self.spring_config,
                    self.viewport_spring_config,
                    world,
                    width,
                )
            };

            let mut rect = overview.presented_rect(id, geometry.visual.current);
            if !overview.is_presenting() {
                let (x, y) = self
                    .workspaces
                    .workspace_for_window(id)
                    .and_then(|workspace| slide_offsets.get(&workspace).copied())
                    .unwrap_or_default();
                rect.x += x;
                rect.y += y;
            }

            let mut close = record.closing.as_ref().copied().unwrap_or_default();
            if record.closing.is_some() && !delta.is_zero() {
                close.advance(delta, true);
            }

            let eased = smoothstep(close.progress);
            let focused = if self.overview.is_active() {
                self.overview_selected(id)
            } else {
                self.focused_window == Some(id)
            };

            let sample_dim = |motion: Option<&DimAnimation>, fallback| {
                let Some(mut motion) = motion.cloned() else {
                    return fallback;
                };

                if !delta.is_zero() {
                    motion.predict(delta, self.inactive_dim.duration_ms);
                }

                motion.current
            };

            windows.insert(
                id,
                WindowFrame {
                    geometry,
                    rect,
                    close_scale: 1.0 - eased * 0.02,
                    close_alpha: (1.0 - eased) as f32,
                    focus: sample_dim(record.focus.as_ref(), if focused { 1.0 } else { 0.0 }),
                    dim: sample_dim(record.dimming.as_ref(), 0.0),
                },
            );
        }

        FrameScene {
            windows,
            overview,
            animating,
        }
    }
}

fn predict_geometry(
    original: WindowGeometry,
    delta: Duration,
    spring: SpringConfig,
    viewport_spring: SpringConfig,
    world: Option<(AnimatedValue, AnimatedValue, bool)>,
    width: Option<AnimatedValue>,
) -> WindowGeometry {
    let mut predicted = original;
    let zooming = predicted.is_zooming();
    if width.is_some() {
        predicted.visual.target.width = predicted.visual.current.width;
        predicted.visual.velocity.width = 0.0;
    }

    predicted.advance(delta, spring, true);
    predicted.visual.target.width = original.visual.target.width;
    if let Some((mut world, mut viewport, held)) = world
        && !zooming
    {
        world.advance(delta, spring);
        if !held {
            viewport.advance(delta, viewport_spring);
        }

        predicted.visual.current.x = world.current - viewport.current;
        predicted.visual.velocity.x = world.velocity - viewport.velocity;
    }

    if let Some(mut width) = width {
        width.advance(delta, viewport_spring);
        predicted.visual.current.width = width.current;
        predicted.visual.velocity.width = width.velocity;
    }

    predicted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a private XDG_RUNTIME_DIR and permission to bind test sockets"]
    fn frame_sampling_preserves_compositor_state_between_output_deadlines() {
        let runtime = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        assert!(
            runtime.starts_with(std::env::temp_dir()),
            "use a disposable runtime directory"
        );
        let mut event_loop = EventLoop::try_new().unwrap();
        let display = Display::new().unwrap();
        let config = crate::config::Config::default().runtime_config().unwrap();
        let mut state = Ferese::new(&mut event_loop, display, config).unwrap();
        let output = Output::new(
            "forecast-test".into(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
            },
        );
        output.change_current_state(
            Some(smithay::output::Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            Some(smithay::utils::Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        state.space.map_output(&output, (0, 0));
        state.register_output(&output, "forecast-test".into());
        let id = WindowId(1);
        state.workspaces.insert_window(id, Axis::Horizontal, 0.5).unwrap();
        let mut geometry = WindowGeometry::new(Rect::new(0., 0., 400., 300.), None);
        geometry.visual.set_target(Rect::new(600., 0., 400., 300.));
        // This test isolates frame sampling from the protocol client fixture.
        state.windows.records.insert(
            id,
            super::super::window_registry::WindowRecord {
                geometry: Some(geometry),
                ..Default::default()
            },
        );
        let tick = state.last_animation_tick;
        for horizon in [
            Duration::from_millis(16),
            Duration::from_millis(4),
            Duration::from_millis(8),
        ] {
            {
                let forecast = state.sample_frame(&output, horizon);
                assert!(forecast.windows[&id].geometry.visual.current.x > geometry.visual.current.x);
                assert!(forecast.animating);
            }

            assert_eq!(*state.windows.geometry(&id).unwrap(), geometry);
            assert_eq!(state.last_animation_tick, tick);
        }

        // Sampling one output must not visit or include another output's records.
        let other = Output::new(
            "other-forecast".into(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
            },
        );
        other.change_current_state(
            Some(smithay::output::Mode {
                size: (1280, 720).into(),
                refresh: 240_000,
            }),
            Some(smithay::utils::Transform::Normal),
            None,
            Some((1920, 0).into()),
        );
        state.space.map_output(&other, (1920, 0));
        state.register_output(&other, "other-forecast".into());
        let other_workspace = state
            .output_workspaces
            .active_workspace(state.output_id(&other).unwrap())
            .unwrap();
        let other_id = WindowId(2);
        state
            .workspaces
            .insert_floating_window(other_id, other_workspace, geometry.visual.current, false)
            .unwrap();
        state.windows.records.insert(
            other_id,
            super::super::window_registry::WindowRecord {
                geometry: Some(geometry),
                ..Default::default()
            },
        );

        assert_eq!(
            state
                .output_window_records(&output, false)
                .map(|(id, _, _)| id)
                .collect::<Vec<_>>(),
            vec![id]
        );
        assert_eq!(
            state
                .output_window_records(&other, false)
                .map(|(id, _, _)| id)
                .collect::<Vec<_>>(),
            vec![other_id]
        );
        assert!(
            !state
                .sample_frame(&output, Duration::from_millis(8))
                .windows
                .contains_key(&other_id)
        );
        assert!(
            !state
                .sample_frame(&other, Duration::from_millis(4))
                .windows
                .contains_key(&id)
        );

        let output_id = state.output_id(&output).unwrap();
        let from = state.workspaces.workspace_for_window(id).unwrap();
        let to = state.workspaces.create_workspace();
        state.output_workspaces.assign_workspace(output_id, to).unwrap();
        let mut slide = WorkspaceSlide::new(None, from, to, SwipeDirection::Left);
        slide.held_progress = Some(0.25);
        state.workspace_slides.insert(output_id, slide);
        state.refresh_workspace_slide_offsets();
        let offsets = state.workspace_slide_offsets.clone();
        assert_eq!(offsets[&from].0, -480.0);
        assert_eq!(offsets[&to].0, 1440.0);
        assert_eq!(
            state.sample_frame(&output, Duration::from_millis(8)).windows[&id]
                .rect
                .x,
            state.sample_frame(&output, Duration::from_millis(8)).windows[&id]
                .geometry
                .visual
                .current
                .x
                - 480.0
        );
        assert_eq!(state.workspace_slide_offsets, offsets);
        assert!(
            state
                .sample_workspace_slide_offsets(state.output_id(&other), Duration::from_millis(8))
                .is_empty()
        );
        state.cancel_workspace_slides();
        assert!(state.workspace_slide_offsets.is_empty());

        // A prediction must neither advance nor expire a client resize barrier.
        state.windows.set_transaction(
            id,
            crate::resize_transaction::ResizeTransaction::new(9.into(), Duration::ZERO),
        );
        let blocked = state.sample_frame(&output, Duration::from_millis(16));
        assert_eq!(blocked.windows[&id].geometry, geometry);
        assert!(state.windows.transaction(&id).is_some());
        state.windows.clear_transaction(&id);

        state.animations_enabled = false;
        let forecast = state.sample_frame(&output, Duration::from_millis(16));
        assert_eq!(forecast.windows[&id].geometry, geometry);

        let settled = WindowGeometry::new(Rect::new(0., 0., 400., 300.), None);
        state.windows.set_geometry(id, settled);
        state.animations_enabled = true;
        assert!(!state.sample_frame(&output, Duration::ZERO).animating);
        assert!(state.sample_frame(&other, Duration::ZERO).animating);
    }

    #[test]
    fn mixed_refresh_forecasts_do_not_change_authoritative_geometry_or_client_state() {
        let mut geometry = WindowGeometry::new(Rect::new(0., 0., 400., 300.), None);
        geometry.visual.set_target(Rect::new(600., 0., 500., 300.));
        let original = geometry;
        let sample = |delta| {
            predict_geometry(
                geometry,
                delta,
                SpringConfig::default(),
                SpringConfig::default(),
                None,
                None,
            )
        };

        let fast = sample(Duration::from_nanos(1_000_000_000 / 240));
        let slow = sample(Duration::from_nanos(1_000_000_000 / 60));
        assert!(fast.visual.current.x > 0.);
        assert!(slow.visual.current.x > fast.visual.current.x);
        assert_eq!(sample(Duration::from_nanos(1_000_000_000 / 240)), fast);
        assert_eq!(geometry, original);
        assert_eq!(slow.client, original.client);
        assert_eq!(slow.logical, original.logical);
    }

    #[test]
    fn scrolling_coordinates_and_coupled_width_use_the_same_forecast_time() {
        let geometry = WindowGeometry::new(Rect::new(500., 0., 400., 300.), None);
        let mut world = AnimatedValue::new(500.);
        world.set_target(600.);
        let mut viewport = AnimatedValue::new(0.);
        viewport.set_target(200.);
        let mut width = AnimatedValue::new(400.);
        width.set_target(500.);
        let spring = SpringConfig::default();
        let delta = Duration::from_millis(10);
        let predicted = predict_geometry(
            geometry,
            delta,
            spring,
            spring,
            Some((world, viewport, false)),
            Some(width),
        );
        world.advance(delta, spring);
        viewport.advance(delta, spring);
        width.advance(delta, spring);
        assert_eq!(predicted.visual.current.x, world.current - viewport.current);
        assert_eq!(predicted.visual.current.width, width.current);
        assert_eq!(predicted.visual.target, geometry.visual.target);
    }
}
