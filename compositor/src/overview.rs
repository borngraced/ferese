use std::{collections::HashMap, time::Duration};

use ferese_animation::{AnimatedRect, SpringConfig};
use ferese_layout::{Direction, Rect, WindowId};

use crate::Ferese;

const OVERVIEW_MARGIN: f64 = 48.0;
const OVERVIEW_GAP: f64 = 24.0;
const MAX_PREVIEW_SCALE: f64 = 0.82;

#[derive(Debug, Default)]
pub(crate) struct OverviewState {
    active: bool,
    selected: Option<WindowId>,
    presentations: HashMap<WindowId, AnimatedRect>,
}

impl OverviewState {
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn is_presenting(&self) -> bool {
        self.active || !self.presentations.is_empty()
    }

    pub(crate) fn selected(&self) -> Option<WindowId> {
        self.selected
    }

    pub(crate) fn select_direction(&mut self, direction: Direction) -> bool {
        let Some(current) = self
            .selected
            .and_then(|id| self.presentations.get(&id))
            .map(|presentation| presentation.current)
        else {
            return false;
        };
        let current_center = rect_center(current);
        let next = self
            .presentations
            .iter()
            .filter(|(id, _)| Some(**id) != self.selected)
            .filter_map(|(id, presentation)| {
                let center = rect_center(presentation.current);
                directional_distance(direction, current_center, center)
                    .map(|distance| (*id, distance))
            })
            .min_by(|(left_id, left), (right_id, right)| {
                left.total_cmp(right)
                    .then_with(|| left_id.0.cmp(&right_id.0))
            })
            .map(|(id, _)| id);
        let Some(next) = next else {
            return false;
        };

        self.selected = Some(next);
        true
    }

    pub(crate) fn presented_rect(&self, id: WindowId, normal: Rect) -> Rect {
        self.presentations
            .get(&id)
            .map_or(normal, |presentation| presentation.current)
    }

    fn enter(
        &mut self,
        targets: HashMap<WindowId, (Rect, Rect)>,
        selected: Option<WindowId>,
        animations_enabled: bool,
    ) {
        self.active = true;
        self.selected = selected
            .filter(|id| targets.contains_key(id))
            .or_else(|| targets.keys().copied().min_by_key(|id| id.0));
        self.retarget(targets, animations_enabled);
    }

    fn retarget(&mut self, targets: HashMap<WindowId, (Rect, Rect)>, animations_enabled: bool) {
        self.presentations.retain(|id, _| targets.contains_key(id));

        for (id, (normal, target)) in targets {
            let presentation = self
                .presentations
                .entry(id)
                .or_insert_with(|| AnimatedRect::new(normal));

            presentation.set_target(target);
            if !animations_enabled {
                presentation.snap();
            }
        }
    }

    fn exit(&mut self, normal: HashMap<WindowId, Rect>, animations_enabled: bool) {
        self.active = false;
        self.selected = None;
        self.presentations.retain(|id, presentation| {
            let Some(target) = normal.get(id).copied() else {
                return false;
            };

            presentation.set_target(target);
            if !animations_enabled {
                presentation.snap();
            }
            true
        });

        if !animations_enabled {
            self.presentations.clear();
        }
    }

    pub(crate) fn advance(
        &mut self,
        delta: Duration,
        spring: SpringConfig,
        animations_enabled: bool,
    ) -> bool {
        let mut active_animation = false;

        for presentation in self.presentations.values_mut() {
            if animations_enabled {
                active_animation |= presentation.advance(delta, spring);
            } else {
                presentation.snap();
            }
        }

        if !self.active && !active_animation {
            self.presentations.clear();
        }

        active_animation
    }
}

impl Ferese {
    pub(crate) fn set_overview_active(&mut self, active: bool) {
        if self.overview.is_active() == active {
            return;
        }

        if active {
            let targets = self.overview_targets();
            self.overview
                .enter(targets, self.focused_window, self.animations_enabled());
        } else {
            let normal = self.normal_window_rects();
            self.overview.exit(normal, self.animations_enabled());
        }

        self.notify_overview_state();
        crate::backends::direct::render_all(self);
    }

