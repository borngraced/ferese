use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use ferese_layout::{Axis, LayoutError, LayoutTree, Rect, WindowId};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceId(pub u64);

#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub layout: LayoutTree,
    pub floating: Vec<WindowId>,
    pub last_focused: Option<WindowId>,
    pub fullscreen: Option<WindowId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowPlacement {
    Tiled,
    Floating { rect: Rect },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceError {
    InvalidNumericName(u32),
    UnknownWorkspace(WorkspaceId),
    DuplicateWindow(WindowId),
    InvalidState(&'static str),
    Layout(LayoutError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNumericName(index) => {
                write!(formatter, "invalid workspace number {index}")
            }
            Self::UnknownWorkspace(workspace) => {
                write!(formatter, "unknown workspace {workspace:?}")
            }
            Self::DuplicateWindow(window) => write!(formatter, "duplicate window {window:?}"),
            Self::InvalidState(reason) => write!(formatter, "invalid workspace state: {reason}"),
            Self::Layout(error) => error.fmt(formatter),
        }
    }
}

impl Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Layout(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LayoutError> for WorkspaceError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

#[derive(Debug)]
pub struct WorkspaceSet {
    active: WorkspaceId,
    workspaces: HashMap<WorkspaceId, Workspace>,
    names: HashMap<String, WorkspaceId>,
    window_workspaces: HashMap<WindowId, WorkspaceId>,
    placements: HashMap<WindowId, WindowPlacement>,
    next_id: u64,
}

impl Default for WorkspaceSet {
    fn default() -> Self {
        let active = WorkspaceId(1);
        let workspace = Workspace {
            id: active,
            name: "1".to_owned(),
            layout: LayoutTree::default(),
            floating: Vec::new(),
            last_focused: None,
            fullscreen: None,
        };
        let workspaces = HashMap::from([(active, workspace)]);
        let names = HashMap::from([("1".to_owned(), active)]);

        Self {
            active,
            workspaces,
            names,
            window_workspaces: HashMap::new(),
            placements: HashMap::new(),
            next_id: 2,
        }
    }
}

impl WorkspaceSet {
    pub fn active_id(&self) -> WorkspaceId {
        self.active
    }

    pub fn active(&self) -> &Workspace {
        self.workspaces
            .get(&self.active)
            .expect("active workspace always exists")
    }

    pub fn active_mut(&mut self) -> &mut Workspace {
        self.workspaces
            .get_mut(&self.active)
            .expect("active workspace always exists")
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    pub fn workspace_for_window(&self, window: WindowId) -> Option<WorkspaceId> {
        self.window_workspaces.get(&window).copied()
    }

    pub fn placement(&self, window: WindowId) -> Option<WindowPlacement> {
        self.placements.get(&window).copied()
    }

    pub fn set_floating_rect(
        &mut self,
        window: WindowId,
        rect: Rect,
    ) -> Result<(), WorkspaceError> {
        let placement = self
            .placements
            .get_mut(&window)
            .ok_or(LayoutError::UnknownWindow(window))?;

        if !matches!(placement, WindowPlacement::Floating { .. }) {
            return Err(WorkspaceError::InvalidState("window is not floating"));
        }

        *placement = WindowPlacement::Floating {
            rect: normalized_floating_rect(rect),
        };

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn focus_window(&mut self, window: WindowId) -> Result<(), WorkspaceError> {
        if self.workspace_for_window(window) != Some(self.active) {
            return Err(WorkspaceError::InvalidState(
                "focused window is not on the active workspace",
            ));
        }
        if self
            .active()
            .fullscreen
            .is_some_and(|fullscreen| fullscreen != window)
        {
            return Err(WorkspaceError::InvalidState(
                "focused window is hidden by fullscreen",
            ));
        }

        if self.placement(window) == Some(WindowPlacement::Tiled) {
            self.active_mut().layout.activate_window(window)?;
        }
        self.active_mut().last_focused = Some(window);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn stack_window(
        &mut self,
        window: WindowId,
        target: WindowId,
    ) -> Result<(), WorkspaceError> {
        let workspace = self
            .workspace_for_window(window)
            .ok_or(LayoutError::UnknownWindow(window))?;

        if self.workspace_for_window(target) != Some(workspace) {
            return Err(WorkspaceError::InvalidState(
                "stack windows belong to different workspaces",
            ));
        }
        if self.placement(window) != Some(WindowPlacement::Tiled)
            || self.placement(target) != Some(WindowPlacement::Tiled)
        {
            return Err(WorkspaceError::InvalidState("stack window is not tiled"));
        }

        let workspace = self
            .workspaces
            .get_mut(&workspace)
            .ok_or(WorkspaceError::InvalidState("window workspace is missing"))?;
        workspace.layout.stack_window(window, target)?;
        workspace.last_focused = Some(window);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn ensure_numeric(&mut self, index: u32) -> Result<WorkspaceId, WorkspaceError> {
        if index == 0 {
            return Err(WorkspaceError::InvalidNumericName(index));
        }

        let name = index.to_string();

        if let Some(id) = self.names.get(&name) {
            return Ok(*id);
        }

        let id = WorkspaceId(self.next_id);
        self.next_id += 1;
        self.names.insert(name.clone(), id);
        self.workspaces.insert(
            id,
            Workspace {
                id,
                name,
                layout: LayoutTree::default(),
                floating: Vec::new(),
                last_focused: None,
                fullscreen: None,
            },
        );

        debug_assert!(self.validate().is_ok());
        Ok(id)
    }

    pub fn switch_to_numeric(&mut self, index: u32) -> Result<Option<WindowId>, WorkspaceError> {
        self.active = self.ensure_numeric(index)?;

        debug_assert!(self.validate().is_ok());
        Ok(self.active().fullscreen.or(self.active().last_focused))
    }

    pub fn insert_window(
        &mut self,
        window: WindowId,
        axis: Axis,
        ratio: f64,
    ) -> Result<(), WorkspaceError> {
        if self.window_workspaces.contains_key(&window) {
            return Err(WorkspaceError::DuplicateWindow(window));
        }

        let workspace = self.active_mut();
        workspace
            .layout
            .insert(window, workspace.last_focused, axis, ratio)?;
        if workspace.fullscreen.is_none() {
            workspace.last_focused = Some(window);
        }
        self.window_workspaces.insert(window, self.active);
        self.placements.insert(window, WindowPlacement::Tiled);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn insert_floating_window(
        &mut self,
        window: WindowId,
        workspace_id: WorkspaceId,
        rect: Rect,
        focus: bool,
    ) -> Result<(), WorkspaceError> {
        if self.window_workspaces.contains_key(&window) {
            return Err(WorkspaceError::DuplicateWindow(window));
        }

        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(WorkspaceError::UnknownWorkspace(workspace_id))?;
        workspace.floating.push(window);
        if focus && workspace.fullscreen.is_none() {
            workspace.last_focused = Some(window);
        }
        self.window_workspaces.insert(window, workspace_id);
        self.placements.insert(
            window,
            WindowPlacement::Floating {
                rect: normalized_floating_rect(rect),
            },
        );

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn remove_window(&mut self, window: WindowId) -> Result<(), WorkspaceError> {
        let workspace_id = self
            .window_workspaces
            .remove(&window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(WorkspaceError::UnknownWorkspace(workspace_id))?;

        match self.placements.remove(&window) {
            Some(WindowPlacement::Tiled) => workspace.layout.remove(window)?,
            Some(WindowPlacement::Floating { .. }) => {
                workspace.floating.retain(|candidate| *candidate != window);
            }
            None => return Err(LayoutError::UnknownWindow(window).into()),
        }

        if workspace.fullscreen == Some(window) {
            workspace.fullscreen = None;
        }

        if workspace.last_focused == Some(window) {
            workspace.last_focused = first_window(workspace);
        }

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn move_window_to_numeric(
        &mut self,
        window: WindowId,
        index: u32,
        axis: Axis,
        ratio: f64,
    ) -> Result<WorkspaceId, WorkspaceError> {
        let source_id = self
            .workspace_for_window(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let destination_id = self.ensure_numeric(index)?;

        if source_id == destination_id {
            return Ok(destination_id);
        }

        let placement = self
            .placements
            .get(&window)
            .copied()
            .ok_or(LayoutError::UnknownWindow(window))?;
        let source = self
            .workspaces
            .get_mut(&source_id)
            .ok_or(WorkspaceError::UnknownWorkspace(source_id))?;

        match placement {
            WindowPlacement::Tiled => source.layout.remove(window)?,
            WindowPlacement::Floating { .. } => {
                source.floating.retain(|candidate| *candidate != window);
            }
        }

        let was_fullscreen = source.fullscreen == Some(window);
        if was_fullscreen {
            source.fullscreen = None;
        }

        if source.last_focused == Some(window) {
            source.last_focused = first_window(source);
        }

        let destination = self
            .workspaces
            .get_mut(&destination_id)
            .ok_or(WorkspaceError::UnknownWorkspace(destination_id))?;
        match placement {
            WindowPlacement::Tiled => {
                let focused = tiled_focus(destination);
                destination.layout.insert(window, focused, axis, ratio)?;
            }
            WindowPlacement::Floating { .. } => destination.floating.push(window),
        }
        if was_fullscreen {
            destination.fullscreen = Some(window);
        }
        if destination.fullscreen.is_none() || was_fullscreen {
            destination.last_focused = Some(window);
        }
        self.window_workspaces.insert(window, destination_id);

        debug_assert!(self.validate().is_ok());
        Ok(destination_id)
    }

    pub fn toggle_floating(
        &mut self,
        window: WindowId,
        floating_rect: Rect,
        axis: Axis,
        ratio: f64,
    ) -> Result<WindowPlacement, WorkspaceError> {
        let workspace_id = self
            .workspace_for_window(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(WorkspaceError::UnknownWorkspace(workspace_id))?;
        let placement = self
            .placements
            .get_mut(&window)
            .ok_or(LayoutError::UnknownWindow(window))?;

        *placement = match *placement {
            WindowPlacement::Tiled => {
                workspace.layout.remove(window)?;
                workspace.floating.push(window);
                WindowPlacement::Floating {
                    rect: normalized_floating_rect(floating_rect),
                }
            }
            WindowPlacement::Floating { .. } => {
                workspace.floating.retain(|candidate| *candidate != window);
                let focused = tiled_focus(workspace);
                workspace.layout.insert(window, focused, axis, ratio)?;
                WindowPlacement::Tiled
            }
        };

        let result = *placement;

        debug_assert!(self.validate().is_ok());
        Ok(result)
    }

    pub fn toggle_fullscreen(&mut self, window: WindowId) -> Result<bool, WorkspaceError> {
        let enabled = self
            .workspace_for_window(window)
            .and_then(|workspace| self.workspaces.get(&workspace))
            .is_none_or(|workspace| workspace.fullscreen != Some(window));

        self.set_fullscreen(window, enabled)?;
        Ok(enabled)
    }

    pub fn set_fullscreen(
        &mut self,
        window: WindowId,
        enabled: bool,
    ) -> Result<bool, WorkspaceError> {
        let workspace_id = self
            .workspace_for_window(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(WorkspaceError::UnknownWorkspace(workspace_id))?;

        let changed = if enabled {
            let changed = workspace.fullscreen != Some(window);
            workspace.fullscreen = Some(window);
            workspace.last_focused = Some(window);
            changed
        } else if workspace.fullscreen == Some(window) {
            workspace.fullscreen = None;
            true
        } else {
            false
        };

        debug_assert!(self.validate().is_ok());
        Ok(changed)
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if !self.workspaces.contains_key(&self.active) {
            return Err(WorkspaceError::InvalidState("active workspace is missing"));
        }
        if self.names.len() != self.workspaces.len() {
            return Err(WorkspaceError::InvalidState(
                "workspace name index is inconsistent",
            ));
        }

        let mut seen_windows = HashSet::new();

        for (id, workspace) in &self.workspaces {
            if workspace.id != *id || self.names.get(&workspace.name) != Some(id) {
                return Err(WorkspaceError::InvalidState(
                    "workspace index is inconsistent",
                ));
            }

            workspace.layout.validate()?;

            for window in workspace.layout.window_ids() {
                if !seen_windows.insert(window)
                    || self.window_workspaces.get(&window) != Some(id)
                    || self.placements.get(&window) != Some(&WindowPlacement::Tiled)
                {
                    return Err(WorkspaceError::InvalidState(
                        "window ownership is inconsistent",
                    ));
                }
            }

            let mut seen_floating = HashSet::new();

            for window in &workspace.floating {
                if !seen_floating.insert(*window)
                    || !seen_windows.insert(*window)
                    || self.window_workspaces.get(window) != Some(id)
                    || !matches!(
                        self.placements.get(window),
                        Some(WindowPlacement::Floating { .. })
                    )
                {
                    return Err(WorkspaceError::InvalidState(
                        "floating window ownership is inconsistent",
                    ));
                }
            }

            if workspace
                .last_focused
                .is_some_and(|window| self.window_workspaces.get(&window) != Some(id))
            {
                return Err(WorkspaceError::InvalidState(
                    "workspace focus is inconsistent",
                ));
            }

            if workspace
                .fullscreen
                .is_some_and(|window| self.window_workspaces.get(&window) != Some(id))
            {
                return Err(WorkspaceError::InvalidState(
                    "fullscreen window ownership is inconsistent",
                ));
            }
        }

        if seen_windows.len() != self.window_workspaces.len()
            || self.placements.len() != self.window_workspaces.len()
        {
            return Err(WorkspaceError::InvalidState(
                "window index retains stale entries",
            ));
        }

        Ok(())
    }
}

fn first_window(workspace: &Workspace) -> Option<WindowId> {
    workspace
        .layout
        .window_ids()
        .chain(workspace.floating.iter().copied())
        .min_by_key(|window| window.0)
}

fn tiled_focus(workspace: &Workspace) -> Option<WindowId> {
    workspace
        .last_focused
        .filter(|window| workspace.layout.contains(*window))
        .or_else(|| workspace.layout.window_ids().min_by_key(|window| window.0))
}

fn normalized_floating_rect(rect: Rect) -> Rect {
    Rect::new(
        finite_or_zero(rect.x),
        finite_or_zero(rect.y),
        finite_or_zero(rect.width).max(1.0),
        finite_or_zero(rect.height).max(1.0),
    )
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_workspaces_are_created_lazily_with_stable_ids() {
        let mut workspaces = WorkspaceSet::default();
        let first = workspaces.active_id();
        let ninth = workspaces.ensure_numeric(9).unwrap();

        assert_eq!(workspaces.ensure_numeric(1), Ok(first));
        assert_eq!(workspaces.ensure_numeric(9), Ok(ninth));
        assert_ne!(first, ninth);
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn switching_restores_workspace_focus() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.switch_to_numeric(2).unwrap(), None);
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.switch_to_numeric(1).unwrap(), Some(WindowId(1)));
        assert_eq!(workspaces.switch_to_numeric(2).unwrap(), Some(WindowId(2)));
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn moving_a_window_updates_membership_without_switching() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();
        let first = workspaces.active_id();
        let second = workspaces
            .move_window_to_numeric(WindowId(2), 2, Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.active_id(), first);
        assert_eq!(workspaces.workspace_for_window(WindowId(2)), Some(second));
        assert!(
            !workspaces
                .workspace(first)
                .unwrap()
                .layout
                .contains(WindowId(2))
        );
        assert!(
            workspaces
                .workspace(second)
                .unwrap()
                .layout
                .contains(WindowId(2))
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn removing_focus_chooses_a_deterministic_remaining_window() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces.remove_window(WindowId(1)).unwrap();

        assert_eq!(workspaces.active().last_focused, Some(WindowId(2)));
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn focus_rejects_a_window_on_an_inactive_workspace() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .move_window_to_numeric(WindowId(1), 2, Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(
            workspaces.focus_window(WindowId(1)),
            Err(WorkspaceError::InvalidState(
                "focused window is not on the active workspace"
            ))
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn floating_toggle_removes_and_restores_tiled_membership() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        let rect = Rect::new(10.0, 20.0, 640.0, 480.0);

        assert_eq!(
            workspaces
                .toggle_floating(WindowId(1), rect, Axis::Horizontal, 0.5)
                .unwrap(),
            WindowPlacement::Floating { rect }
        );
        assert!(!workspaces.active().layout.contains(WindowId(1)));
        assert_eq!(workspaces.active().floating, vec![WindowId(1)]);

        assert_eq!(
            workspaces
                .toggle_floating(WindowId(1), rect, Axis::Horizontal, 0.5)
                .unwrap(),
            WindowPlacement::Tiled
        );
        assert!(workspaces.active().layout.contains(WindowId(1)));
        assert!(workspaces.active().floating.is_empty());
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn floating_geometry_updates_are_authoritative_and_validated() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        assert_eq!(
            workspaces.set_floating_rect(WindowId(1), Rect::default()),
            Err(WorkspaceError::InvalidState("window is not floating"))
        );

        workspaces
            .toggle_floating(
                WindowId(1),
                Rect::new(10.0, 20.0, 640.0, 480.0),
                Axis::Horizontal,
                0.5,
            )
            .unwrap();
        workspaces
            .set_floating_rect(WindowId(1), Rect::new(30.0, 40.0, f64::NAN, -2.0))
            .unwrap();

        assert_eq!(
            workspaces.placement(WindowId(1)),
            Some(WindowPlacement::Floating {
                rect: Rect::new(30.0, 40.0, 1.0, 1.0)
            })
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn floating_window_can_follow_its_parent_workspace_without_focus() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        let parent_workspace = workspaces.active_id();
        workspaces.switch_to_numeric(2).unwrap();
        workspaces
            .insert_floating_window(
                WindowId(2),
                parent_workspace,
                Rect::new(20.0, 30.0, 400.0, 300.0),
                false,
            )
            .unwrap();

        assert_eq!(
            workspaces.workspace_for_window(WindowId(2)),
            Some(parent_workspace)
        );
        assert_eq!(
            workspaces.workspace(parent_workspace).unwrap().last_focused,
            Some(WindowId(1))
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn fullscreen_preserves_underlying_placement() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        let before = workspaces.placement(WindowId(1));

        assert!(workspaces.toggle_fullscreen(WindowId(1)).unwrap());
        assert_eq!(workspaces.active().fullscreen, Some(WindowId(1)));
        assert_eq!(workspaces.placement(WindowId(1)), before);
        assert!(!workspaces.set_fullscreen(WindowId(1), true).unwrap());

        assert!(!workspaces.toggle_fullscreen(WindowId(1)).unwrap());
        assert_eq!(workspaces.active().fullscreen, None);
        assert_eq!(workspaces.placement(WindowId(1)), before);
        assert!(!workspaces.set_fullscreen(WindowId(1), false).unwrap());
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn mapping_under_fullscreen_does_not_change_focus() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces.toggle_fullscreen(WindowId(1)).unwrap();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.active().last_focused, Some(WindowId(1)));
        assert_eq!(workspaces.active().fullscreen, Some(WindowId(1)));
        assert!(workspaces.active().layout.contains(WindowId(2)));
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn switching_to_fullscreen_restores_the_visible_window() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces.toggle_fullscreen(WindowId(1)).unwrap();
        workspaces.switch_to_numeric(2).unwrap();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .move_window_to_numeric(WindowId(2), 1, Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.switch_to_numeric(1).unwrap(), Some(WindowId(1)));
        assert_eq!(workspaces.active().last_focused, Some(WindowId(1)));
        assert_eq!(
            workspaces.focus_window(WindowId(2)),
            Err(WorkspaceError::InvalidState(
                "focused window is hidden by fullscreen"
            ))
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn stacking_tiled_windows_preserves_workspace_membership() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces.stack_window(WindowId(2), WindowId(1)).unwrap();

        assert_eq!(workspaces.active().last_focused, Some(WindowId(2)));
        assert_eq!(
            workspaces
                .active()
                .layout
                .geometry(Rect::new(0.0, 0.0, 100.0, 80.0))
                .unwrap()
                .len(),
            1
        );
        assert!(workspaces.validate().is_ok());
    }
}
