use super::*;

/// Rendering samples an isolated forecast. Restore authoritative state before
/// returning to input, client commits, or another monitor's render deadline.
pub(crate) struct RenderForecast<'a> {
    state: &'a mut Ferese,
    geometry: Vec<(WindowId, WindowGeometry)>,
    slides: HashMap<OutputId, WorkspaceSlide>,
    closing: HashMap<WindowId, ClosingAnimation>,
    focus: HashMap<WindowId, crate::dimming::DimAnimation>,
    dimming: HashMap<WindowId, crate::dimming::DimAnimation>,
    overview: Option<crate::overview::OverviewAnimationSnapshot>,
}

impl std::ops::Deref for RenderForecast<'_> {
    type Target = Ferese;

    fn deref(&self) -> &Ferese {
        self.state
    }
}

impl std::ops::DerefMut for RenderForecast<'_> {
    fn deref_mut(&mut self) -> &mut Ferese {
        self.state
    }
}

impl Drop for RenderForecast<'_> {
    fn drop(&mut self) {
        if self.overview.is_none() {
            return;
        }

        for (id, geometry) in self.geometry.drain(..) {
            self.state.window_geometry.insert(id, geometry);
        }

        self.state.workspace_slides = std::mem::take(&mut self.slides);
        self.state.closing_windows = std::mem::take(&mut self.closing);
        self.state.window_focus = std::mem::take(&mut self.focus);
        self.state.window_dimming = std::mem::take(&mut self.dimming);
        if let Some(overview) = self.overview.take() {
            self.state.overview.restore_prediction(overview);
        }
    }
}

impl Ferese {
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

        self.window_geometry.iter().any(|(id, geometry)| {
            if !self.window_belongs_to_output(*id, output) {
                return false;
            }

            if self.resize_transactions.contains_key(id)
                || self.resize_snapshots.contains_key(id)
                || self.closing_windows.get(id).is_some_and(|close| !close.close_sent)
                || self.window_focus.get(id).is_some_and(|focus| focus.is_animating())
                || self.window_dimming.get(id).is_some_and(|dim| dim.is_animating())
            {
                return true;
            }

            let mut geometry = *geometry;
            if geometry.advance(Duration::ZERO, self.spring_config, self.animations_enabled) {
                return true;
            }

            let Some(workspace) = self.workspaces.workspace_for_window(*id) else {
                return false;
            };

            self.viewport_animations.get(&workspace).is_some_and(|viewport| {
                let mut viewport = *viewport;
                let held = self
                    .focus_swipe
                    .as_ref()
                    .is_some_and(|swipe| swipe.workspace == workspace);
                !held && viewport.advance(Duration::ZERO, self.viewport_spring_config)
            }) || self.viewport_coupled_widths.get(id).is_some_and(|(_, width)| {
                let mut width = *width;
                width.advance(Duration::ZERO, self.viewport_spring_config)
            })
        })
    }

    pub(crate) fn forecast_render(&mut self, output: &Output, horizon: Duration) -> RenderForecast<'_> {
        if horizon.is_zero() || !self.animations_enabled || !self.output_has_animations(output) {
            return RenderForecast {
                state: self,
                geometry: Vec::new(),
                slides: HashMap::new(),
                closing: HashMap::new(),
                focus: HashMap::new(),
                dimming: HashMap::new(),
                overview: None,
            };
        }

        let delta = if self.animations_enabled {
            horizon.min(Duration::from_millis(100)).mul_f64(self.animation_speed)
        } else {
            Duration::ZERO
        };

        let geometry = self
            .window_geometry
            .iter()
            .filter(|(id, _)| self.window_belongs_to_output(**id, output))
            .map(|(id, geometry)| (*id, *geometry))
            .collect::<Vec<_>>();
        let slides = self.workspace_slides.clone();
        let closing = self.closing_windows.clone();
        let focus = self.window_focus.clone();
        let dimming = self.window_dimming.clone();
        let overview = self.overview.predict(delta, self.spring_config);
        let blocked = self
            .resize_transactions
            .keys()
            .filter_map(|id| self.workspaces.workspace_for_window(*id))
            .collect::<HashSet<_>>();
        for (id, original) in &geometry {
            let workspace = self.workspaces.workspace_for_window(*id);
            if workspace.is_some_and(|workspace| blocked.contains(&workspace)) {
                continue;
            }

            let world = self.scrolling_world_x.get(id).and_then(|(workspace, world)| {
                let viewport = self.viewport_animations.get(workspace)?;
                let held = self
                    .focus_swipe
                    .as_ref()
                    .is_some_and(|swipe| swipe.workspace == *workspace);
                Some((*world, *viewport, held))
            });

            let width = self.viewport_coupled_widths.get(id).map(|(_, width)| *width);
            let predicted = predict_geometry(
                *original,
                delta,
                self.spring_config,
                self.viewport_spring_config,
                world,
                width,
            );
            self.window_geometry.insert(*id, predicted);
        }

        for slide in self
            .workspace_slides
            .values_mut()
            .filter(|slide| slide.held_progress.is_none())
        {
            slide.advance(delta);
        }

        for close in self.closing_windows.values_mut() {
            close.advance(delta, self.animations_enabled);
        }

        for focus in self.window_focus.values_mut() {
            focus.predict(delta, self.inactive_dim.duration_ms);
        }

        for dim in self.window_dimming.values_mut() {
            dim.predict(delta, self.inactive_dim.duration_ms);
        }

        RenderForecast {
            state: self,
            geometry,
            slides,
            closing,
            focus,
            dimming,
            overview: Some(overview),
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
    fn render_forecast_restores_compositor_state_between_output_deadlines() {
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
        state.window_geometry.insert(id, geometry);
        let tick = state.last_animation_tick;
        for horizon in [
            Duration::from_millis(16),
            Duration::from_millis(4),
            Duration::from_millis(8),
        ] {
            {
                let forecast = state.forecast_render(&output, horizon);
                assert!(forecast.window_geometry[&id].visual.current.x > geometry.visual.current.x);
            }

            assert_eq!(state.window_geometry[&id], geometry);
            assert_eq!(state.last_animation_tick, tick);
        }

        state.animations_enabled = false;
        let forecast = state.forecast_render(&output, Duration::from_millis(16));
        assert_eq!(forecast.window_geometry[&id], geometry);
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