    pub(crate) fn toggle_overview(&mut self) {
        self.set_overview_active(!self.overview.is_active());
    }

    pub(crate) fn focus_overview_direction(&mut self, direction: Direction) -> bool {
        if !self.overview.is_active() {
            return false;
        }

        if self.overview.select_direction(direction) {
            crate::backends::direct::render_all(self);
        }

        true
    }

    pub(crate) fn select_overview_window(&mut self, id: WindowId) -> bool {
        if !self.overview.is_active() || !self.overview.presentations.contains_key(&id) {
            return false;
        }

        if !self.activate_managed_window(id) {
            return false;
        }

        self.set_overview_active(false);
        true
    }

    pub(crate) fn retarget_overview(&mut self) {
        if !self.overview.is_active() {
            return;
        }

        let targets = self.overview_targets();
        self.overview.retarget(targets, self.animations_enabled());
    }

    pub(crate) fn presented_window_rect(&self, id: WindowId) -> Option<Rect> {
        let normal = self.window_geometry.get(&id)?.visual.current;

        Some(self.overview.presented_rect(id, normal))
    }

    pub(crate) fn inverse_presented_window_point(
        &self,
        id: WindowId,
        x: f64,
        y: f64,
    ) -> Option<(f64, f64)> {
        let geometry = self.window_geometry.get(&id)?;
        let source = geometry.client.committed_size?;
        let presented = self.overview.presented_rect(id, geometry.visual.current);

        if source.width <= 0
            || source.height <= 0
            || presented.width <= 0.0
            || presented.height <= 0.0
        {
            return None;
        }

        let scale_x = presented.width / f64::from(source.width);
        let scale_y = presented.height / f64::from(source.height);

        Some(((x - presented.x) / scale_x, (y - presented.y) / scale_y))
    }

    pub(crate) fn overview_selected(&self, id: WindowId) -> bool {
        self.overview.is_active() && self.overview.selected() == Some(id)
    }

    fn overview_targets(&self) -> HashMap<WindowId, (Rect, Rect)> {
        let mut targets = HashMap::new();

        for output in self.space.outputs() {
            let Some(output_id) = self.output_id(output) else {
                continue;
            };
            let Some(workspace) = self.output_workspaces.active_workspace(output_id) else {
                continue;
            };
            let Some(bounds) = self.output_bounds_for(output) else {
                continue;
            };
            let mut windows = self
                .window_ids
                .values()
                .filter(|id| self.workspaces.workspace_for_window(**id) == Some(workspace))
                .filter_map(|id| {
                    let normal = self.window_geometry.get(id)?.visual.current;
                    Some((*id, normal))
                })
                .collect::<Vec<_>>();

            windows.sort_by_key(|(id, _)| id.0);
            for (id, target) in overview_layout(bounds, &windows) {
                let normal = self.window_geometry[&id].visual.current;
                targets.insert(id, (normal, target));
            }
        }

        targets
    }

    fn normal_window_rects(&self) -> HashMap<WindowId, Rect> {
        self.window_geometry
            .iter()
            .map(|(id, geometry)| (*id, geometry.visual.current))
            .collect()
    }
}

