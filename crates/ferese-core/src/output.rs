use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::WorkspaceId;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OutputId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl OutputGeometry {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { x, y, width, height }
    }

    fn center(self) -> (i64, i64) {
        (
            i64::from(self.x) * 2 + i64::from(self.width),
            i64::from(self.y) * 2 + i64::from(self.height),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutputError {
    AlreadyConnected(OutputId),
    UnknownOutput(OutputId),
    WorkspaceAlreadyAssigned { workspace: WorkspaceId, output: OutputId },
}

impl fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyConnected(output) => write!(formatter, "output {output:?} is connected"),
            Self::UnknownOutput(output) => write!(formatter, "unknown output {output:?}"),
            Self::WorkspaceAlreadyAssigned { workspace, output } => {
                write!(formatter, "workspace {workspace:?} is assigned to {output:?}")
            }
        }
    }
}

impl Error for OutputError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceSwitch {
    Activated(OutputId),
    FocusedExisting(OutputId),
}

#[derive(Debug)]
struct OutputState {
    geometry: OutputGeometry,
    active: WorkspaceId,
    previous: Option<WorkspaceId>,
    workspaces: HashSet<WorkspaceId>,
}

impl OutputState {
    fn activate(&mut self, workspace: WorkspaceId) {
        if self.active != workspace {
            self.previous = self.workspaces.contains(&self.active).then_some(self.active);
            self.active = workspace;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct EvacuatedWorkspace {
    workspace: WorkspaceId,
    target: Option<OutputId>,
    revision: u64,
}

#[derive(Debug)]
struct EvacuationRecord {
    active: WorkspaceId,
    workspaces: Vec<EvacuatedWorkspace>,
}

#[derive(Debug, Default)]
pub struct OutputWorkspaceMap {
    outputs: HashMap<OutputId, OutputState>,
    assignments: HashMap<WorkspaceId, OutputId>,
    revisions: HashMap<WorkspaceId, u64>,
    evacuations: HashMap<OutputId, EvacuationRecord>,
    focused: Option<OutputId>,
}

impl OutputWorkspaceMap {
    pub fn focused_output(&self) -> Option<OutputId> {
        self.focused
    }

    pub fn active_workspace(&self, output: OutputId) -> Option<WorkspaceId> {
        self.outputs.get(&output).map(|state| state.active)
    }

    pub fn previous_workspace(&self, output: OutputId) -> Option<WorkspaceId> {
        self.outputs.get(&output).and_then(|state| state.previous)
    }

    pub fn history_workspaces(&self) -> impl Iterator<Item = WorkspaceId> + '_ {
        self.outputs.values().filter_map(|state| state.previous)
    }

    pub fn output_for_workspace(&self, workspace: WorkspaceId) -> Option<OutputId> {
        self.assignments.get(&workspace).copied()
    }

    pub fn forget_workspace(&mut self, workspace: WorkspaceId) -> bool {
        if self.outputs.values().any(|output| output.active == workspace) {
            return false;
        }
        if let Some(owner) = self.assignments.remove(&workspace)
            && let Some(output) = self.outputs.get_mut(&owner)
        {
            output.workspaces.remove(&workspace);
            if output.previous == Some(workspace) {
                output.previous = None;
            }
        }
        self.revisions.remove(&workspace);
        self.evacuations.retain(|_, record| {
            record.workspaces.retain(|entry| entry.workspace != workspace);
            if record.active == workspace {
                if let Some(entry) = record.workspaces.first() {
                    record.active = entry.workspace;
                } else {
                    return false;
                }
            }
            true
        });
        debug_assert!(self.validate());
        true
    }

    pub fn connected_outputs(&self) -> impl Iterator<Item = OutputId> + '_ {
        self.outputs.keys().copied()
    }

    pub fn update_geometry(&mut self, output: OutputId, geometry: OutputGeometry) {
        if let Some(state) = self.outputs.get_mut(&output) {
            state.geometry = geometry;
        }
    }

    pub fn connect(
        &mut self,
        output: OutputId,
        geometry: OutputGeometry,
        fallback_workspace: WorkspaceId,
    ) -> Result<WorkspaceId, OutputError> {
        if self.outputs.contains_key(&output) {
            return Err(OutputError::AlreadyConnected(output));
        }

        let record = self.evacuations.remove(&output);
        let mut reclaimed = Vec::new();
        if let Some(record) = &record {
            for evacuated in &record.workspaces {
                let assignment_matches = self.assignments.get(&evacuated.workspace).copied() == evacuated.target;
                let revision_matches = self.revision(evacuated.workspace) == evacuated.revision;

                if assignment_matches && revision_matches {
                    if let Some(target) = evacuated.target
                        && let Some(target_state) = self.outputs.get_mut(&target)
                    {
                        target_state.workspaces.remove(&evacuated.workspace);
                        if target_state.previous == Some(evacuated.workspace) {
                            target_state.previous = None;
                        }
                        if target_state.active == evacuated.workspace
                            && let Some(active) = target_state
                                .workspaces
                                .iter()
                                .copied()
                                .min_by_key(|workspace| workspace.0)
                        {
                            target_state.activate(active);
                        }
                    }
                    self.assignments.insert(evacuated.workspace, output);
                    reclaimed.push(evacuated.workspace);
                }
            }
        }

        let active = record
            .as_ref()
            .and_then(|record| reclaimed.contains(&record.active).then_some(record.active))
            .or_else(|| reclaimed.iter().copied().min_by_key(|workspace| workspace.0));
        let active = match active {
            Some(active) => active,
            None => {
                if let Some(owner) = self.assignments.get(&fallback_workspace).copied() {
                    return Err(OutputError::WorkspaceAlreadyAssigned {
                        workspace: fallback_workspace,
                        output: owner,
                    });
                }
                self.assignments.insert(fallback_workspace, output);
                reclaimed.push(fallback_workspace);
                fallback_workspace
            }
        };

        self.outputs.insert(
            output,
            OutputState {
                geometry,
                active,
                previous: None,
                workspaces: reclaimed.into_iter().collect(),
            },
        );
        self.focused.get_or_insert(output);

        debug_assert!(self.validate());
        Ok(active)
    }

    pub fn disconnect(&mut self, output: OutputId) -> Result<Option<OutputId>, OutputError> {
        let removed = self.outputs.remove(&output).ok_or(OutputError::UnknownOutput(output))?;
        let target = self.migration_target(output, removed.geometry);
        let removed_had_focus = self.focused == Some(output);
        let mut evacuated = removed.workspaces.into_iter().collect::<Vec<_>>();
        evacuated.sort_by_key(|workspace| workspace.0);

        for workspace in &evacuated {
            if let Some(target) = target {
                self.assignments.insert(*workspace, target);
                self.outputs
                    .get_mut(&target)
                    .expect("migration target is connected")
                    .workspaces
                    .insert(*workspace);
            } else {
                self.assignments.remove(workspace);
            }
        }

        if removed_had_focus {
            self.focused = target;
            if let Some(target) = target {
                self.outputs
                    .get_mut(&target)
                    .expect("migration target is connected")
                    .activate(removed.active);
            }
        }

        self.evacuations.insert(
            output,
            EvacuationRecord {
                active: removed.active,
                workspaces: evacuated
                    .into_iter()
                    .map(|workspace| EvacuatedWorkspace {
                        workspace,
                        target,
                        revision: self.revision(workspace),
                    })
                    .collect(),
            },
        );

        debug_assert!(self.validate());
        Ok(target)
    }

    pub fn focus_output(&mut self, output: OutputId) -> Result<(), OutputError> {
        if !self.outputs.contains_key(&output) {
            return Err(OutputError::UnknownOutput(output));
        }
        self.focused = Some(output);
        Ok(())
    }

    pub fn switch_workspace(
        &mut self,
        output: OutputId,
        workspace: WorkspaceId,
    ) -> Result<WorkspaceSwitch, OutputError> {
        if !self.outputs.contains_key(&output) {
            return Err(OutputError::UnknownOutput(output));
        }

        if let Some(owner) = self.assignments.get(&workspace).copied()
            && owner != output
        {
            self.focused = Some(owner);
            self.outputs
                .get_mut(&owner)
                .expect("workspace owner is connected")
                .activate(workspace);
            return Ok(WorkspaceSwitch::FocusedExisting(owner));
        }

        let reassigned = self.assignments.insert(workspace, output) != Some(output);
        let state = self.outputs.get_mut(&output).expect("output was checked above");
        state.workspaces.insert(workspace);
        state.activate(workspace);
        self.focused = Some(output);
        if reassigned {
            self.bump_revision(workspace);
        }

        debug_assert!(self.validate());
        Ok(WorkspaceSwitch::Activated(output))
    }

    pub fn select_workspace(
        &mut self,
        output: OutputId,
        workspace: WorkspaceId,
        auto_back_and_forth: bool,
    ) -> Result<WorkspaceSwitch, OutputError> {
        let workspace = if auto_back_and_forth && self.active_workspace(output) == Some(workspace) {
            self.previous_workspace(output).unwrap_or(workspace)
        } else {
            workspace
        };
        self.switch_workspace(output, workspace)
    }

    pub fn assign_workspace(&mut self, output: OutputId, workspace: WorkspaceId) -> Result<OutputId, OutputError> {
        if !self.outputs.contains_key(&output) {
            return Err(OutputError::UnknownOutput(output));
        }
        if let Some(owner) = self.assignments.get(&workspace).copied() {
            return Ok(owner);
        }

        self.assignments.insert(workspace, output);
        self.outputs
            .get_mut(&output)
            .expect("output was checked above")
            .workspaces
            .insert(workspace);
        self.bump_revision(workspace);

        debug_assert!(self.validate());
        Ok(output)
    }

    fn migration_target(&self, removed: OutputId, removed_geometry: OutputGeometry) -> Option<OutputId> {
        if let Some(focused) = self.focused
            && focused != removed
            && self.outputs.contains_key(&focused)
        {
            return Some(focused);
        }

        let removed_center = removed_geometry.center();
        self.outputs
            .iter()
            .min_by_key(|(id, state)| {
                let center = state.geometry.center();
                let x = center.0 - removed_center.0;
                let y = center.1 - removed_center.1;
                (x * x + y * y, **id)
            })
            .map(|(id, _)| *id)
    }

    fn revision(&self, workspace: WorkspaceId) -> u64 {
        self.revisions.get(&workspace).copied().unwrap_or(0)
    }

    fn bump_revision(&mut self, workspace: WorkspaceId) {
        let revision = self.revisions.entry(workspace).or_default();
        *revision = revision.saturating_add(1);
    }

    fn validate(&self) -> bool {
        if self.focused.is_some_and(|focused| !self.outputs.contains_key(&focused)) {
            return false;
        }

        for (output, state) in &self.outputs {
            if !state.workspaces.contains(&state.active) {
                return false;
            }
            if state
                .previous
                .is_some_and(|previous| previous == state.active || !state.workspaces.contains(&previous))
            {
                return false;
            }
            for workspace in &state.workspaces {
                if self.assignments.get(workspace) != Some(output) {
                    return false;
                }
            }
        }

        self.assignments.iter().all(|(workspace, output)| {
            self.outputs
                .get(output)
                .is_some_and(|state| state.workspaces.contains(workspace))
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn forgetting_an_inactive_workspace_prevents_hotplug_resurrection() {
        let mut outputs = super::OutputWorkspaceMap::default();
        let display = super::OutputId(1);
        let geometry = super::OutputGeometry::new(0, 0, 1920, 1080);
        let first = super::WorkspaceId(1);
        let second = super::WorkspaceId(2);
        outputs.connect(display, geometry, first).unwrap();
        assert!(!outputs.forget_workspace(first));
        outputs.switch_workspace(display, second).unwrap();
        assert!(outputs.forget_workspace(first));
        assert_eq!(outputs.output_for_workspace(first), None);
        outputs.disconnect(display).unwrap();
        outputs.connect(display, geometry, super::WorkspaceId(3)).unwrap();
        assert_eq!(outputs.active_workspace(display), Some(second));
        assert_eq!(outputs.output_for_workspace(first), None);
        assert!(outputs.validate());
    }

    #[test]
    fn changing_output_geometry_preserves_workspace_ownership_and_focus() {
        let mut outputs = super::OutputWorkspaceMap::default();
        let id = super::OutputId(1);
        let workspace = crate::WorkspaceId(1);
        outputs
            .connect(id, super::OutputGeometry::new(0, 0, 1920, 1080), workspace)
            .unwrap();
        outputs.update_geometry(id, super::OutputGeometry::new(2000, 0, 1280, 720));
        assert_eq!(outputs.output_for_workspace(workspace), Some(id));
        assert_eq!(outputs.active_workspace(id), Some(workspace));
        assert_eq!(outputs.focused_output(), Some(id));
    }
    use super::*;

    fn geometry(x: i32) -> OutputGeometry {
        OutputGeometry::new(x, 0, 1_920, 1_080)
    }

    #[test]
    fn history_tracks_actual_switches_and_toggles_without_replacing_it_on_a_noop() {
        let mut outputs = OutputWorkspaceMap::default();
        let display = OutputId(1);
        outputs.connect(display, geometry(0), WorkspaceId(1)).unwrap();
        assert_eq!(outputs.previous_workspace(display), None);
        outputs.select_workspace(display, WorkspaceId(1), true).unwrap();
        assert_eq!(outputs.previous_workspace(display), None);
        outputs.switch_workspace(display, WorkspaceId(2)).unwrap();
        outputs.switch_workspace(display, WorkspaceId(2)).unwrap();
        assert_eq!(outputs.previous_workspace(display), Some(WorkspaceId(1)));
        outputs.select_workspace(display, WorkspaceId(2), false).unwrap();
        assert_eq!(outputs.active_workspace(display), Some(WorkspaceId(2)));
        for expected in [WorkspaceId(1), WorkspaceId(2), WorkspaceId(1)] {
            let current = outputs.active_workspace(display).unwrap();
            outputs.select_workspace(display, current, true).unwrap();
            assert_eq!(outputs.active_workspace(display), Some(expected));
            assert_eq!(outputs.previous_workspace(display), Some(current));
        }
        assert!(outputs.validate());
    }

    #[test]
    fn another_monitors_workspace_updates_only_its_owners_history() {
        let mut outputs = OutputWorkspaceMap::default();
        let left = OutputId(1);
        let right = OutputId(2);
        outputs.connect(left, geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(right, geometry(1920), WorkspaceId(2)).unwrap();
        outputs.switch_workspace(left, WorkspaceId(3)).unwrap();
        outputs.switch_workspace(right, WorkspaceId(4)).unwrap();
        assert_eq!(
            outputs.select_workspace(left, WorkspaceId(2), true),
            Ok(WorkspaceSwitch::FocusedExisting(right))
        );
        assert_eq!(outputs.active_workspace(left), Some(WorkspaceId(3)));
        assert_eq!(outputs.previous_workspace(left), Some(WorkspaceId(1)));
        assert_eq!(outputs.previous_workspace(right), Some(WorkspaceId(4)));
        let previous = outputs.previous_workspace(right).unwrap();
        outputs.switch_workspace(right, previous).unwrap();
        assert_eq!(outputs.active_workspace(right), Some(WorkspaceId(4)));
        assert_eq!(outputs.previous_workspace(right), Some(WorkspaceId(2)));
        assert!(outputs.validate());
    }

    #[test]
    fn reconnect_clears_history_for_workspaces_reclaimed_by_another_output() {
        let mut outputs = OutputWorkspaceMap::default();
        let left = OutputId(1);
        let right = OutputId(2);
        outputs.connect(left, geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(right, geometry(1920), WorkspaceId(2)).unwrap();
        outputs.focus_output(right).unwrap();
        outputs.disconnect(right).unwrap();
        assert_eq!(outputs.previous_workspace(left), Some(WorkspaceId(1)));
        outputs.switch_workspace(left, WorkspaceId(1)).unwrap();
        assert_eq!(outputs.previous_workspace(left), Some(WorkspaceId(2)));
        outputs.connect(right, geometry(1920), WorkspaceId(3)).unwrap();
        assert_eq!(outputs.previous_workspace(left), None);
        assert_eq!(outputs.previous_workspace(right), None);
        assert!(outputs.validate());
    }

    #[test]
    fn forgetting_a_previous_workspace_clears_its_history() {
        let mut outputs = OutputWorkspaceMap::default();
        let display = OutputId(1);
        outputs.connect(display, geometry(0), WorkspaceId(1)).unwrap();
        outputs.switch_workspace(display, WorkspaceId(2)).unwrap();
        assert!(outputs.forget_workspace(WorkspaceId(1)));
        assert_eq!(outputs.previous_workspace(display), None);
        outputs.select_workspace(display, WorkspaceId(2), true).unwrap();
        assert_eq!(outputs.active_workspace(display), Some(WorkspaceId(2)));
        assert_eq!(
            outputs.select_workspace(OutputId(99), WorkspaceId(2), true),
            Err(OutputError::UnknownOutput(OutputId(99)))
        );
    }

    #[test]
    fn empty_previous_workspace_survives_pruning_until_history_moves_on() {
        let mut workspaces = crate::WorkspaceSet::default();
        let first = workspaces.active_id();
        let second = workspaces.ensure_numeric(2).unwrap();
        let third = workspaces.ensure_numeric(3).unwrap();
        let mut outputs = OutputWorkspaceMap::default();
        let display = OutputId(1);
        outputs.connect(display, geometry(0), first).unwrap();
        outputs.switch_workspace(display, second).unwrap();
        workspaces.activate(second).unwrap();
        let protected = outputs.history_workspaces().chain([second]).collect();
        let removed = workspaces.prune_empty(&protected);
        assert!(removed.contains(&third));
        assert!(workspaces.workspace(first).is_some());
        let third = workspaces.ensure_numeric(3).unwrap();
        outputs.switch_workspace(display, third).unwrap();
        workspaces.activate(third).unwrap();
        let protected = outputs.history_workspaces().chain([third]).collect();
        for workspace in workspaces.prune_empty(&protected) {
            outputs.forget_workspace(workspace);
        }
        assert!(workspaces.workspace(first).is_none());
        assert!(workspaces.workspace(second).is_some());
        assert_eq!(outputs.previous_workspace(display), Some(second));
    }

    #[test]
    fn new_outputs_receive_distinct_workspaces() {
        let mut outputs = OutputWorkspaceMap::default();

        assert_eq!(
            outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)),
            Ok(WorkspaceId(1))
        );
        assert_eq!(
            outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(2)),
            Ok(WorkspaceId(2))
        );
        assert_eq!(outputs.output_for_workspace(WorkspaceId(1)), Some(OutputId(1)));
        assert_eq!(outputs.output_for_workspace(WorkspaceId(2)), Some(OutputId(2)));
    }

    #[test]
    fn removal_prefers_a_remaining_focused_output_without_replacing_its_workspace() {
        let mut outputs = OutputWorkspaceMap::default();
        outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(2)).unwrap();
        outputs.focus_output(OutputId(1)).unwrap();

        assert_eq!(outputs.disconnect(OutputId(2)), Ok(Some(OutputId(1))));
        assert_eq!(outputs.active_workspace(OutputId(1)), Some(WorkspaceId(1)));
        assert_eq!(outputs.output_for_workspace(WorkspaceId(2)), Some(OutputId(1)));
    }

    #[test]
    fn removing_the_focused_output_activates_its_workspace_on_the_nearest_output() {
        let mut outputs = OutputWorkspaceMap::default();
        outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(2)).unwrap();
        outputs.connect(OutputId(3), geometry(3_840), WorkspaceId(3)).unwrap();
        outputs.focus_output(OutputId(2)).unwrap();

        assert_eq!(outputs.disconnect(OutputId(2)), Ok(Some(OutputId(1))));
        assert_eq!(outputs.focused_output(), Some(OutputId(1)));
        assert_eq!(outputs.active_workspace(OutputId(1)), Some(WorkspaceId(2)));
    }

    #[test]
    fn reconnect_reclaims_automatically_evacuated_workspaces() {
        let mut outputs = OutputWorkspaceMap::default();
        outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(2)).unwrap();
        outputs.focus_output(OutputId(1)).unwrap();
        outputs.disconnect(OutputId(2)).unwrap();

        assert_eq!(
            outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(3)),
            Ok(WorkspaceId(2))
        );
        assert_eq!(outputs.output_for_workspace(WorkspaceId(2)), Some(OutputId(2)));
        assert_eq!(outputs.active_workspace(OutputId(2)), Some(WorkspaceId(2)));
    }

    #[test]
    fn reconnect_restores_the_migration_targets_remaining_workspace() {
        let mut outputs = OutputWorkspaceMap::default();
        outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(2)).unwrap();
        outputs.focus_output(OutputId(2)).unwrap();
        outputs.disconnect(OutputId(2)).unwrap();
        assert_eq!(outputs.active_workspace(OutputId(1)), Some(WorkspaceId(2)));

        outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(3)).unwrap();

        assert_eq!(outputs.active_workspace(OutputId(1)), Some(WorkspaceId(1)));
        assert_eq!(outputs.active_workspace(OutputId(2)), Some(WorkspaceId(2)));
    }

    #[test]
    fn using_an_evacuated_workspace_does_not_prevent_automatic_return() {
        let mut outputs = OutputWorkspaceMap::default();
        outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(2)).unwrap();
        outputs.focus_output(OutputId(1)).unwrap();
        outputs.disconnect(OutputId(2)).unwrap();
        outputs.switch_workspace(OutputId(1), WorkspaceId(2)).unwrap();

        assert_eq!(
            outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(3)),
            Ok(WorkspaceId(2))
        );
        assert_eq!(outputs.output_for_workspace(WorkspaceId(2)), Some(OutputId(2)));
    }

    #[test]
    fn repeated_lid_and_monitor_handoffs_restore_both_workspace_owners() {
        let mut outputs = OutputWorkspaceMap::default();
        outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(OutputId(2), geometry(1920), WorkspaceId(2)).unwrap();
        for removed in [OutputId(1), OutputId(2), OutputId(1), OutputId(2)] {
            let remaining = if removed == OutputId(1) {
                OutputId(2)
            } else {
                OutputId(1)
            };
            outputs.focus_output(removed).unwrap();
            outputs.disconnect(removed).unwrap();
            assert_eq!(outputs.output_for_workspace(WorkspaceId(1)), Some(remaining));
            assert_eq!(outputs.output_for_workspace(WorkspaceId(2)), Some(remaining));
            // Switching between handed-off workspaces is normal use, not an
            // explicit change of their home output.
            outputs.switch_workspace(remaining, WorkspaceId(1)).unwrap();
            outputs.switch_workspace(remaining, WorkspaceId(2)).unwrap();
            outputs
                .connect(
                    removed,
                    geometry(if removed == OutputId(1) { 0 } else { 1920 }),
                    WorkspaceId(3),
                )
                .unwrap();
            assert_eq!(outputs.output_for_workspace(WorkspaceId(1)), Some(OutputId(1)));
            assert_eq!(outputs.output_for_workspace(WorkspaceId(2)), Some(OutputId(2)));
            assert_eq!(outputs.active_workspace(OutputId(1)), Some(WorkspaceId(1)));
            assert_eq!(outputs.active_workspace(OutputId(2)), Some(WorkspaceId(2)));
        }
    }

    #[test]
    fn selecting_a_workspace_owned_by_another_output_focuses_that_output() {
        let mut outputs = OutputWorkspaceMap::default();
        outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)).unwrap();
        outputs.connect(OutputId(2), geometry(1_920), WorkspaceId(2)).unwrap();

        assert_eq!(
            outputs.switch_workspace(OutputId(1), WorkspaceId(2)),
            Ok(WorkspaceSwitch::FocusedExisting(OutputId(2)))
        );
        assert_eq!(outputs.focused_output(), Some(OutputId(2)));
    }

    #[test]
    fn assigning_an_unused_workspace_does_not_change_the_active_workspace() {
        let mut outputs = OutputWorkspaceMap::default();
        outputs.connect(OutputId(1), geometry(0), WorkspaceId(1)).unwrap();

        assert_eq!(outputs.assign_workspace(OutputId(1), WorkspaceId(2)), Ok(OutputId(1)));
        assert_eq!(outputs.active_workspace(OutputId(1)), Some(WorkspaceId(1)));
        assert_eq!(outputs.output_for_workspace(WorkspaceId(2)), Some(OutputId(1)));
    }
}
