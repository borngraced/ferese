use std::cell::RefCell;

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};

use crate::Ferese;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResizeEdge(xdg_toplevel::ResizeEdge);

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    fn from(edge: xdg_toplevel::ResizeEdge) -> Self {
        Self(edge)
    }
}

impl ResizeEdge {
    fn left(self) -> bool {
        matches!(
            self.0,
            xdg_toplevel::ResizeEdge::Left
                | xdg_toplevel::ResizeEdge::TopLeft
                | xdg_toplevel::ResizeEdge::BottomLeft
        )
    }

    fn right(self) -> bool {
        matches!(
            self.0,
            xdg_toplevel::ResizeEdge::Right
                | xdg_toplevel::ResizeEdge::TopRight
                | xdg_toplevel::ResizeEdge::BottomRight
        )
    }

    fn top(self) -> bool {
        matches!(
            self.0,
            xdg_toplevel::ResizeEdge::Top
                | xdg_toplevel::ResizeEdge::TopLeft
                | xdg_toplevel::ResizeEdge::TopRight
        )
    }

    fn bottom(self) -> bool {
        matches!(
            self.0,
            xdg_toplevel::ResizeEdge::Bottom
                | xdg_toplevel::ResizeEdge::BottomLeft
                | xdg_toplevel::ResizeEdge::BottomRight
        )
    }
}

pub struct ResizeSurfaceGrab {
    start_data: GrabStartData<Ferese>,
    window: Window,
    edges: ResizeEdge,
    initial_rect: Rectangle<i32, Logical>,
    last_size: Size<i32, Logical>,
    finished: bool,
}

impl ResizeSurfaceGrab {
    pub fn new(
        start_data: GrabStartData<Ferese>,
        window: Window,
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    ) -> Self {
        ResizeState::with(
            window
                .toplevel()
                .expect("managed window has a toplevel")
                .wl_surface(),
            |state| {
                *state = ResizeState::Resizing {
                    edges,
                    initial_rect,
                }
            },
        );
        Self {
            start_data,
            window,
            edges,
            initial_rect,
            last_size: initial_rect.size,
            finished: false,
        }
    }
}

impl PointerGrab<Ferese> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let delta = event.location - self.start_data.location;
        let surface = self
            .window
            .toplevel()
            .expect("managed window has a toplevel");
        let (minimum, maximum) = with_states(surface.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let state = cached.current();
            (state.min_size, state.max_size)
        });
        self.last_size =
            constrained_size(self.initial_rect.size, delta, self.edges, minimum, maximum);
        let rect = resized_rect(self.initial_rect, self.last_size, self.edges);
        data.set_floating_window_geometry(&self.window, rect.loc, rect.size);

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
            state.size = Some(self.last_size);
        });
        surface.send_pending_configure();
    }

    fn relative_motion(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            self.finished = true;
            handle.unset_grab(self, data, event.serial, event.time, true);
            let surface = self
                .window
                .toplevel()
                .expect("managed window has a toplevel");
            surface.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Resizing);
                state.size = Some(self.last_size);
            });
            surface.send_pending_configure();
            ResizeState::with(surface.wl_surface(), |state| {
                *state = ResizeState::WaitingForFinalCommit {
                    edges: self.edges,
                    initial_rect: self.initial_rect,
                };
            });
        }
    }

    fn axis(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        frame: AxisFrame,
    ) {
        handle.axis(data, frame);
    }

    fn frame(&mut self, data: &mut Ferese, handle: &mut PointerInnerHandle<'_, Ferese>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }
    fn gesture_swipe_update(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }
    fn gesture_swipe_end(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }
    fn gesture_pinch_begin(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }
    fn gesture_pinch_update(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }
    fn gesture_pinch_end(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }
    fn gesture_hold_begin(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }
    fn gesture_hold_end(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<Ferese> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut Ferese) {
        if self.finished {
            return;
        }

        let surface = self
            .window
            .toplevel()
            .expect("managed window has a toplevel");
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Resizing);
            state.size = Some(self.initial_rect.size);
        });
        surface.send_pending_configure();
        ResizeState::with(surface.wl_surface(), |state| *state = ResizeState::Idle);
        data.set_floating_window_geometry(
            &self.window,
            self.initial_rect.loc,
            self.initial_rect.size,
        );
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum ResizeState {
    #[default]
    Idle,
    Resizing {
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    },
    WaitingForFinalCommit {
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    },
}

