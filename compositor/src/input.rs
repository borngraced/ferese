use smithay::reexports::wayland_server::Resource;
use std::process::Command;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent,
        GestureEndEvent, GestureSwipeUpdateEvent as _, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent, Switch,
        SwitchState, SwitchToggleEvent, TouchEvent,
    },
    input::{
        keyboard::{FilterResult, keysyms},
        pointer::{
            AxisFrame, ButtonEvent, Focus, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerHandle,
            RelativeMotionEvent,
        },
        touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as XdgResizeEdge,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
    wayland::{
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        pointer_constraints::{PointerConstraint, with_pointer_constraint},
    },
};

use crate::{
    Ferese,
    config::BindingAction,
    grabs::{MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab},
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

impl Ferese {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);

        if self.session_lock.active {
            // An already-bound IME or drag client may install a grab after the
            // lock request. Never let that grab receive subsequent lock input.
            if let Some(keyboard) = seat.get_keyboard() {
                keyboard.unset_grab(self);
            }
            if let Some(pointer) = seat.get_pointer() {
                pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), 0);
            }
            if let Some(touch) = seat.get_touch() {
                touch.unset_grab(self);
            }
            self.focus_lock_surface();
        }

        match event {
            InputEvent::GestureSwipeBegin { event } => {
                let pointer = seat.get_pointer().expect("seat has a pointer");
                self.swipe.begin(
                    event.fingers(),
                    self.swipe_navigation_blocked()
                        || !self
                            .bindings
                            .iter()
                            .any(|binding| binding.swipe_fingers(event.fingers())),
                    self.input_settings.touchpad.swipe_threshold,
                );
                if self.swipe.active() {
                    self.focus_output_at(pointer.current_location());
                } else {
                    pointer.gesture_swipe_begin(
                        self,
                        &GestureSwipeBeginEvent {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time() as u32,
                            fingers: event.fingers(),
                        },
                    );
                }
            }
            InputEvent::GestureSwipeUpdate { event } => {
                if self.swipe.active() {
                    self.swipe.update(event.delta_x(), event.delta_y());
                } else {
                    seat.get_pointer()
                        .expect("seat has a pointer")
                        .gesture_swipe_update(
                            self,
                            &GestureSwipeUpdateEvent {
                                time: event.time() as u32,
                                delta: event.delta(),
                            },
                        );
                }
            }
            InputEvent::GestureSwipeEnd { event } => {
                if self.swipe.active() {
                    let fingers = self.swipe.fingers();
                    let cancelled = event.cancelled() || self.swipe_navigation_blocked();
                    if let Some(direction) = self.swipe.finish(cancelled)
                        && let Some(action) = self
                            .bindings
                            .iter()
                            .find(|binding| binding.matches_swipe(fingers, direction))
                            .map(|binding| binding.action.clone())
                    {
                        self.execute_binding(action);
                    }
                } else {
                    seat.get_pointer()
                        .expect("seat has a pointer")
                        .gesture_swipe_end(
                            self,
                            &GestureSwipeEndEvent {
                                serial: SERIAL_COUNTER.next_serial(),
                                time: event.time() as u32,
                                cancelled: event.cancelled(),
                            },
                        );
                }
            }
            InputEvent::SwitchToggle { event } if event.switch() == Some(Switch::Lid) => {
                crate::backends::direct::set_lid_closed(self, event.state() == SwitchState::On);
            }
            InputEvent::Keyboard { event, .. } => {
                let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");
                let keycode = event.key_code();
                let state = event.state();

                keyboard.input::<(), _>(
                    self,
                    keycode,
                    state,
                    SERIAL_COUNTER.next_serial(),
                    Event::time(&event) as u32,
                    |data, modifiers, keysym| {
                        if data.session_lock.active {
                            return FilterResult::Forward;
                        }
                        if state == KeyState::Released && data.intercepted_keys.remove(&keycode) {
                            return FilterResult::Intercept(());
                        }

                        let symbol = keysym.modified_sym().raw();
                        if data.overview.is_active()
                            && matches!(symbol, keysyms::KEY_Return | keysyms::KEY_KP_Enter)
                        {
                            if state == KeyState::Pressed {
                                data.intercepted_keys.insert(keycode);
                                if let Some(id) = data.overview.selected() {
                                    data.select_overview_window(id);
                                }
                            }
                            return FilterResult::Intercept(());
                        }
                        if overview_escape(symbol, data.overview.is_active()) {
                            if state == KeyState::Pressed {
                                data.intercepted_keys.insert(keycode);
                                data.set_overview_active(false);
                            }

                            return FilterResult::Intercept(());
                        }
                        if emergency_shortcut_escape(symbol, modifiers.ctrl, modifiers.alt) {
                            if state == KeyState::Pressed {
                                data.intercepted_keys.insert(keycode);

                                if let Some(inhibitor) = data.active_shortcuts_inhibitor.take() {
                                    inhibitor.inactivate();
                                }
                            }

                            return FilterResult::Intercept(());
                        }

                        if data.seat.keyboard_shortcuts_inhibited() {
                            return FilterResult::Forward;
                        }

                        if let Some(vt) = virtual_terminal(
                            symbol,
                            modifiers.ctrl,
                            modifiers.alt,
                            data.direct_backend.is_some(),
                        ) {
                            if state == KeyState::Pressed {
                                data.intercepted_keys.insert(keycode);
                                crate::backends::direct::switch_vt(data, vt);
                            }

                            return FilterResult::Intercept(());
                        }

                        let raw_symbols = keysym
                            .raw_syms()
                            .into_iter()
                            .map(|symbol| symbol.raw())
                            .collect::<Vec<_>>();
                        let action = data.bindings.iter().find_map(|binding| {
                            binding
                                .matches(
                                    keycode,
                                    &raw_symbols,
                                    modifiers.logo,
                                    modifiers.ctrl,
                                    modifiers.alt,
                                    modifiers.shift,
                                )
                                .then(|| binding.action.clone())
                        });
                        let Some(action) = action else {
                            return FilterResult::Forward;
                        };

                        if state == KeyState::Pressed {
                            data.intercepted_keys.insert(keycode);
                            data.execute_binding(action);
                        }

                        FilterResult::Intercept(())
                    },
                );
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(position) = self.absolute_event_position(&event) else {
                    return;
                };
                let pointer = self.seat.get_pointer().expect("seat has a pointer");
                let position = self.constrain_pointer_position(&pointer, position);

                pointer.motion(
                    self,
                    self.surface_under(position),
                    &MotionEvent {
                        location: position,
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time() as u32,
                    },
                );
                pointer.frame(self);
                self.focus_window_under_pointer(&pointer, position);
                self.activate_focused_pointer_constraint(&pointer);
                crate::backends::direct::render_all(self);
            }
            InputEvent::PointerMotion { event, .. } => {
                let pointer = self.seat.get_pointer().expect("seat has a pointer");
                let position = pointer.current_location();
                let focus = self.surface_under(position);
                let (scale_x, scale_y) = if self.overview.is_presenting() {
                    self.window_under_visual(position)
                        .and_then(|window| self.visual_scale_for_window(&window))
                        .unwrap_or((1.0, 1.0))
                } else {
                    (1.0, 1.0)
                };
                let delta = event.delta();
                let delta_unaccel = event.delta_unaccel();

                pointer.relative_motion(
                    self,
                    focus,
                    &RelativeMotionEvent {
                        delta: (delta.x / scale_x, delta.y / scale_y).into(),
                        delta_unaccel: (delta_unaccel.x / scale_x, delta_unaccel.y / scale_y)
                            .into(),
                        utime: (event.time_msec() as u64).saturating_mul(1_000),
                    },
                );
                let requested = self.clamp_pointer_position(position + delta);
                let location = self.constrain_pointer_position(&pointer, requested);

                pointer.motion(
                    self,
                    self.surface_under(location),
                    &MotionEvent {
                        location,
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time() as u32,
                    },
                );
                pointer.frame(self);
                self.focus_window_under_pointer(&pointer, location);
                self.activate_focused_pointer_constraint(&pointer);
                crate::backends::direct::render_all(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().expect("seat has a pointer");
                let serial = SERIAL_COUNTER.next_serial();

                // Native popup_done destroys Iced's window immediately. For
                // effects-capable shell popups, release input now but defer
                // popup_done until the compositor's whole-surface fade ends.
                if event.state() == ButtonState::Pressed
                    && pointer.is_grabbed()
                    && let Some(surface) = self.seat.get_keyboard().and_then(|k| k.current_focus())
                    && let Some(popup) = self.popups.find_popup(&surface)
                    && !self
                        .surface_under(pointer.current_location())
                        .is_some_and(|(target, _)| surface.id().same_client_as(&target.id()))
                    && let Ok(root) = smithay::desktop::find_popup_root_surface(&popup)
                    && crate::effects::begin_surface_dismiss(&surface)
                {
                    let opacity = crate::effects::surface_opacity(&surface);
                    self.dismissing_popups.push((
                        root,
                        popup,
                        crate::dimming::DimAnimation::new(f64::from(opacity)),
                    ));
                    pointer.unset_grab(self, serial, event.time() as u32);
                    self.focus_window_at(pointer.current_location(), serial, true);
                    crate::backends::direct::render_all(self);
                }

                if self.overview.is_active() {
                    let position = pointer.current_location();
                    if self.layer_under(position).is_some() {
                        pointer.button(
                            self,
                            &ButtonEvent {
                                button: event.button_code(),
                                state: event.state(),
                                serial,
                                time: event.time() as u32,
                            },
                        );
                        pointer.frame(self);
                        return;
                    }

                    if event.state() == ButtonState::Pressed {
                        if self.click_overview_workspace(position) {
                            return;
                        }
                        if let Some(window) = self.window_under_visual(position)
                            && let Some(id) = self.window_ids.get(&window).copied()
                        {
                            self.select_overview_window(id);
                        } else {
                            self.set_overview_active(false);
                        }
                    }

                    return;
                }

                if event.state() == ButtonState::Pressed
                    && self.start_floating_pointer_grab(&pointer, event.button_code(), serial)
                {
                    pointer.button(
                        self,
                        &ButtonEvent {
                            button: event.button_code(),
                            state: event.state(),
                            serial,
                            time: event.time() as u32,
                        },
                    );
                    pointer.frame(self);
                    return;
                }

                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
                    self.focus_window_at(pointer.current_location(), serial, true);
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button: event.button_code(),
                        state: event.state(),
                        serial,
                        time: event.time() as u32,
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let horizontal = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0
                });
                let vertical = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0
                });
                let pointer = self.seat.get_pointer().expect("seat has a pointer");
                if self.scroll_overview_strip(
                    pointer.current_location(),
                    if horizontal != 0.0 {
                        horizontal
                    } else {
                        vertical
                    },
                ) {
                    return;
                }
                let mut frame = AxisFrame::new(event.time() as u32).source(source);
                if horizontal != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal);
                    if let Some(value) = event.amount_v120(Axis::Horizontal) {
                        frame = frame.v120(Axis::Horizontal, value as i32);
                    }
                }

                if vertical != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical);
                    if let Some(value) = event.amount_v120(Axis::Vertical) {
                        frame = frame.v120(Axis::Vertical, value as i32);
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let pointer = self.seat.get_pointer().expect("seat has a pointer");

                pointer.axis(self, frame);
                pointer.frame(self);
            }
            InputEvent::TouchDown { event, .. } => {
                let Some(location) = self.absolute_event_position(&event) else {
                    return;
                };
                self.focus_window_at(location, SERIAL_COUNTER.next_serial(), true);
                let touch = self.seat.get_touch().expect("seat has touch capability");
                let serial = SERIAL_COUNTER.next_serial();

                touch.down(
                    self,
                    self.surface_under(location),
                    &DownEvent {
                        slot: event.slot(),
                        location,
                        serial,
                        time: event.time() as u32,
                    },
                );
            }
            InputEvent::TouchMotion { event, .. } => {
                let Some(location) = self.absolute_event_position(&event) else {
                    return;
                };
                let touch = self.seat.get_touch().expect("seat has touch capability");

                touch.motion(
                    self,
                    self.surface_under(location),
                    &TouchMotionEvent {
                        slot: event.slot(),
                        location,
                        time: event.time() as u32,
                    },
                );
            }
            InputEvent::TouchUp { event, .. } => {
                let touch = self.seat.get_touch().expect("seat has touch capability");
                touch.up(
                    self,
                    &UpEvent {
                        slot: event.slot(),
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time() as u32,
                    },
                );
            }
            InputEvent::TouchCancel { .. } => {
                let touch = self.seat.get_touch().expect("seat has touch capability");
                touch.cancel(self);
            }
            InputEvent::TouchFrame { .. } => {
                let touch = self.seat.get_touch().expect("seat has touch capability");
                touch.frame(self);
            }
            _ => {}
        }
    }

    fn start_floating_pointer_grab(
        &mut self,
        pointer: &PointerHandle<Self>,
        button: u32,
        serial: Serial,
    ) -> bool {
        if self.session_lock.active
            || pointer.is_grabbed()
            || !matches!(button, BTN_LEFT | BTN_RIGHT)
            || !self
                .seat
                .get_keyboard()
                .is_some_and(|keyboard| keyboard.modifier_state().logo)
        {
            return false;
        }

        let location = pointer.current_location();
        if self.layer_under(location).is_some() {
            return false;
        }
        let Some(window) = self.window_under_visual(location) else {
            return false;
        };
        if !self.is_floating_window(&window) {
            return false;
        }
        let Some(rect) = self.visual_rect_for_window(&window) else {
            return false;
        };

        self.focus_window_at(location, serial, true);
        let start_data = GrabStartData {
            focus: None,
            button,
            location,
        };

        if button == BTN_LEFT {
            pointer.set_grab(
                self,
                MoveSurfaceGrab {
                    start_data,
                    window,
                    initial_location: rect.loc,
                    initial_size: rect.size,
                    finished: false,
                },
                serial,
                Focus::Clear,
            );
        } else {
            let horizontal_right =
                location.x >= f64::from(rect.loc.x) + f64::from(rect.size.w) / 2.0;
            let vertical_bottom =
                location.y >= f64::from(rect.loc.y) + f64::from(rect.size.h) / 2.0;
            let edge = match (horizontal_right, vertical_bottom) {
                (false, false) => XdgResizeEdge::TopLeft,
                (true, false) => XdgResizeEdge::TopRight,
                (false, true) => XdgResizeEdge::BottomLeft,
                (true, true) => XdgResizeEdge::BottomRight,
            };
            pointer.set_grab(
                self,
                ResizeSurfaceGrab::new(start_data, window, ResizeEdge::from(edge), rect),
                serial,
                Focus::Clear,
            );
        }

        true
    }

    fn absolute_event_position<I, E>(&self, event: &E) -> Option<Point<f64, Logical>>
    where
        I: InputBackend,
        E: AbsolutePositionEvent<I>,
    {
        let output = self
            .focused_output()
            .or_else(|| self.space.outputs().next())?;
        let geometry = self.space.output_geometry(output)?;

        Some(event.position_transformed(geometry.size) + geometry.loc.to_f64())
    }

    fn swipe_navigation_blocked(&self) -> bool {
        self.session_lock.active
            || self.active_shortcuts_inhibitor.is_some()
            || self
                .seat
                .get_pointer()
                .is_some_and(|pointer| pointer.is_grabbed())
            || self
                .seat
                .get_keyboard()
                .is_some_and(|keyboard| keyboard.is_grabbed())
    }

    fn switch_relative_workspace(&mut self, next: bool) {
        let Some(output) = self.output_workspaces.focused_output() else {
            return;
        };
        let Some(current) = self.output_workspaces.active_workspace(output) else {
            return;
        };
        let mut candidates = self
            .workspaces
            .iter()
            .filter(|workspace| {
                self.output_workspaces
                    .output_for_workspace(workspace.id)
                    .is_none_or(|owner| owner == output)
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|workspace| {
            (
                workspace.name.parse::<u32>().unwrap_or(u32::MAX),
                workspace.id.0,
            )
        });
        let Some(index) = candidates
            .iter()
            .position(|workspace| workspace.id == current)
        else {
            return;
        };
        let index = if next {
            index.checked_add(1)
        } else {
            index.checked_sub(1)
        };
        if let Some(workspace) = index
            .and_then(|index| candidates.get(index))
            .map(|workspace| workspace.id)
        {
            self.activate_managed_workspace(workspace);
        } else if next
            && self.workspaces.workspace(current).is_some_and(|workspace| {
                workspace.layout.window_ids().next().is_some() || !workspace.floating.is_empty()
            })
        {
            let next_number = self
                .workspaces
                .iter()
                .filter_map(|workspace| workspace.name.parse::<u32>().ok())
                .max()
                .unwrap_or(1)
                .checked_add(1);
            if let Some(number) = next_number {
                self.switch_workspace(number);
            }
        }
    }

    fn focus_window_at(&mut self, position: Point<f64, Logical>, serial: Serial, raise: bool) {
        if self.session_lock.active {
            self.focus_output_at(position);
            self.focus_lock_surface();
            return;
        }
        let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");
        self.focus_output_at(position);

        if let Some((layer, _, _)) = self.layer_under(position) {
            if layer.can_receive_keyboard_focus() {
                keyboard.set_focus(self, Some(layer.wl_surface().clone()), serial);
            }
            return;
        }

        if let Some(window) = self.window_under_visual(position) {
            let focused = self.window_ids.get(&window).copied();
            if let Some(focused) = focused {
                let result = if raise {
                    self.workspaces.focus_window(focused)
                } else {
                    self.workspaces.focus_window_without_reveal(focused)
                };
                if let Err(error) = result {
                    tracing::error!(%error, ?focused, "failed to update workspace focus");
                    return;
                }
            }
            self.focused_window = focused;
            if raise {
                self.raise_window(&window, true);
            } else {
                // Hover transfers keyboard focus without changing the persistent
                // stack: an exposed window must not cover the floats above it.
                for mapped in self.space.elements() {
                    mapped.set_activated(mapped == &window);
                }
            }
            let surface = window
                .toplevel()
                .expect("mapped window has a toplevel")
                .wl_surface()
                .clone();
            keyboard.set_focus(self, Some(surface), serial);
        } else {
            self.focused_window = None;
            keyboard.set_focus(self, Option::<WlSurface>::None, serial);
        }

        self.space.elements().for_each(|window| {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        });

        if raise {
            self.relayout();
        } else {
            // Focus, dimming and the bar title update without moving the layout.
            self.send_shell_snapshots();
            crate::backends::direct::render_all(self);
        }
    }

    fn focus_window_under_pointer(
        &mut self,
        pointer: &PointerHandle<Self>,
        position: Point<f64, Logical>,
    ) {
        if self.session_lock.active {
            return;
        }
        if self.overview.is_active() {
            self.hover_overview_window(position);
            return;
        }
        // Keep the explicitly selected window focused until Overview's exit
        // animation settles. Moving previews must not steal focus on pointer jitter.
        if self.overview.is_presenting()
            || !self.input_settings.focus_follows_mouse
            || pointer.is_grabbed()
        {
            return;
        }
        if self.layer_under(position).is_some() {
            return;
        }

        let Some(window) = self.window_under_visual(position) else {
            return;
        };
        if self.window_ids.get(&window).copied() == self.focused_window {
            return;
        }

        self.focus_window_at(position, SERIAL_COUNTER.next_serial(), false);
    }

    fn clamp_pointer_position(&self, requested: Point<f64, Logical>) -> Point<f64, Logical> {
        self.space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .map(|geometry| {
                let position = Point::from((
                    requested.x.clamp(
                        f64::from(geometry.loc.x),
                        f64::from(geometry.loc.x + geometry.size.w - 1),
                    ),
                    requested.y.clamp(
                        f64::from(geometry.loc.y),
                        f64::from(geometry.loc.y + geometry.size.h - 1),
                    ),
                ));
                let x = position.x - requested.x;
                let y = position.y - requested.y;
                (x * x + y * y, position)
            })
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map(|(_, position)| position)
            .unwrap_or(requested)
    }

    fn constrain_pointer_position(
        &self,
        pointer: &PointerHandle<Self>,
        requested: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        if self.session_lock.active {
            return requested;
        }
        let current = pointer.current_location();
        let Some(focused_surface) = pointer.current_focus() else {
            return requested;
        };
        let Some((surface, origin)) = self.surface_under(current) else {
            return requested;
        };
        if surface != focused_surface {
            return requested;
        }

        with_pointer_constraint(&surface, pointer, |constraint| {
            let Some(constraint) = constraint.filter(|constraint| constraint.is_active()) else {
                return requested;
            };

            match &*constraint {
                PointerConstraint::Locked(_) => current,
                PointerConstraint::Confined(_) => {
                    let remains_on_surface = self
                        .surface_under(requested)
                        .is_some_and(|(candidate, _)| candidate == surface);
                    let inside_region = constraint
                        .region()
                        .is_none_or(|region| region.contains((requested - origin).to_i32_round()));

                    if remains_on_surface && inside_region {
                        requested
                    } else {
                        current
                    }
                }
            }
        })
    }

    pub fn activate_focused_pointer_constraint(&self, pointer: &PointerHandle<Self>) {
        if self.session_lock.active {
            return;
        }
        let position = pointer.current_location();
        let Some((surface, origin)) = self.surface_under(position) else {
            return;
        };
        if pointer.current_focus().as_ref() != Some(&surface) {
            return;
        }

        with_pointer_constraint(&surface, pointer, |constraint| {
            let Some(constraint) = constraint else {
                return;
            };
            let inside_region = constraint
                .region()
                .is_none_or(|region| region.contains((position - origin).to_i32_round()));

            if !constraint.is_active() && inside_region {
                constraint.activate();
            }
        });
    }

    fn execute_binding(&mut self, action: BindingAction) {
        match action {
            BindingAction::None => {}
            BindingAction::Spawn(mut argv) => {
                let program = argv.remove(0);

                match Command::new(&program)
                    .args(argv)
                    .env("WAYLAND_DISPLAY", &self.socket_name)
                    .env_remove("WAYLAND_SOCKET")
                    .spawn()
                {
                    Ok(child) => {
                        tracing::info!(%program, pid = child.id(), "spawned binding command")
                    }
                    Err(error) => {
                        tracing::warn!(%program, %error, "failed to spawn binding command")
                    }
                }
            }
            BindingAction::Close => self.close_focused_window(),
            BindingAction::Exit => self.loop_signal.stop(),
            BindingAction::Focus(direction) => self.focus_direction(direction),
            BindingAction::Move(direction) => self.move_direction(direction),
            BindingAction::Resize(direction) => self.resize_direction(direction),
            BindingAction::SwitchRelativeWorkspace(next) => self.switch_relative_workspace(next),
            BindingAction::SwitchWorkspace(workspace) => {
                self.switch_workspace(u32::from(workspace));
            }
            BindingAction::MoveToWorkspace(workspace) => {
                self.move_focused_to_workspace(u32::from(workspace));
            }
            BindingAction::ToggleFullscreen => self.toggle_focused_fullscreen(),
            BindingAction::ToggleMaximized => self.toggle_focused_maximized(),
            BindingAction::ToggleLayout => self.toggle_layout_mode(),
            BindingAction::CycleColumnWidth => self.cycle_focused_column_width(),
            BindingAction::CenterColumn => self.center_focused_column(),
            BindingAction::Consume => self.consume_focused_window(),
            BindingAction::Expel => self.expel_focused_window(),
            BindingAction::ToggleFloating => self.toggle_focused_floating(),
            BindingAction::ToggleOverview => self.toggle_overview(),
        }
    }
}

