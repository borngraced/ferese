use std::collections::{HashMap, HashSet};

use crate::{
    ConstraintKind, ConstraintWarning, Direction, GapConfig, LayoutError, LayoutResult, Rect,
    SizeConstraints, WindowId, normalized_constraints, record_minimum_warnings,
};

const DEFAULT_WIDTH: f64 = 0.5;
const MIN_COLUMN_WIDTH: f64 = 0.1;
const MAX_COLUMN_WIDTH: f64 = 2.0;
const MIN_ROW_HEIGHT: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    Proportion(f64),
    Fixed(f64),
    Full,
}

impl Default for ColumnWidth {
    fn default() -> Self {
        Self::Proportion(DEFAULT_WIDTH)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub windows: Vec<WindowId>,
    pub active: usize,
    pub width: ColumnWidth,
    pub heights: Vec<f64>,
}

impl Column {
    fn new(window: WindowId, width: ColumnWidth) -> Self {
        Self {
            windows: vec![window],
            active: 0,
            width: normalized_width(width),
            heights: vec![1.0],
        }
    }

    fn normalize_heights(&mut self) {
        if self.windows.is_empty() {
            self.heights.clear();
            return;
        }
        if self.heights.len() != self.windows.len() {
            self.heights.resize(self.windows.len(), 1.0);
        }
        self.heights.iter_mut().for_each(|height| {
            if !height.is_finite() || *height <= 0.0 {
                *height = 1.0;
            }
        });
        let total = self.heights.iter().sum::<f64>();
        self.heights.iter_mut().for_each(|height| *height /= total);
        self.active = self.active.min(self.windows.len() - 1);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScrollingLayout {
    columns: Vec<Column>,
    active_column: Option<usize>,
    viewport_x: f64,
    neighbor_context: f64,
    default_width: ColumnWidth,
}

impl Default for ScrollingLayout {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            active_column: None,
            viewport_x: 0.0,
            neighbor_context: 48.0,
            default_width: ColumnWidth::default(),
        }
    }
}

impl ScrollingLayout {
    pub fn with_default_width(default_width: ColumnWidth) -> Self {
        Self {
            default_width: normalized_width(default_width),
            ..Self::default()
        }
    }