fn overview_layout(bounds: Rect, windows: &[(WindowId, Rect)]) -> HashMap<WindowId, Rect> {
    if windows.is_empty() {
        return HashMap::new();
    }

    let content_width = (bounds.width - OVERVIEW_MARGIN * 2.0).max(1.0);
    let content_height = (bounds.height - OVERVIEW_MARGIN * 2.0).max(1.0);
    let aspect = (content_width / content_height).max(0.1);
    let columns = ((windows.len() as f64 * aspect).sqrt().ceil() as usize).clamp(1, windows.len());
    let rows = windows.len().div_ceil(columns);
    let cell_width = ((content_width - OVERVIEW_GAP * columns.saturating_sub(1) as f64)
        / columns as f64)
        .max(1.0);
    let cell_height =
        ((content_height - OVERVIEW_GAP * rows.saturating_sub(1) as f64) / rows as f64).max(1.0);
    let origin_x = bounds.x + OVERVIEW_MARGIN;
    let origin_y = bounds.y + OVERVIEW_MARGIN;

    windows
        .iter()
        .enumerate()
        .map(|(index, (id, source))| {
            let column = index % columns;
            let row = index / columns;
            let scale = (cell_width / source.width.max(1.0))
                .min(cell_height / source.height.max(1.0))
                .min(MAX_PREVIEW_SCALE);
            let width = (source.width * scale).max(1.0);
            let height = (source.height * scale).max(1.0);
            let cell_x = origin_x + column as f64 * (cell_width + OVERVIEW_GAP);
            let cell_y = origin_y + row as f64 * (cell_height + OVERVIEW_GAP);
            let target = Rect::new(
                cell_x + (cell_width - width) / 2.0,
                cell_y + (cell_height - height) / 2.0,
                width,
                height,
            );

            (*id, target)
        })
        .collect()
}

fn rect_center(rect: Rect) -> (f64, f64) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

fn directional_distance(
    direction: Direction,
    current: (f64, f64),
    candidate: (f64, f64),
) -> Option<f64> {
    let delta_x = candidate.0 - current.0;
    let delta_y = candidate.1 - current.1;
    let (primary, secondary) = match direction {
        Direction::Left if delta_x < 0.0 => (-delta_x, delta_y.abs()),
        Direction::Right if delta_x > 0.0 => (delta_x, delta_y.abs()),
        Direction::Up if delta_y < 0.0 => (-delta_y, delta_x.abs()),
        Direction::Down if delta_y > 0.0 => (delta_y, delta_x.abs()),
        _ => return None,
    };

    Some(primary + secondary * 0.35)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_grid_fits_windows_inside_the_output() {
        let bounds = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let windows = (1..=5)
            .map(|id| (WindowId(id), Rect::new(0.0, 0.0, 960.0, 1000.0)))
            .collect::<Vec<_>>();
        let layout = overview_layout(bounds, &windows);

        assert_eq!(layout.len(), windows.len());
        assert!(layout.values().all(|rect| {
            rect.x >= bounds.x
                && rect.y >= bounds.y
                && rect.x + rect.width <= bounds.x + bounds.width
                && rect.y + rect.height <= bounds.y + bounds.height
        }));
    }

    #[test]
    fn overview_grid_preserves_preview_aspect_ratio() {
        let source = Rect::new(0.0, 0.0, 1200.0, 800.0);
        let layout = overview_layout(
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            &[(WindowId(1), source)],
        );
        let preview = layout[&WindowId(1)];

        assert!((preview.width / preview.height - source.width / source.height).abs() < 0.001);
    }

    #[test]
    fn leaving_overview_keeps_presentations_until_they_settle() {
        let id = WindowId(1);
        let normal = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let target = Rect::new(200.0, 150.0, 500.0, 400.0);
        let mut overview = OverviewState::default();

        overview.enter(HashMap::from([(id, (normal, target))]), Some(id), false);
        overview.exit(HashMap::from([(id, normal)]), true);

        assert_eq!(overview.presented_rect(id, normal), target);
        assert!(!overview.is_active());
        assert!(overview.advance(Duration::from_millis(16), SpringConfig::default(), true));
    }

    #[test]
    fn directional_selection_uses_presented_geometry() {
        let left = WindowId(1);
        let right = WindowId(2);
        let left_rect = Rect::new(0.0, 0.0, 400.0, 400.0);
        let right_rect = Rect::new(500.0, 0.0, 400.0, 400.0);
        let mut overview = OverviewState::default();

        overview.enter(
            HashMap::from([
                (left, (left_rect, left_rect)),
                (right, (right_rect, right_rect)),
            ]),
            Some(left),
            false,
        );

        assert!(overview.select_direction(Direction::Right));
        assert_eq!(overview.selected(), Some(right));
        assert!(!overview.select_direction(Direction::Right));
    }
}