impl ResizeState {
    fn with<T>(surface: &WlSurface, callback: impl FnOnce(&mut Self) -> T) -> T {
        with_states(surface, |states| {
            states.data_map.insert_if_missing(RefCell::<Self>::default);
            callback(
                &mut states
                    .data_map
                    .get::<RefCell<Self>>()
                    .expect("resize state was inserted")
                    .borrow_mut(),
            )
        })
    }

    fn commit(&mut self) -> Option<(ResizeEdge, Rectangle<i32, Logical>)> {
        match *self {
            Self::Idle => None,
            Self::Resizing {
                edges,
                initial_rect,
            } => Some((edges, initial_rect)),
            Self::WaitingForFinalCommit {
                edges,
                initial_rect,
            } => {
                *self = Self::Idle;
                Some((edges, initial_rect))
            }
        }
    }
}

pub fn handle_resize_commit(surface: &WlSurface) {
    // The compositor's visual rect owns the anchored edge. A delayed client
    // commit must not move the window or change its stacking order.
    ResizeState::with(surface, ResizeState::commit);
}

fn constrained_size(
    initial: Size<i32, Logical>,
    delta: Point<f64, Logical>,
    edges: ResizeEdge,
    minimum: Size<i32, Logical>,
    maximum: Size<i32, Logical>,
) -> Size<i32, Logical> {
    let mut width = initial.w;
    let mut height = initial.h;
    if edges.left() {
        width -= delta.x as i32;
    } else if edges.right() {
        width += delta.x as i32;
    }
    if edges.top() {
        height -= delta.y as i32;
    } else if edges.bottom() {
        height += delta.y as i32;
    }

    let maximum_width = if maximum.w == 0 { i32::MAX } else { maximum.w };
    let maximum_height = if maximum.h == 0 { i32::MAX } else { maximum.h };
    (
        width.clamp(minimum.w.max(1), maximum_width),
        height.clamp(minimum.h.max(1), maximum_height),
    )
        .into()
}

fn resized_rect(
    initial: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
    edges: ResizeEdge,
) -> Rectangle<i32, Logical> {
    let mut location = initial.loc;

    if edges.left() {
        location.x += initial.size.w - size.w;
    }
    if edges.top() {
        location.y += initial.size.h - size.h;
    }

    Rectangle::new(location, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_and_top_edges_invert_pointer_delta() {
        let size = constrained_size(
            (800, 600).into(),
            (100.0, 50.0).into(),
            ResizeEdge(xdg_toplevel::ResizeEdge::TopLeft),
            (1, 1).into(),
            (0, 0).into(),
        );
        assert_eq!(size, (700, 550).into());
    }

    #[test]
    fn right_and_bottom_edges_follow_pointer_delta() {
        let size = constrained_size(
            (800, 600).into(),
            (100.0, 50.0).into(),
            ResizeEdge(xdg_toplevel::ResizeEdge::BottomRight),
            (1, 1).into(),
            (0, 0).into(),
        );
        assert_eq!(size, (900, 650).into());
    }

    #[test]
    fn client_constraints_clamp_interactive_size() {
        let size = constrained_size(
            (800, 600).into(),
            (1_000.0, 1_000.0).into(),
            ResizeEdge(xdg_toplevel::ResizeEdge::BottomRight),
            (640, 480).into(),
            (1_024, 768).into(),
        );
        assert_eq!(size, (1_024, 768).into());
    }

    #[test]
    fn resized_rect_keeps_the_opposite_edge_fixed() {
        let initial = Rectangle::new((100, 80).into(), (800, 600).into());
        let resized = resized_rect(
            initial,
            (700, 550).into(),
            ResizeEdge(xdg_toplevel::ResizeEdge::TopLeft),
        );

        assert_eq!(resized.loc, (200, 130).into());
        assert_eq!(resized.size, (700, 550).into());
    }
}
