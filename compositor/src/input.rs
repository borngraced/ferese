use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    input::{
        keyboard::{FilterResult, keysyms},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerHandle, RelativeMotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
    wayland::{
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        pointer_constraints::{PointerConstraint, with_pointer_constraint},
    },
};

use crate::Ferese;
use ferese_layout::Direction;

impl Ferese {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);

        match event {
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
                        if state == KeyState::Released && data.intercepted_keys.remove(&keycode) {
                            return FilterResult::Intercept(());
                        }

                        let symbol = keysym.modified_sym().raw();
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

                        let direction = if modifiers.logo && !modifiers.ctrl && !modifiers.alt {
                            match symbol {
                                keysyms::KEY_h | keysyms::KEY_H => Some(Direction::Left),
                                keysyms::KEY_j | keysyms::KEY_J => Some(Direction::Down),
                                keysyms::KEY_k | keysyms::KEY_K => Some(Direction::Up),
                                keysyms::KEY_l | keysyms::KEY_L => Some(Direction::Right),
                                _ => None,
                            }
                        } else {
                            None
                        };

                        if let Some(direction) = direction {
                            if state == KeyState::Pressed {
                                data.intercepted_keys.insert(keycode);

                                if modifiers.shift {
                                    data.move_direction(direction);
                                } else {
                                    data.focus_direction(direction);
                                }
                            }

                            FilterResult::Intercept(())
                        } else if modifiers.logo && !modifiers.ctrl && !modifiers.alt {
                            let workspace = match symbol {
                                keysyms::KEY_1 => Some(1),
                                keysyms::KEY_2 => Some(2),
                                keysyms::KEY_3 => Some(3),
                                keysyms::KEY_4 => Some(4),
                                keysyms::KEY_5 => Some(5),
                                keysyms::KEY_6 => Some(6),
                                keysyms::KEY_7 => Some(7),
                                keysyms::KEY_8 => Some(8),
                                keysyms::KEY_9 => Some(9),
                                _ => None,
                            };

                            if let Some(workspace) = workspace {
                                if state == KeyState::Pressed {
                                    data.intercepted_keys.insert(keycode);

                                    if modifiers.shift {
                                        data.move_focused_to_workspace(workspace);
                                    } else {
                                        data.switch_workspace(workspace);
                                    }
                                }

                                FilterResult::Intercept(())
                            } else if !modifiers.shift
                                && matches!(symbol, keysyms::KEY_q | keysyms::KEY_Q)
                            {
                                if state == KeyState::Pressed {
                                    data.intercepted_keys.insert(keycode);
                                    data.close_focused_window();
                                }

                                FilterResult::Intercept(())
                            } else if !modifiers.shift
                                && matches!(symbol, keysyms::KEY_f | keysyms::KEY_F)
                            {
                                if state == KeyState::Pressed {
                                    data.intercepted_keys.insert(keycode);
                                    data.toggle_focused_fullscreen();
                                }

                                FilterResult::Intercept(())
                            } else if !modifiers.shift
                                && matches!(symbol, keysyms::KEY_m | keysyms::KEY_M)
                            {
                                if state == KeyState::Pressed {
                                    data.intercepted_keys.insert(keycode);
                                    data.toggle_layout_mode();
                                }

                                FilterResult::Intercept(())
                            } else if modifiers.shift && symbol == keysyms::KEY_space {
                                if state == KeyState::Pressed {
                                    data.intercepted_keys.insert(keycode);
                                    data.toggle_focused_floating();
                                }

                                FilterResult::Intercept(())
                            } else {
                                FilterResult::Forward
                            }
                        } else {
                            FilterResult::Forward
                        }
                    },
                );
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next() else {
                    return;
                };
                let Some(geometry) = self.space.output_geometry(output) else {
                    return;
                };
                let position = event.position_transformed(geometry.size) + geometry.loc.to_f64();
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
                self.activate_focused_pointer_constraint(&pointer);
                crate::backends::direct::render_all(self);
            }
            InputEvent::PointerMotion { event, .. } => {
                let pointer = self.seat.get_pointer().expect("seat has a pointer");
                let position = pointer.current_location();
                let focus = self.surface_under(position);
                let (scale_x, scale_y) = self
                    .window_under_visual(position)
                    .and_then(|window| self.visual_scale_for_window(&window))
                    .unwrap_or((1.0, 1.0));
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
                self.activate_focused_pointer_constraint(&pointer);
                crate::backends::direct::render_all(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().expect("seat has a pointer");
                let serial = SERIAL_COUNTER.next_serial();

                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
                    self.focus_window_under_pointer(serial);
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
            _ => {}
        }
    }

    fn focus_window_under_pointer(&mut self, serial: Serial) {
        let pointer = self.seat.get_pointer().expect("seat has a pointer");
        let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");
        let position = pointer.current_location();
        self.focus_output_at(position);

        if let Some((layer, _, _)) = self.layer_under(position) {
            if layer.can_receive_keyboard_focus() {
                keyboard.set_focus(self, Some(layer.wl_surface().clone()), serial);
            }
            return;
        }

        if let Some(window) = self.window_under_visual(position) {
            self.focused_window = self.window_ids.get(&window).copied();
            if let Some(focused) = self.focused_window
                && let Err(error) = self.workspaces.focus_window(focused)
            {
                tracing::error!(%error, ?focused, "failed to update workspace focus");
            }
            self.space.raise_element(&window, true);
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

        self.relayout();
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

#[cfg(test)]
mod tests {
    use super::{emergency_shortcut_escape, virtual_terminal};

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
