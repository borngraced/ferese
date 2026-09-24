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
    wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint},
};

use crate::Ferese;
use ferese_layout::Direction;

impl Ferese {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
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

                        let direction = if modifiers.logo && !modifiers.ctrl && !modifiers.alt {
                            match keysym.modified_sym().raw() {
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
            }
            InputEvent::PointerMotion { event, .. } => {
                let pointer = self.seat.get_pointer().expect("seat has a pointer");
                let focus = self.surface_under(pointer.current_location());

                pointer.relative_motion(
                    self,
                    focus,
                    &RelativeMotionEvent {
                        delta: event.delta(),
                        delta_unaccel: event.delta_unaccel(),
                        utime: (event.time_msec() as u64).saturating_mul(1_000),
                    },
                );
                pointer.frame(self);
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

        if let Some((window, _)) = self.space.element_under(pointer.current_location()) {
            let window = window.clone();

            self.focused_window = self.window_ids.get(&window).copied();
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
