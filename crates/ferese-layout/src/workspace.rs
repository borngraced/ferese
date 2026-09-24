use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{Axis, LayoutError, LayoutTree, WindowId};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceId(pub u64);

#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub layout: LayoutTree,
    pub last_focused: Option<WindowId>,
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
    next_id: u64,
}

impl Default for WorkspaceSet {
    fn default() -> Self {
        let active = WorkspaceId(1);
        let workspace = Workspace {
            id: active,
            name: "1".to_owned(),
            layout: LayoutTree::default(),
            last_focused: None,
        };
        let workspaces = HashMap::from([(active, workspace)]);
        let names = HashMap::from([("1".to_owned(), active)]);

        Self {
            active,
            workspaces,
            names,
            window_workspaces: HashMap::new(),
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

    pub fn focus_window(&mut self, window: WindowId) -> Result<(), WorkspaceError> {
        if self.workspace_for_window(window) != Some(self.active) {
            return Err(WorkspaceError::InvalidState(
                "focused window is not on the active workspace",
            ));
        }

        self.active_mut().last_focused = Some(window);

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
                last_focused: None,
            },
        );

        debug_assert!(self.validate().is_ok());
        Ok(id)
    }

    pub fn switch_to_numeric(&mut self, index: u32) -> Result<Option<WindowId>, WorkspaceError> {
        self.active = self.ensure_numeric(index)?;

        debug_assert!(self.validate().is_ok());
        Ok(self.active().last_focused)
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
        workspace.last_focused = Some(window);
        self.window_workspaces.insert(window, self.active);

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

        workspace.layout.remove(window)?;

        if workspace.last_focused == Some(window) {
            workspace.last_focused = first_window(&workspace.layout);
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

        let source = self
            .workspaces
            .get_mut(&source_id)
            .ok_or(WorkspaceError::UnknownWorkspace(source_id))?;
        source.layout.remove(window)?;

        if source.last_focused == Some(window) {
            source.last_focused = first_window(&source.layout);
        }

        let destination = self
            .workspaces
            .get_mut(&destination_id)
            .ok_or(WorkspaceError::UnknownWorkspace(destination_id))?;
        destination
            .layout
            .insert(window, destination.last_focused, axis, ratio)?;
        destination.last_focused = Some(window);
        self.window_workspaces.insert(window, destination_id);

        debug_assert!(self.validate().is_ok());
        Ok(destination_id)
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
                if !seen_windows.insert(window) || self.window_workspaces.get(&window) != Some(id) {
                    return Err(WorkspaceError::InvalidState(
                        "window ownership is inconsistent",
                    ));
                }
            }

            if workspace
                .last_focused
                .is_some_and(|window| !workspace.layout.contains(window))
            {
                return Err(WorkspaceError::InvalidState(
                    "workspace focus is inconsistent",
                ));
            }
        }

        if seen_windows.len() != self.window_workspaces.len() {
            return Err(WorkspaceError::InvalidState(
                "window index retains stale entries",
            ));
        }

        Ok(())
    }
}

fn first_window(layout: &LayoutTree) -> Option<WindowId> {
    layout.window_ids().min_by_key(|window| window.0)
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
}
