use smithay::desktop::Window;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
    MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Size};

use crate::Ferese;

pub struct MoveSurfaceGrab {
    pub start_data: GrabStartData<Ferese>,
    pub window: Window,
    pub initial_location: Point<i32, Logical>,
    pub initial_size: Size<i32, Logical>,
    pub finished: bool,
}

impl PointerGrab<Ferese> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut Ferese,
        handle: &mut PointerInnerHandle<'_, Ferese>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let location = self.initial_location.to_f64() + (event.location - self.start_data.location);
        let location = location.to_i32_round();
        data.set_floating_window_geometry(&self.window, location, self.initial_size);
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

    fn button(&mut self, data: &mut Ferese, handle: &mut PointerInnerHandle<'_, Ferese>, event: &ButtonEvent) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            self.finished = true;
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut Ferese, handle: &mut PointerInnerHandle<'_, Ferese>, frame: AxisFrame) {
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

        data.set_floating_window_geometry(&self.window, self.initial_location, self.initial_size);
    }
}
