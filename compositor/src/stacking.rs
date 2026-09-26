use ferese_layout::WindowId;

pub(crate) fn layer_priority(floating: bool, zooming: bool, fullscreen: bool) -> u8 {
    if fullscreen {
        3
    } else if floating {
        2
    } else if zooming {
        1
    } else {
        0
    }
}

/// Persistent back-to-front order. Geometry updates never change this order.
#[derive(Default)]
pub(crate) struct WindowStack {
    order: Vec<WindowId>,
}

impl WindowStack {
    pub fn insert(&mut self, id: WindowId) {
        if !self.order.contains(&id) {
            self.order.push(id);
        }
    }

    pub fn remove(&mut self, id: WindowId) {
        self.order.retain(|candidate| *candidate != id);
    }

    pub fn raise(&mut self, id: WindowId) {
        self.remove(id);
        self.insert(id);
    }

    pub fn rank(&self, id: WindowId) -> usize {
        self.order
            .iter()
            .position(|candidate| *candidate == id)
            .unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_windows_stay_above_tiled_and_maximized_windows() {
        assert!(layer_priority(true, false, false) > layer_priority(false, true, false));
        assert!(layer_priority(false, true, false) > layer_priority(false, false, false));
        assert!(layer_priority(false, false, true) > layer_priority(true, false, false));
    }

    #[test]
    fn remapping_neighbours_does_not_change_order() {
        let mut stack = WindowStack::default();
        stack.insert(WindowId(1));
        stack.insert(WindowId(2));
        stack.raise(WindowId(1));
        stack.insert(WindowId(2));
        assert!(stack.rank(WindowId(1)) > stack.rank(WindowId(2)));
        stack.remove(WindowId(1));
        assert_eq!(stack.rank(WindowId(2)), 0);
    }
}