fn virtual_terminal(symbol: u32, ctrl: bool, alt: bool, direct: bool) -> Option<i32> {
    if !direct {
        return None;
    }

    if (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12).contains(&symbol) {
        return Some((symbol - keysyms::KEY_XF86Switch_VT_1 + 1) as i32);
    }

    (ctrl && alt && (keysyms::KEY_F1..=keysyms::KEY_F12).contains(&symbol))
        .then_some((symbol - keysyms::KEY_F1 + 1) as i32)
}

fn emergency_shortcut_escape(symbol: u32, ctrl: bool, alt: bool) -> bool {
    ctrl && alt && symbol == keysyms::KEY_Escape
}

fn overview_escape(symbol: u32, overview_active: bool) -> bool {
    overview_active && symbol == keysyms::KEY_Escape
}

#[cfg(test)]
mod tests {
    use super::{emergency_shortcut_escape, overview_escape, virtual_terminal};

    #[test]
    fn escape_closes_only_an_active_overview() {
        assert!(overview_escape(keysyms::KEY_Escape, true));
        assert!(!overview_escape(keysyms::KEY_Escape, false));
        assert!(!overview_escape(keysyms::KEY_q, true));
    }

    #[test]
    fn emergency_escape_requires_control_alt_escape() {
        assert!(emergency_shortcut_escape(keysyms::KEY_Escape, true, true));
        assert!(!emergency_shortcut_escape(keysyms::KEY_Escape, true, false));
        assert!(!emergency_shortcut_escape(keysyms::KEY_q, true, true));
    }
    use smithay::input::keyboard::keysyms;

    #[test]
    fn maps_xkb_virtual_terminal_symbols_for_direct_sessions() {
        assert_eq!(
            virtual_terminal(keysyms::KEY_XF86Switch_VT_1, false, false, true),
            Some(1)
        );
        assert_eq!(
            virtual_terminal(keysyms::KEY_XF86Switch_VT_12, false, false, true),
            Some(12)
        );
    }

    #[test]
    fn accepts_plain_function_symbols_only_with_control_alt() {
        assert_eq!(virtual_terminal(keysyms::KEY_F7, true, true, true), Some(7));
        assert_eq!(virtual_terminal(keysyms::KEY_F7, true, false, true), None);
        assert_eq!(virtual_terminal(keysyms::KEY_F7, false, true, true), None);
    }

    #[test]
    fn leaves_virtual_terminal_keys_to_nested_clients() {
        assert_eq!(
            virtual_terminal(keysyms::KEY_XF86Switch_VT_3, false, false, false),
            None
        );
        assert_eq!(virtual_terminal(keysyms::KEY_F3, true, true, false), None);
    }
}
