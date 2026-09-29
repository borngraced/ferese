use ferese_layout::WindowId;
use std::collections::HashMap;

pub(crate) fn layer_priority(
    floating: bool,
    zooming: bool,
    fullscreen: bool,
    above_fullscreen: bool,
) -> u8 {
    if above_fullscreen {
        4
    } else if fullscreen {
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
    ranks: HashMap<WindowId, usize>,
}

impl WindowStack {
    pub fn insert(&mut self, id: WindowId) {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.ranks.entry(id) {
            entry.insert(self.order.len());
            self.order.push(id);
        }
    }

    pub fn remove(&mut self, id: WindowId) {
        if let Some(rank) = self.ranks.remove(&id) {
            self.order.remove(rank);
            for (rank, candidate) in self.order.iter().enumerate().skip(rank) {
                self.ranks.insert(*candidate, rank);
            }
        }
    }

    pub fn raise(&mut self, id: WindowId) {
        self.remove(id);
        self.insert(id);
    }

    pub fn rank(&self, id: WindowId) -> usize {
        self.ranks.get(&id).copied().unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_ranks_match_reference_order_after_mixed_operations() {
        let mut stack = WindowStack::default();
        let mut reference = Vec::new();
        let mut random = 42_u64;
        for _ in 0..2000 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let id = WindowId((random >> 32) % 32);
            match random % 3 {
                0 => {
                    stack.insert(id);
                    if !reference.contains(&id) {
                        reference.push(id);
                    }
                }
                1 => {
                    stack.remove(id);
                    reference.retain(|candidate| *candidate != id);
                }
                _ => {
                    stack.raise(id);
                    reference.retain(|candidate| *candidate != id);
                    reference.push(id);
                }
            }
            for id in (0..33).map(WindowId) {
                assert_eq!(
                    stack.rank(id),
                    reference
                        .iter()
                        .position(|candidate| *candidate == id)
                        .unwrap_or(usize::MAX)
                );
            }
        }
    }

    #[test]
    fn floating_windows_stay_above_tiled_and_maximized_windows() {
        assert!(
            layer_priority(true, false, false, false) > layer_priority(false, true, false, false)
        );
        assert!(
            layer_priority(false, true, false, false) > layer_priority(false, false, false, false)
        );
        assert!(
            layer_priority(false, false, true, false) > layer_priority(true, false, false, false)
        );
        assert!(
            layer_priority(true, false, false, true) > layer_priority(false, false, true, false)
        );
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
