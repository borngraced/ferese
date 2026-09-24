use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

const MIN_SPLIT_RATIO: f64 = 0.05;
const MAX_SPLIT_RATIO: f64 = 0.95;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WindowId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Window(WindowId),
    Split {
        axis: Axis,
        ratio: f64,
        first: NodeId,
        second: NodeId,
    },
    Stack {
        children: Vec<NodeId>,
        active: usize,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn center(self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GapConfig {
    pub inner: f64,
    pub outer: f64,
    pub smart: bool,
}

impl Default for GapConfig {
    fn default() -> Self {
        Self {
            inner: 10.0,
            outer: 10.0,
            smart: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    DuplicateWindow(WindowId),
    UnknownWindow(WindowId),
    InvalidTree(&'static str),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateWindow(window) => write!(formatter, "duplicate window {window:?}"),
            Self::UnknownWindow(window) => write!(formatter, "unknown window {window:?}"),
            Self::InvalidTree(reason) => write!(formatter, "invalid layout tree: {reason}"),
        }
    }
}

impl Error for LayoutError {}

#[derive(Debug, Default)]
pub struct LayoutTree {
    root: Option<NodeId>,
    nodes: HashMap<NodeId, Node>,
    parents: HashMap<NodeId, NodeId>,
    windows: HashMap<WindowId, NodeId>,
    next_node: u64,
}

impl LayoutTree {
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn contains(&self, window: WindowId) -> bool {
        self.windows.contains_key(&window)
    }

    pub fn window_ids(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.keys().copied()
    }

    pub fn insert(
        &mut self,
        window: WindowId,
        focused: Option<WindowId>,
        axis: Axis,
        ratio: f64,
    ) -> Result<NodeId, LayoutError> {
        if self.contains(window) {
            return Err(LayoutError::DuplicateWindow(window));
        }

        let Some(root) = self.root else {
            let window_node = self.insert_node(Node::Window(window));
            self.windows.insert(window, window_node);
            self.root = Some(window_node);
            return Ok(window_node);
        };

        let target = match focused {
            Some(focused) => self
                .windows
                .get(&focused)
                .copied()
                .ok_or(LayoutError::UnknownWindow(focused))?,
            None => self.first_window(root)?,
        };
        let old_parent = self.parents.get(&target).copied();
        let window_node = self.insert_node(Node::Window(window));
        self.windows.insert(window, window_node);
        let split = self.insert_node(Node::Split {
            axis,
            ratio: normalized_ratio(ratio),
            first: target,
            second: window_node,
        });

        self.parents.insert(target, split);
        self.parents.insert(window_node, split);

        if let Some(parent) = old_parent {
            self.replace_child(parent, target, split)?;
            self.parents.insert(split, parent);
        } else {
            self.root = Some(split);
        }

        debug_assert!(self.validate().is_ok());
        Ok(window_node)
    }

    pub fn remove(&mut self, window: WindowId) -> Result<(), LayoutError> {
        let leaf = self
            .windows
            .remove(&window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let Some(parent) = self.parents.remove(&leaf) else {
            self.nodes.remove(&leaf);
            self.root = None;
            return Ok(());
        };

        let sibling = match self.nodes.get(&parent) {
            Some(Node::Split { first, second, .. }) if *first == leaf => *second,
            Some(Node::Split { first, second, .. }) if *second == leaf => *first,
            _ => return Err(LayoutError::InvalidTree("window parent is not a split")),
        };
        let grandparent = self.parents.remove(&parent);

        self.nodes.remove(&leaf);
        self.nodes.remove(&parent);

        if let Some(grandparent) = grandparent {
            self.replace_child(grandparent, parent, sibling)?;
            self.parents.insert(sibling, grandparent);
        } else {
            self.parents.remove(&sibling);
            self.root = Some(sibling);
        }

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn geometry(&self, bounds: Rect) -> Result<HashMap<WindowId, Rect>, LayoutError> {
        let mut geometry = HashMap::new();

        if let Some(root) = self.root {
            self.layout_node(root, bounds, &mut geometry)?;
        }

        Ok(geometry)
    }

    pub fn geometry_with_gaps(
        &self,
        bounds: Rect,
        gaps: GapConfig,
    ) -> Result<HashMap<WindowId, Rect>, LayoutError> {
        let mut geometry = self.geometry(bounds)?;
        let outer = if gaps.smart && geometry.len() == 1 {
            0.0
        } else {
            valid_gap(gaps.outer)
        };
        let half_inner = valid_gap(gaps.inner) / 2.0;
        let right = bounds.x + bounds.width;
        let bottom = bounds.y + bounds.height;

        for rect in geometry.values_mut() {
            let left_inset = if nearly_equal(rect.x, bounds.x) {
                outer
            } else {
                half_inner
            };
            let top_inset = if nearly_equal(rect.y, bounds.y) {
                outer
            } else {
                half_inner
            };
            let right_inset = if nearly_equal(rect.x + rect.width, right) {
                outer
            } else {
                half_inner
            };
            let bottom_inset = if nearly_equal(rect.y + rect.height, bottom) {
                outer
            } else {
                half_inner
            };

            rect.x += left_inset;
            rect.y += top_inset;
            rect.width = (rect.width - left_inset - right_inset).max(1.0);
            rect.height = (rect.height - top_inset - bottom_inset).max(1.0);
        }

        Ok(geometry)
    }

    pub fn automatic_axis(
        &self,
        focused: Option<WindowId>,
        bounds: Rect,
    ) -> Result<Axis, LayoutError> {
        let Some(root) = self.root else {
            return Ok(Axis::Horizontal);
        };
        let target = match focused {
            Some(window) => window,
            None => match self.nodes.get(&self.first_window(root)?) {
                Some(Node::Window(window)) => *window,
                _ => return Err(LayoutError::InvalidTree("leaf is not a window")),
            },
        };
        let rect = self
            .geometry(bounds)?
            .remove(&target)
            .ok_or(LayoutError::UnknownWindow(target))?;

        Ok(if rect.width > rect.height {
            Axis::Horizontal
        } else {
            Axis::Vertical
        })
    }

    pub fn directional_neighbor(
        &self,
        window: WindowId,
        direction: Direction,
        bounds: Rect,
    ) -> Result<Option<WindowId>, LayoutError> {
        let geometry = self.geometry(bounds)?;
        let target = geometry
            .get(&window)
            .copied()
            .ok_or(LayoutError::UnknownWindow(window))?;
        let (target_x, target_y) = target.center();

        Ok(geometry
            .into_iter()
            .filter(|(candidate, _)| *candidate != window)
            .filter_map(|(candidate, rect)| {
                let (candidate_x, candidate_y) = rect.center();
                let (primary, secondary, aligned) = match direction {
                    Direction::Left if candidate_x < target_x => (
                        target_x - candidate_x,
                        (target_y - candidate_y).abs(),
                        intervals_overlap(target.y, target.height, rect.y, rect.height),
                    ),
                    Direction::Right if candidate_x > target_x => (
                        candidate_x - target_x,
                        (target_y - candidate_y).abs(),
                        intervals_overlap(target.y, target.height, rect.y, rect.height),
                    ),
                    Direction::Up if candidate_y < target_y => (
                        target_y - candidate_y,
                        (target_x - candidate_x).abs(),
                        intervals_overlap(target.x, target.width, rect.x, rect.width),
                    ),
                    Direction::Down if candidate_y > target_y => (
                        candidate_y - target_y,
                        (target_x - candidate_x).abs(),
                        intervals_overlap(target.x, target.width, rect.x, rect.width),
                    ),
                    _ => return None,
                };

                Some((candidate, primary, secondary, aligned))
            })
            .min_by(|left, right| {
                right
                    .3
                    .cmp(&left.3)
                    .then_with(|| left.1.total_cmp(&right.1))
                    .then_with(|| left.2.total_cmp(&right.2))
                    .then_with(|| left.0.0.cmp(&right.0.0))
            })
            .map(|(candidate, _, _, _)| candidate))
    }

    pub fn move_window(
        &mut self,
        window: WindowId,
        direction: Direction,
        bounds: Rect,
    ) -> Result<bool, LayoutError> {
        let Some(neighbor) = self.directional_neighbor(window, direction, bounds)? else {
            return Ok(false);
        };
        let window_node = self
            .windows
            .get(&window)
            .copied()
            .ok_or(LayoutError::UnknownWindow(window))?;
        let neighbor_node = self
            .windows
            .get(&neighbor)
            .copied()
            .ok_or(LayoutError::UnknownWindow(neighbor))?;

        self.nodes.insert(window_node, Node::Window(neighbor));
        self.nodes.insert(neighbor_node, Node::Window(window));
        self.windows.insert(window, neighbor_node);
        self.windows.insert(neighbor, window_node);

        debug_assert!(self.validate().is_ok());
        Ok(true)
    }

    pub fn resize_window(
        &mut self,
        window: WindowId,
        direction: Direction,
        amount: f64,
    ) -> Result<bool, LayoutError> {
        let mut child = self
            .windows
            .get(&window)
            .copied()
            .ok_or(LayoutError::UnknownWindow(window))?;
        let amount = valid_resize_amount(amount);

        if amount == 0.0 {
            return Ok(false);
        }

        while let Some(parent) = self.parents.get(&child).copied() {
            let adjustment = match self.nodes.get(&parent) {
                Some(Node::Split {
                    axis: Axis::Horizontal,
                    first,
                    ..
                }) if direction == Direction::Right && *first == child => Some(amount),
                Some(Node::Split {
                    axis: Axis::Horizontal,
                    second,
                    ..
                }) if direction == Direction::Left && *second == child => Some(-amount),
                Some(Node::Split {
                    axis: Axis::Vertical,
                    first,
                    ..
                }) if direction == Direction::Down && *first == child => Some(amount),
                Some(Node::Split {
                    axis: Axis::Vertical,
                    second,
                    ..
                }) if direction == Direction::Up && *second == child => Some(-amount),
                Some(Node::Split { .. } | Node::Stack { .. }) => None,
                Some(Node::Window(_)) => {
                    return Err(LayoutError::InvalidTree("window cannot be a parent"));
                }
                None => return Err(LayoutError::InvalidTree("parent node is missing")),
            };

            if let Some(adjustment) = adjustment {
                let Some(Node::Split { ratio, .. }) = self.nodes.get_mut(&parent) else {
                    return Err(LayoutError::InvalidTree("resize target is not a split"));
                };
                let resized = normalized_ratio(*ratio + adjustment);
                let changed = resized != *ratio;
                *ratio = resized;

                debug_assert!(self.validate().is_ok());
                return Ok(changed);
            }

            child = parent;
        }

        Ok(false)
    }

    pub fn validate(&self) -> Result<(), LayoutError> {
        let Some(root) = self.root else {
            return if self.nodes.is_empty() && self.parents.is_empty() && self.windows.is_empty() {
                Ok(())
            } else {
                Err(LayoutError::InvalidTree("empty root retains nodes"))
            };
        };
        if self.parents.contains_key(&root) {
            return Err(LayoutError::InvalidTree("root has a parent"));
        }

        let mut visited = HashSet::new();
        self.validate_node(root, None, &mut visited)?;

        if visited.len() != self.nodes.len() {
            return Err(LayoutError::InvalidTree("tree contains unreachable nodes"));
        }
        if self.windows.len()
            != self
                .nodes
                .values()
                .filter(|node| matches!(node, Node::Window(_)))
                .count()
        {
            return Err(LayoutError::InvalidTree("window index is inconsistent"));
        }

        Ok(())
    }

    fn insert_node(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node += 1;
        self.nodes.insert(id, node);
        id
    }

    fn first_window(&self, node: NodeId) -> Result<NodeId, LayoutError> {
        match self.nodes.get(&node) {
            Some(Node::Window(_)) => Ok(node),
            Some(Node::Split { first, .. }) => self.first_window(*first),
            Some(Node::Stack {
                children, active, ..
            }) => children
                .get(*active)
                .copied()
                .ok_or(LayoutError::InvalidTree("stack active index is invalid"))
                .and_then(|child| self.first_window(child)),
            None => Err(LayoutError::InvalidTree("node is missing")),
        }
    }

    fn replace_child(
        &mut self,
        parent: NodeId,
        old: NodeId,
        new: NodeId,
    ) -> Result<(), LayoutError> {
        match self.nodes.get_mut(&parent) {
            Some(Node::Split { first, .. }) if *first == old => *first = new,
            Some(Node::Split { second, .. }) if *second == old => *second = new,
            Some(Node::Stack { children, .. }) => {
                let child = children
                    .iter_mut()
                    .find(|child| **child == old)
                    .ok_or(LayoutError::InvalidTree("parent does not contain child"))?;
                *child = new;
            }
            _ => return Err(LayoutError::InvalidTree("parent does not contain child")),
        }

        Ok(())
    }

    fn layout_node(
        &self,
        node: NodeId,
        bounds: Rect,
        geometry: &mut HashMap<WindowId, Rect>,
    ) -> Result<(), LayoutError> {
        match self.nodes.get(&node) {
            Some(Node::Window(window)) => {
                geometry.insert(*window, bounds);
            }
            Some(Node::Split {
                axis,
                ratio,
                first,
                second,
            }) => {
                let (first_bounds, second_bounds) = split_rect(bounds, *axis, *ratio);
                self.layout_node(*first, first_bounds, geometry)?;
                self.layout_node(*second, second_bounds, geometry)?;
            }
            Some(Node::Stack { children, active }) => {
                let child = children
                    .get(*active)
                    .ok_or(LayoutError::InvalidTree("stack active index is invalid"))?;
                self.layout_node(*child, bounds, geometry)?;
            }
            None => return Err(LayoutError::InvalidTree("node is missing")),
        }

        Ok(())
    }

    fn validate_node(
        &self,
        node: NodeId,
        expected_parent: Option<NodeId>,
        visited: &mut HashSet<NodeId>,
    ) -> Result<(), LayoutError> {
        if !visited.insert(node) {
            return Err(LayoutError::InvalidTree("tree contains a cycle"));
        }
        if self.parents.get(&node).copied() != expected_parent {
            return Err(LayoutError::InvalidTree("parent index is inconsistent"));
        }

        match self.nodes.get(&node) {
            Some(Node::Window(window)) => {
                if self.windows.get(window) != Some(&node) {
                    return Err(LayoutError::InvalidTree("window index is inconsistent"));
                }
            }
            Some(Node::Split {
                ratio,
                first,
                second,
                ..
            }) => {
                if first == second
                    || !ratio.is_finite()
                    || !(MIN_SPLIT_RATIO..=MAX_SPLIT_RATIO).contains(ratio)
                {
                    return Err(LayoutError::InvalidTree("split is invalid"));
                }

                self.validate_node(*first, Some(node), visited)?;
                self.validate_node(*second, Some(node), visited)?;
            }
            Some(Node::Stack { children, active }) => {
                if children.len() < 2 || *active >= children.len() {
                    return Err(LayoutError::InvalidTree("stack is invalid"));
                }

                for child in children {
                    self.validate_node(*child, Some(node), visited)?;
                }
            }
            None => return Err(LayoutError::InvalidTree("node is missing")),
        }

        Ok(())
    }
}

fn normalized_ratio(ratio: f64) -> f64 {
    if ratio.is_finite() {
        ratio.clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO)
    } else {
        0.5
    }
}

fn valid_gap(gap: f64) -> f64 {
    if gap.is_finite() { gap.max(0.0) } else { 0.0 }
}

fn valid_resize_amount(amount: f64) -> f64 {
    if amount.is_finite() {
        amount.abs()
    } else {
        0.0
    }
}

fn nearly_equal(left: f64, right: f64) -> bool {
    (left - right).abs() < f64::EPSILON * left.abs().max(right.abs()).max(1.0)
}

fn intervals_overlap(
    first_start: f64,
    first_size: f64,
    second_start: f64,
    second_size: f64,
) -> bool {
    first_start < second_start + second_size && second_start < first_start + first_size
}

fn split_rect(bounds: Rect, axis: Axis, ratio: f64) -> (Rect, Rect) {
    match axis {
        Axis::Horizontal => {
            let boundary = bounds.x + bounds.width * ratio;
            let first = Rect::new(bounds.x, bounds.y, boundary - bounds.x, bounds.height);
            let second = Rect::new(
                boundary,
                bounds.y,
                bounds.x + bounds.width - boundary,
                bounds.height,
            );

            (first, second)
        }
        Axis::Vertical => {
            let boundary = bounds.y + bounds.height * ratio;
            let first = Rect::new(bounds.x, bounds.y, bounds.width, boundary - bounds.y);
            let second = Rect::new(
                bounds.x,
                boundary,
                bounds.width,
                bounds.y + bounds.height - boundary,
            );

            (first, second)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_splits_the_focused_leaf() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(3), Some(WindowId(1)), Axis::Vertical, 0.5)
            .unwrap();

        assert!(tree.validate().is_ok());
        assert_eq!(
            tree.geometry(Rect::new(0.0, 0.0, 100.0, 80.0))
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn removal_collapses_the_parent_split() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();
        tree.remove(WindowId(1)).unwrap();

        assert!(tree.validate().is_ok());
        assert_eq!(
            tree.node(tree.root().unwrap()),
            Some(&Node::Window(WindowId(2)))
        );
    }

    #[test]
    fn shared_split_boundary_is_exact() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 1.0 / 3.0)
            .unwrap();
        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 1.0 / 3.0)
            .unwrap();

        let geometry = tree.geometry(Rect::new(0.0, 0.0, 101.0, 80.0)).unwrap();
        let first = geometry[&WindowId(1)];
        let second = geometry[&WindowId(2)];

        assert_eq!(first.x + first.width, second.x);
        assert_eq!(second.x + second.width, 101.0);
    }

    #[test]
    fn rejects_duplicate_and_unknown_windows() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(
            tree.insert(WindowId(1), None, Axis::Horizontal, 0.5),
            Err(LayoutError::DuplicateWindow(WindowId(1)))
        );
        assert_eq!(
            tree.insert(WindowId(2), Some(WindowId(9)), Axis::Horizontal, 0.5,),
            Err(LayoutError::UnknownWindow(WindowId(9)))
        );
    }

    #[test]
    fn non_finite_ratio_uses_balanced_split() {
        assert_eq!(normalized_ratio(f64::NAN), 0.5);
        assert_eq!(normalized_ratio(f64::INFINITY), 0.5);
    }

    #[test]
    fn automatic_axis_follows_focused_tile_shape() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();

        let wide = Rect::new(0.0, 0.0, 1_920.0, 1_080.0);
        assert_eq!(
            tree.automatic_axis(Some(WindowId(1)), wide).unwrap(),
            Axis::Horizontal
        );

        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();
        assert_eq!(
            tree.automatic_axis(Some(WindowId(1)), wide).unwrap(),
            Axis::Vertical
        );
    }

    #[test]
    fn gaps_preserve_outer_and_shared_spacing() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();

        let geometry = tree
            .geometry_with_gaps(Rect::new(0.0, 0.0, 100.0, 80.0), GapConfig::default())
            .unwrap();
        let first = geometry[&WindowId(1)];
        let second = geometry[&WindowId(2)];

        assert_eq!(first.x, 10.0);
        assert_eq!(second.x + second.width, 90.0);
        assert_eq!(second.x - (first.x + first.width), 10.0);
    }

    #[test]
    fn smart_gaps_remove_outer_gap_for_one_window() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();

        let bounds = Rect::new(0.0, 0.0, 100.0, 80.0);
        let geometry = tree
            .geometry_with_gaps(bounds, GapConfig::default())
            .unwrap();

        assert_eq!(geometry[&WindowId(1)], bounds);
    }

    #[test]
    fn directional_neighbor_prefers_primary_distance() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(3), Some(WindowId(2)), Axis::Vertical, 0.5)
            .unwrap();

        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(
            tree.directional_neighbor(WindowId(1), Direction::Right, bounds)
                .unwrap(),
            Some(WindowId(2))
        );
        assert_eq!(
            tree.directional_neighbor(WindowId(2), Direction::Down, bounds)
                .unwrap(),
            Some(WindowId(3))
        );
        assert_eq!(
            tree.directional_neighbor(WindowId(1), Direction::Left, bounds)
                .unwrap(),
            None
        );
    }

    #[test]
    fn directional_move_swaps_window_leaves() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();
        let bounds = Rect::new(0.0, 0.0, 100.0, 80.0);

        assert!(
            tree.move_window(WindowId(1), Direction::Right, bounds)
                .unwrap()
        );

        let geometry = tree.geometry(bounds).unwrap();
        assert_eq!(geometry[&WindowId(2)].x, 0.0);
        assert_eq!(geometry[&WindowId(1)].x, 50.0);
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn directional_move_at_boundary_is_a_noop() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();

        assert!(
            !tree
                .move_window(
                    WindowId(1),
                    Direction::Left,
                    Rect::new(0.0, 0.0, 100.0, 80.0),
                )
                .unwrap()
        );
    }

    #[test]
    fn directional_resize_changes_the_nearest_matching_split() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(3), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);

        assert!(
            tree.resize_window(WindowId(1), Direction::Right, 0.1)
                .unwrap()
        );

        let geometry = tree.geometry(bounds).unwrap();
        assert_eq!(geometry[&WindowId(1)].width, 30.0);
        assert_eq!(geometry[&WindowId(3)].x, 30.0);
        assert_eq!(geometry[&WindowId(2)].x, 50.0);
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn directional_resize_walks_to_an_ancestor_and_clamps() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(2), Some(WindowId(1)), Axis::Horizontal, 0.5)
            .unwrap();
        tree.insert(WindowId(3), Some(WindowId(1)), Axis::Vertical, 0.5)
            .unwrap();
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);

        assert!(
            tree.resize_window(WindowId(3), Direction::Right, 1.0)
                .unwrap()
        );
        assert!(
            !tree
                .resize_window(WindowId(3), Direction::Right, 1.0)
                .unwrap()
        );

        let geometry = tree.geometry(bounds).unwrap();
        assert_eq!(geometry[&WindowId(2)].x, 95.0);
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn directional_resize_at_an_outer_edge_is_a_noop() {
        let mut tree = LayoutTree::default();
        tree.insert(WindowId(1), None, Axis::Horizontal, 0.5)
            .unwrap();

        assert!(
            !tree
                .resize_window(WindowId(1), Direction::Left, 0.05)
                .unwrap()
        );
        assert!(
            !tree
                .resize_window(WindowId(1), Direction::Right, f64::NAN)
                .unwrap()
        );
    }
}
