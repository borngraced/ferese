use super::*;

impl Ferese {
    pub(crate) fn defer_output_redraw(&mut self, output: Output) {
        if !self.output_redraw_pending.contains(&output) {
            self.output_redraw_pending.push(output);
        }
    }

    pub fn register_output(&mut self, output: &Output, identity: String) {
        if self.output_ids.is_empty() {
            self.reset_animation_clock();
        }

        if self.session_lock.active {
            self.session_lock.output_added(output);
            self.lock_input_activity();
        }
        let Some(geometry) = self.space.output_geometry(output) else {
            tracing::error!(output = %output.name(), "cannot register an unmapped output");
            return;
        };
        let output_id = *self.output_identity_ids.entry(identity).or_insert_with(|| {
            let id = OutputId(self.next_output_id);
            self.next_output_id = self.next_output_id.saturating_add(1);
            id
        });
        let fallback_workspace = self
            .workspaces
            .iter()
            .filter(|workspace| self.output_workspaces.output_for_workspace(workspace.id).is_none())
            .map(|workspace| workspace.id)
            .min_by_key(|id| id.0)
            .unwrap_or_else(|| self.workspaces.create_workspace());
        let geometry = OutputGeometry::new(geometry.loc.x, geometry.loc.y, geometry.size.w, geometry.size.h);

        let registration = self.output_workspaces.connect(output_id, geometry, fallback_workspace);

        match registration {
            Ok(_) => {
                self.output_ids.insert(output.clone(), output_id);
                self.outputs_by_id.insert(output_id, output.clone());
                self.output_names.insert(output_id, output.name());
                self.restore_output_focus();
                let visible = self.visible_workspace_ids();
                self.reconcile_workspaces(&visible);
                self.send_shell_snapshots();
            }
            Err(error) => {
                tracing::error!(%error, output = %output.name(), "failed to register output");
            }
        }
    }

    pub fn unregister_output(&mut self, output: &Output) {
        let Some(output_id) = self.output_ids.remove(output) else {
            return;
        };

        self.output_redraw_pending.retain(|pending| pending != output);
        self.outputs_by_id.remove(&output_id);
        self.output_names.remove(&output_id);
        self.workspace_slides.remove(&output_id);
        self.render.remove_output(output_id);
        self.pending_screencopies.retain(|capture| {
            if capture.output == *output {
                capture.fail();
                false
            } else {
                true
            }
        });
        self.space.unmap_output(output);
        self.session_lock.surfaces.remove(output);
        self.session_lock.backgrounds.remove(output);
        self.session_lock.output_removed(output);

        match self.output_workspaces.disconnect(output_id) {
            Ok(Some(target)) => {
                if let Some(workspace) = self.output_workspaces.active_workspace(target) {
                    self.activate_output_workspace(target, workspace);
                }
            }
            Ok(None) => {
                self.focused_window = None;
                self.restore_keyboard_focus();
            }
            Err(error) => {
                tracing::error!(%error, output = %output.name(), "failed to unregister output")
            }
        }

        self.relayout();
    }

    pub(crate) fn reposition_output_floats(&mut self, output: OutputId, old: Rect, new: Rect) {
        let floats = self
            .windows
            .ids()
            .values()
            .filter_map(|id| {
                let workspace = self.workspaces.workspace_for_window(*id)?;
                if self.output_workspaces.output_for_workspace(workspace) != Some(output) {
                    return None;
                }
                match self.workspaces.placement(*id)? {
                    WindowPlacement::Floating { rect } => Some((*id, moved_floating_rect(rect, old, new))),
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        for (id, rect) in floats {
            let _ = self.workspaces.set_floating_rect(id, rect);
        }
    }

    pub(super) fn activate_output_workspace(&mut self, output: OutputId, workspace: ferese_core::WorkspaceId) {
        if let Err(error) = self.output_workspaces.focus_output(output) {
            tracing::error!(%error, "failed to focus output");
            return;
        }

        match self.workspaces.activate(workspace) {
            Ok(focus) => self.focused_window = focus,
            Err(error) => tracing::error!(%error, "failed to activate output workspace"),
        }
    }

    pub(crate) fn focused_output(&self) -> Option<&Output> {
        let focused = self.output_workspaces.focused_output()?;
        self.outputs_by_id.get(&focused)
    }

    pub(crate) fn restore_output_focus(&mut self) {
        if let Some(output) = self.output_workspaces.focused_output()
            && let Some(workspace) = self.output_workspaces.active_workspace(output)
        {
            self.activate_output_workspace(output, workspace);
            self.restore_keyboard_focus();
        }
    }

    pub(crate) fn output_id(&self, output: &Output) -> Option<OutputId> {
        self.output_ids.get(output).copied()
    }

    pub(crate) fn focus_output_at(&mut self, position: Point<f64, Logical>) {
        let output = self.space.outputs().find_map(|output| {
            let geometry = self.space.output_geometry(output)?;
            (position.x >= f64::from(geometry.loc.x)
                && position.y >= f64::from(geometry.loc.y)
                && position.x < f64::from(geometry.loc.x + geometry.size.w)
                && position.y < f64::from(geometry.loc.y + geometry.size.h))
            .then(|| output.clone())
        });
        let Some(output) = output else {
            return;
        };
        let Some(output_id) = self.output_ids.get(&output).copied() else {
            return;
        };
        let Some(workspace) = self.output_workspaces.active_workspace(output_id) else {
            return;
        };
        if self.output_workspaces.focused_output() == Some(output_id) && self.workspaces.active_id() == workspace {
            return;
        }

        self.activate_output_workspace(output_id, workspace);
        self.relayout();
    }
}
