use std::collections::{HashMap, HashSet};

use crate::{
    ConstraintKind, ConstraintWarning, GapConfig, LayoutError, LayoutResult, Rect, SizeConstraints,
    WindowId, normalized_constraints, record_minimum_warnings,
};

const DEFAULT_WIDTH: f64 = 0.5;

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
    fn new(window: WindowId) -> Self {
        Self {
            windows: vec![window],
            active: 0,
            width: ColumnWidth::default(),
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
}

impl Default for ScrollingLayout {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            active_column: None,
            viewport_x: 0.0,
            neighbor_context: 48.0,
        }
    }
}

impl ScrollingLayout {
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
        self.columns.insert(index, Column::new(window));
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
        self.columns.insert(insertion, Column::new(window));
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
                ColumnWidth::Proportion(proportion) => viewport_width * proportion,
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

    fn window_location(&self, window: WindowId) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(column, data)| {
            data.windows
                .iter()
                .position(|candidate| *candidate == window)
                .map(|index| (column, index))
        })
    }

    fn validate(&self) -> Result<(), LayoutError> {
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
}