    pub fn default_width(&self) -> ColumnWidth {
        self.default_width
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    pub fn active_column(&self) -> Option<usize> {
        self.active_column
    }

    pub fn viewport_x(&self) -> f64 {
        self.viewport_x
    }

    pub fn contains(&self, window: WindowId) -> bool {
        self.window_location(window).is_some()
    }

    pub fn window_ids(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.columns
            .iter()
            .flat_map(|column| column.windows.iter().copied())
    }

    pub fn insert(
        &mut self,
        window: WindowId,
        focused: Option<WindowId>,
    ) -> Result<(), LayoutError> {
        if self.contains(window) {
            return Err(LayoutError::DuplicateWindow(window));
        }

        let index = focused
            .and_then(|focused| self.window_location(focused))
            .map(|(column, _)| column + 1)
            .unwrap_or(self.columns.len());
        self.columns
            .insert(index, Column::new(window, self.default_width));
        self.active_column = Some(index);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn remove(&mut self, window: WindowId) -> Result<(), LayoutError> {
        let (column_index, window_index) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let column = &mut self.columns[column_index];
        column.windows.remove(window_index);
        column.heights.remove(window_index);

        if column.windows.is_empty() {
            self.columns.remove(column_index);
            self.active_column = match self.active_column {
                None => None,
                Some(_) if self.columns.is_empty() => None,
                Some(active) if active > column_index => Some(active - 1),
                Some(active) => Some(active.min(self.columns.len() - 1)),
            };
        } else {
            column.active = column.active.min(column.windows.len() - 1);
            column.normalize_heights();
        }

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn focus(&mut self, window: WindowId) -> Result<(), LayoutError> {
        let (column, index) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        self.active_column = Some(column);
        self.columns[column].active = index;
        Ok(())
    }

    pub fn move_into_column(
        &mut self,
        window: WindowId,
        target: WindowId,
    ) -> Result<(), LayoutError> {
        if window == target {
            return Ok(());
        }
        self.window_location(target)
            .ok_or(LayoutError::UnknownWindow(target))?;
        self.remove(window)?;
        let (target_column, target_index) = self
            .window_location(target)
            .expect("removing another window preserves the target");
        let column = &mut self.columns[target_column];
        let insertion = (target_index + 1).min(column.windows.len());
        column.windows.insert(insertion, window);
        column.heights.insert(insertion, 1.0);
        column.active = insertion;
        column.normalize_heights();
        self.active_column = Some(target_column);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn extract_to_column(&mut self, window: WindowId) -> Result<(), LayoutError> {
        let (source, _) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        if self.columns[source].windows.len() == 1 {
            self.active_column = Some(source);
            return Ok(());
        }

        self.remove(window)?;
        let insertion = (source + 1).min(self.columns.len());
        self.columns
            .insert(insertion, Column::new(window, self.default_width));
        self.active_column = Some(insertion);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn set_column_width(
        &mut self,
        window: WindowId,
        width: ColumnWidth,
    ) -> Result<(), LayoutError> {
        let (column, _) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        self.columns[column].width = normalized_width(width);
        Ok(())
    }

    pub fn cycle_column_width(
        &mut self,
        window: WindowId,
        presets: &[ColumnWidth],
    ) -> Result<bool, LayoutError> {
        let (column, _) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let presets = presets
            .iter()
            .copied()
            .map(normalized_width)
            .collect::<Vec<_>>();
        if presets.is_empty() {
            return Ok(false);
        }

        let current = normalized_width(self.columns[column].width);
        let next = presets
            .iter()
            .position(|preset| *preset == current)
            .map(|index| presets[(index + 1) % presets.len()])
            .unwrap_or(presets[0]);
        self.columns[column].width = next;
        Ok(next != current)
    }

    pub fn center_window(
        &mut self,
        window: WindowId,
        bounds: Rect,
        gaps: GapConfig,
        constraints: &HashMap<WindowId, SizeConstraints>,
    ) -> Result<bool, LayoutError> {
        let before = self.viewport_x;
        let result = self.geometry_with_constraints(bounds, gaps, constraints, Some(window))?;
        let rect = result
            .geometry
            .get(&window)
            .copied()
            .ok_or(LayoutError::UnknownWindow(window))?;
        let viewport_center = bounds.x + bounds.width / 2.0;
        let window_center = rect.x + rect.width / 2.0;
        self.viewport_x += window_center - viewport_center;
        self.geometry_with_constraints(bounds, gaps, constraints, Some(window))?;

        Ok(self.viewport_x != before)
    }

    pub fn directional_neighbor(
        &self,
        window: WindowId,
        direction: Direction,
    ) -> Result<Option<WindowId>, LayoutError> {
        let (column, row) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let neighbor = match direction {
            Direction::Up => row
                .checked_sub(1)
                .map(|row| self.columns[column].windows[row]),
            Direction::Down => self.columns[column].windows.get(row + 1).copied(),
            Direction::Left => column.checked_sub(1).map(|column| {
                let column = &self.columns[column];
                column.windows[column.active]
            }),
            Direction::Right => self
                .columns
                .get(column + 1)
                .map(|column| column.windows[column.active]),
        };

        Ok(neighbor)
    }

    pub fn move_window(
        &mut self,
        window: WindowId,
        direction: Direction,
    ) -> Result<bool, LayoutError> {
        let (column, row) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;

        match direction {
            Direction::Up if row > 0 => {
                self.columns[column].windows.swap(row, row - 1);
                self.columns[column].heights.swap(row, row - 1);
                self.columns[column].active = row - 1;
            }
            Direction::Down if row + 1 < self.columns[column].windows.len() => {
                self.columns[column].windows.swap(row, row + 1);
                self.columns[column].heights.swap(row, row + 1);
                self.columns[column].active = row + 1;
            }
            Direction::Left if column > 0 => self.move_horizontally(window, column - 1)?,
            Direction::Right if column + 1 < self.columns.len() => {
                self.move_horizontally(window, column + 1)?
            }
            _ => return Ok(false),
        }

        debug_assert!(self.validate().is_ok());
        Ok(true)
    }

    pub fn resize_window(
        &mut self,
        window: WindowId,
        direction: Direction,
        amount: f64,
    ) -> Result<bool, LayoutError> {
        let (column, row) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let amount = if amount.is_finite() {
            amount.abs()
        } else {
            0.0
        };
        if amount == 0.0 {
            return Ok(false);
        }

        let changed = match direction {
            Direction::Left | Direction::Right => {
                let current = match normalized_width(self.columns[column].width) {
                    ColumnWidth::Proportion(value) => value,
                    ColumnWidth::Fixed(_) | ColumnWidth::Full => DEFAULT_WIDTH,
                };
                let adjustment = if direction == Direction::Right {
                    amount
                } else {
                    -amount
                };
                let resized = (current + adjustment).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
                self.columns[column].width = ColumnWidth::Proportion(resized);
                resized != current
            }
            Direction::Up if row > 0 => {
                resize_rows(&mut self.columns[column], row, row - 1, amount)
            }
            Direction::Down if row + 1 < self.columns[column].windows.len() => {
                resize_rows(&mut self.columns[column], row, row + 1, amount)
            }
            Direction::Up | Direction::Down => false,
        };

        debug_assert!(self.validate().is_ok());
        Ok(changed)
    }

    pub fn geometry_with_constraints(
        &mut self,
        bounds: Rect,
        gaps: GapConfig,
        constraints: &HashMap<WindowId, SizeConstraints>,
        focused: Option<WindowId>,
    ) -> Result<LayoutResult, LayoutError> {
        if let Some(focused) = focused {
            self.focus(focused)?;
        }
        self.validate()?;
        if self.columns.is_empty() {
            self.viewport_x = 0.0;
            return Ok(LayoutResult::default());
        }

        let window_count = self.window_ids().count();
        let inner = finite_nonnegative(gaps.inner);
        let outer = if gaps.smart && window_count == 1 {
            0.0
        } else {
            finite_nonnegative(gaps.outer)
        };
        let viewport_width = (bounds.width - outer * 2.0).max(1.0);
        let viewport_height = (bounds.height - outer * 2.0).max(1.0);
        let mut column_positions = Vec::with_capacity(self.columns.len());
        let mut strip_width = 0.0;

        for column in &self.columns {
            let requested = match normalized_width(column.width) {
                ColumnWidth::Proportion(proportion) => {
                    ((viewport_width + inner) * proportion - inner).max(1.0)
                }
                ColumnWidth::Fixed(width) => width,
                ColumnWidth::Full => viewport_width,
            };
            let minimum = column
                .windows
                .iter()
                .map(|window| {
                    constraints
                        .get(window)
                        .copied()
                        .map(normalized_constraints)
                        .unwrap_or_default()
                        .min_width
                })
                .fold(1.0_f64, f64::max);
            let width = requested.max(minimum).max(1.0);
            column_positions.push((strip_width, width));
            strip_width += width + inner;
        }
        strip_width = (strip_width - inner).max(0.0);
        self.reveal_active_column(&column_positions, strip_width, viewport_width);

        let mut result = LayoutResult::default();
        for (column_index, column) in self.columns.iter().enumerate() {
            let (column_x, column_width) = column_positions[column_index];
            let available_height = (viewport_height
                - inner * (column.windows.len().saturating_sub(1) as f64))
                .max(1.0);
            let mut y = bounds.y + outer;

            for (window_index, window) in column.windows.iter().enumerate() {
                let mut rect = Rect::new(
                    bounds.x + outer + column_x - self.viewport_x,
                    y,
                    column_width,
                    available_height * column.heights[window_index],
                );
                let constraint = constraints
                    .get(window)
                    .copied()
                    .map(normalized_constraints)
                    .unwrap_or_default();
                record_minimum_warnings(*window, rect, constraint, &mut result.warnings);
                apply_maximums(*window, &mut rect, constraint, &mut result.warnings);
                y += available_height * column.heights[window_index] + inner;
                result.geometry.insert(*window, rect);
            }
        }

        Ok(result)
    }

    fn reveal_active_column(
        &mut self,
        positions: &[(f64, f64)],
        strip_width: f64,
        viewport_width: f64,
    ) {
        let max_scroll = (strip_width - viewport_width).max(0.0);
        self.viewport_x = finite_nonnegative(self.viewport_x).min(max_scroll);
        let Some(active) = self.active_column else {
            return;
        };
        let (start, width) = positions[active];
        let end = start + width;
        let context = finite_nonnegative(self.neighbor_context).min(viewport_width / 3.0);

        if start < self.viewport_x + context {
            self.viewport_x = (start - context).max(0.0);
        } else if end > self.viewport_x + viewport_width - context {
            self.viewport_x = (end - viewport_width + context).min(max_scroll);
        }
    }

    fn move_horizontally(
        &mut self,
        window: WindowId,
        destination: usize,
    ) -> Result<(), LayoutError> {
        let (source, _) = self
            .window_location(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        if self.columns[source].windows.len() == 1 {
            self.columns.swap(source, destination);
            self.active_column = Some(destination);
            return Ok(());
        }

        let insertion = if destination < source {
            source
        } else {
            source + 1
        };
        self.remove(window)?;
        let insertion = insertion.min(self.columns.len());
        self.columns
            .insert(insertion, Column::new(window, self.default_width));
        self.active_column = Some(insertion);
        Ok(())
    }

    fn window_location(&self, window: WindowId) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(column, data)| {
            data.windows
                .iter()
                .position(|candidate| *candidate == window)
                .map(|index| (column, index))
        })
    }

    pub fn validate(&self) -> Result<(), LayoutError> {
        if !self.viewport_x.is_finite() || self.viewport_x < 0.0 {
            return Err(LayoutError::InvalidTree(
                "scrolling viewport offset is invalid",
            ));
        }
        if normalized_width(self.default_width) != self.default_width {
            return Err(LayoutError::InvalidTree(
                "scrolling default column width is invalid",
            ));
        }
        if self.columns.is_empty() {
            return if self.active_column.is_none() {
                Ok(())
            } else {
                Err(LayoutError::InvalidTree(
                    "empty scrolling layout has an active column",
                ))
            };
        }
        if self
            .active_column
            .is_none_or(|active| active >= self.columns.len())
        {
            return Err(LayoutError::InvalidTree(
                "scrolling active column is invalid",
            ));
        }

        let mut windows = HashSet::new();
        for column in &self.columns {
            if normalized_width(column.width) != column.width {
                return Err(LayoutError::InvalidTree(
                    "scrolling column width is invalid",
                ));
            }
            if column.windows.is_empty() {
                return Err(LayoutError::InvalidTree("scrolling column is empty"));
            }
            if column.active >= column.windows.len() || column.heights.len() != column.windows.len()
            {
                return Err(LayoutError::InvalidTree(
                    "scrolling column metadata is invalid",
                ));
            }
            if !column
                .heights
                .iter()
                .all(|height| height.is_finite() && *height > 0.0)
            {
                return Err(LayoutError::InvalidTree("scrolling height is invalid"));
            }
            for window in &column.windows {
                if !windows.insert(*window) {
                    return Err(LayoutError::DuplicateWindow(*window));
                }
            }
        }
        Ok(())
    }
}

fn resize_rows(column: &mut Column, row: usize, neighbor: usize, amount: f64) -> bool {
    let transferable = (column.heights[neighbor] - MIN_ROW_HEIGHT).max(0.0);
    let adjustment = amount.min(transferable);
    if adjustment == 0.0 {
        return false;
    }

    column.heights[row] += adjustment;
    column.heights[neighbor] -= adjustment;
    true
}

fn normalized_width(width: ColumnWidth) -> ColumnWidth {
    match width {
        ColumnWidth::Proportion(value) if value.is_finite() && value > 0.0 => {
            ColumnWidth::Proportion(value)
        }
        ColumnWidth::Fixed(value) if value.is_finite() && value > 0.0 => ColumnWidth::Fixed(value),
        ColumnWidth::Full => ColumnWidth::Full,
        _ => ColumnWidth::default(),
    }
}

fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn apply_maximums(
    window: WindowId,
    rect: &mut Rect,
    constraints: SizeConstraints,
    warnings: &mut Vec<ConstraintWarning>,
) {
    if let Some(maximum) = constraints.max_width
        && rect.width > maximum
    {
        warnings.push(ConstraintWarning {
            window,
            kind: ConstraintKind::MaximumWidth,
            requested: maximum,
            assigned: rect.width,
        });
        rect.width = maximum;
    }
    if let Some(maximum) = constraints.max_height
        && rect.height > maximum
    {
        warnings.push(ConstraintWarning {
            window,
            kind: ConstraintKind::MaximumHeight,
            requested: maximum,
            assigned: rect.height,
        });
        rect.height = maximum;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64) -> WindowId {
        WindowId(id)
    }

    #[test]
    fn inserts_new_columns_after_focus() {
        let mut layout = ScrollingLayout::default();
        layout.insert(window(1), None).unwrap();
        layout.insert(window(2), Some(window(1))).unwrap();
        layout.insert(window(3), Some(window(1))).unwrap();

        assert_eq!(
            layout.window_ids().collect::<Vec<_>>(),
            vec![window(1), window(3), window(2)]
        );
        assert_eq!(layout.active_column(), Some(1));
    }

    #[test]
    fn configured_default_width_is_applied_to_new_columns() {
        let mut layout = ScrollingLayout::with_default_width(ColumnWidth::Full);
        layout.insert(window(1), None).unwrap();
        layout.insert(window(2), Some(window(1))).unwrap();

        assert_eq!(layout.default_width(), ColumnWidth::Full);
        assert!(
            layout
                .columns()
                .iter()
                .all(|column| column.width == ColumnWidth::Full)
        );
    }

    #[test]
    fn focus_reveal_scrolls_only_as_far_as_needed() {
        let mut layout = ScrollingLayout::default();
        for id in 1..=4 {
            layout
                .insert(window(id), Some(window(id.saturating_sub(1))))
                .unwrap();
        }
        let bounds = Rect::new(0.0, 0.0, 1_000.0, 800.0);
        let gaps = GapConfig {
            inner: 0.0,
            outer: 0.0,
            smart: false,
        };

        let result = layout
            .geometry_with_constraints(bounds, gaps, &HashMap::new(), Some(window(4)))
            .unwrap();

        assert_eq!(layout.viewport_x(), 1_000.0);
        assert_eq!(result.geometry[&window(4)].x, 500.0);
    }

    #[test]
    fn two_half_width_columns_fit_without_focus_scrolling() {
        let mut layout = ScrollingLayout::default();
        layout.insert(window(1), None).unwrap();
        layout.insert(window(2), Some(window(1))).unwrap();
        let bounds = Rect::new(0.0, 0.0, 1_000.0, 800.0);
        let gaps = GapConfig {
            inner: 10.0,
            outer: 10.0,
            smart: false,
        };

        let first = layout
            .geometry_with_constraints(bounds, gaps, &HashMap::new(), Some(window(1)))
            .unwrap();
        assert_eq!(layout.viewport_x(), 0.0);
        let second = layout
            .geometry_with_constraints(bounds, gaps, &HashMap::new(), Some(window(2)))
            .unwrap();

        assert_eq!(layout.viewport_x(), 0.0);
        assert_eq!(first.geometry, second.geometry);
        assert_eq!(
            second.geometry[&window(1)],
            Rect::new(10.0, 10.0, 485.0, 780.0)
        );
        assert_eq!(
            second.geometry[&window(2)],
            Rect::new(505.0, 10.0, 485.0, 780.0)
        );
    }

    #[test]
    fn grouping_and_extracting_preserve_membership() {
        let mut layout = ScrollingLayout::default();
        layout.insert(window(1), None).unwrap();
        layout.insert(window(2), Some(window(1))).unwrap();
        layout.move_into_column(window(2), window(1)).unwrap();

        assert_eq!(layout.columns().len(), 1);
        assert_eq!(layout.columns()[0].windows, vec![window(1), window(2)]);

        layout.extract_to_column(window(2)).unwrap();
        assert_eq!(layout.columns().len(), 2);
        assert_eq!(
            layout.window_ids().collect::<HashSet<_>>(),
            HashSet::from([window(1), window(2)])
        );
    }

    #[test]
    fn later_column_changes_do_not_move_the_first_column() {
        let mut layout = ScrollingLayout::default();
        layout.insert(window(1), None).unwrap();
        let bounds = Rect::new(0.0, 0.0, 1_000.0, 800.0);
        let gaps = GapConfig {
            smart: false,
            ..GapConfig::default()
        };
        let first = layout
            .geometry_with_constraints(bounds, gaps, &HashMap::new(), Some(window(1)))
            .unwrap()
            .geometry[&window(1)];

        layout.insert(window(2), Some(window(1))).unwrap();
        let second = layout
            .geometry_with_constraints(bounds, gaps, &HashMap::new(), Some(window(1)))
            .unwrap()
            .geometry[&window(1)];

        assert_eq!(first.x, second.x);
        assert_eq!(first.width, second.width);
    }

    #[test]
    fn directional_navigation_follows_columns_and_rows() {
        let mut layout = ScrollingLayout::default();
        layout.insert(window(1), None).unwrap();
        layout.insert(window(2), Some(window(1))).unwrap();
        layout.insert(window(3), Some(window(2))).unwrap();
        layout.move_into_column(window(3), window(2)).unwrap();

        assert_eq!(
            layout.directional_neighbor(window(1), Direction::Right),
            Ok(Some(window(3)))
        );
        assert_eq!(
            layout.directional_neighbor(window(3), Direction::Up),
            Ok(Some(window(2)))
        );
        assert_eq!(
            layout.directional_neighbor(window(2), Direction::Down),
            Ok(Some(window(3)))
        );
    }

    #[test]
    fn vertical_moves_reorder_rows_and_horizontal_moves_extract_them() {
        let mut layout = ScrollingLayout::default();
        layout.insert(window(1), None).unwrap();
        layout.insert(window(2), Some(window(1))).unwrap();
        layout.insert(window(3), Some(window(2))).unwrap();
        layout.move_into_column(window(3), window(2)).unwrap();

        assert!(layout.move_window(window(3), Direction::Up).unwrap());
        assert_eq!(layout.columns()[1].windows, vec![window(3), window(2)]);
        assert!(layout.move_window(window(3), Direction::Left).unwrap());
        assert_eq!(layout.columns().len(), 3);
        assert_eq!(
            layout.window_ids().collect::<Vec<_>>(),
            vec![window(1), window(3), window(2)]
        );
    }

    #[test]
    fn resize_changes_column_width_and_neighboring_row_weights() {
        let mut layout = ScrollingLayout::default();
        layout.insert(window(1), None).unwrap();
        layout.insert(window(2), Some(window(1))).unwrap();
        layout.move_into_column(window(2), window(1)).unwrap();

        assert!(
            layout
                .resize_window(window(1), Direction::Right, 0.1)
                .unwrap()
        );
        assert_eq!(layout.columns()[0].width, ColumnWidth::Proportion(0.6));

        assert!(
            layout
                .resize_window(window(1), Direction::Down, 0.1)
                .unwrap()
        );
        assert_eq!(layout.columns()[0].heights, vec![0.6, 0.4]);
    }

    #[test]
    fn width_cycle_wraps_through_configured_presets() {
        let mut layout = ScrollingLayout::default();
        layout.insert(window(1), None).unwrap();
        let presets = [
            ColumnWidth::Proportion(0.5),
            ColumnWidth::Proportion(2.0 / 3.0),
            ColumnWidth::Full,
        ];

        assert!(layout.cycle_column_width(window(1), &presets).unwrap());
        assert_eq!(
            layout.columns()[0].width,
            ColumnWidth::Proportion(2.0 / 3.0)
        );
        assert!(layout.cycle_column_width(window(1), &presets).unwrap());
        assert_eq!(layout.columns()[0].width, ColumnWidth::Full);
        assert!(layout.cycle_column_width(window(1), &presets).unwrap());
        assert_eq!(layout.columns()[0].width, ColumnWidth::Proportion(0.5));
    }

    #[test]
    fn explicit_center_places_a_middle_column_in_the_viewport_center() {
        let mut layout = ScrollingLayout::default();
        for id in 1_u64..=4 {
            layout
                .insert(
                    window(id),
                    id.checked_sub(1).filter(|id| *id > 0).map(window),
                )
                .unwrap();
        }
        let bounds = Rect::new(0.0, 0.0, 1_000.0, 800.0);
        let gaps = GapConfig {
            inner: 0.0,
            outer: 0.0,
            smart: false,
        };
        layout
            .geometry_with_constraints(bounds, gaps, &HashMap::new(), Some(window(4)))
            .unwrap();

        assert!(
            layout
                .center_window(window(2), bounds, gaps, &HashMap::new())
                .unwrap()
        );
        let result = layout
            .geometry_with_constraints(bounds, gaps, &HashMap::new(), Some(window(2)))
            .unwrap();
        let rect = result.geometry[&window(2)];

        assert_eq!(rect.x + rect.width / 2.0, 500.0);
    }

    #[test]
    fn randomized_scrolling_operations_preserve_invariants() {
        let mut layout = ScrollingLayout::default();
        let mut windows = Vec::new();
        let mut next_window = 1_u64;
        let mut random = 0x5c40_11ab_u64;

        for _ in 0..2_000 {
            random = random
                .wrapping_mul(2_862_933_555_777_941_757)
                .wrapping_add(3_037_000_493);

            match random % 7 {
                0 if windows.len() < 48 => {
                    let new_window = window(next_window);
                    next_window += 1;
                    let focused = windows.get(random as usize % windows.len().max(1)).copied();
                    layout.insert(new_window, focused).unwrap();
                    windows.push(new_window);
                }
                1 if !windows.is_empty() => {
                    let index = random as usize % windows.len();
                    layout.remove(windows.swap_remove(index)).unwrap();
                }
                2 if windows.len() > 1 => {
                    let first = random as usize % windows.len();
                    let mut second = random.rotate_left(17) as usize % windows.len();
                    if first == second {
                        second = (second + 1) % windows.len();
                    }
                    layout
                        .move_into_column(windows[first], windows[second])
                        .unwrap();
                }
                3 if !windows.is_empty() => {
                    let index = random as usize % windows.len();
                    layout.extract_to_column(windows[index]).unwrap();
                }
                4 if !windows.is_empty() => {
                    let index = random as usize % windows.len();
                    let direction = random_direction(random.rotate_left(9));
                    layout.move_window(windows[index], direction).unwrap();
                }
                5 if !windows.is_empty() => {
                    let index = random as usize % windows.len();
                    let direction = random_direction(random.rotate_left(23));
                    layout
                        .resize_window(windows[index], direction, 0.05)
                        .unwrap();
                }
                6 if !windows.is_empty() => {
                    let index = random as usize % windows.len();
                    layout.focus(windows[index]).unwrap();
                }
                _ => {}
            }

            layout.validate().unwrap();
            assert_eq!(
                layout.window_ids().collect::<HashSet<_>>().len(),
                windows.len()
            );
        }
    }

    fn random_direction(random: u64) -> Direction {
        match random % 4 {
            0 => Direction::Left,
            1 => Direction::Right,
            2 => Direction::Up,
            _ => Direction::Down,
        }
    }
}
