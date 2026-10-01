use super::*;

impl Ferese {
    pub(super) fn focus_candidates(&self) -> Vec<WindowId> {
        self.windows
            .ids()
            .iter()
            .filter_map(|(window, id)| {
                self.workspaces.workspace_for_window(*id)?;
                (window.toplevel().is_some()
                    && self.window_content_ready(window)
                    && self.windows.geometry(id).is_some()
                    && self.output_workspaces.focused_output().is_some()
                    && !self.windows.record(*id).is_some_and(|record| record.closing.is_some()))
                .then_some(*id)
            })
            .collect()
    }

    pub(crate) fn focus_preview_output(&self, id: WindowId) -> Option<OutputId> {
        if self.windows.record(id).is_some_and(|record| record.closing.is_some()) {
            return None;
        }
        self.workspaces
            .workspace_for_window(id)
            .and_then(|workspace| self.output_workspaces.output_for_workspace(workspace))
            .or_else(|| self.output_workspaces.focused_output())
    }

    pub(crate) fn focus_last_window(&mut self) {
        if self.session_lock.active || self.input_capture.captures(1) {
            return;
        }
        self.cancel_focus_cycle();
        let target = self
            .focus_history
            .candidates(self.focused_window, self.focus_candidates())
            .into_iter()
            .find(|id| Some(*id) != self.focused_window);
        if let Some(id) = target {
            self.activate_managed_window(id);
        }
    }

    pub(crate) fn cycle_focus(&mut self, reverse: bool, preview: bool) {
        if self.session_lock.active || self.input_capture.captures(1) {
            return;
        }
        let available = self.focus_candidates();
        if !preview {
            self.cancel_focus_cycle();
            let order = self.focus_history.candidates(self.focused_window, available.clone());
            let mut cycle = ferese_core::FocusCycle::new(order, self.focused_window);
            if let Some(id) = cycle.advance(reverse, &available) {
                self.activate_managed_window(id);
            }
            return;
        }
        if self.focus_cycle.is_none() {
            let order = self.focus_history.candidates(self.focused_window, available.clone());
            if order.len() < 2 {
                return;
            }
            self.focus_cycle = Some(ferese_core::FocusCycle::new(order, self.focused_window));
            if self.overview.is_active() {
                self.retarget_overview();
            } else {
                self.set_overview_active(true);
            }
        }
        if let Some(id) = self
            .focus_cycle
            .as_mut()
            .and_then(|cycle| cycle.advance(reverse, &available))
        {
            self.overview.select_window(id);
            crate::backends::direct::render_all(self);
        } else {
            self.cancel_focus_cycle();
        }
    }

    pub(crate) fn finish_focus_cycle(&mut self) {
        let Some(mut cycle) = self.focus_cycle.take() else {
            return;
        };
        if !self.session_lock.active
            && !self.input_capture.captures(1)
            && let Some(id) = cycle.reconcile(&self.focus_candidates())
        {
            self.activate_managed_window(id);
        }
        self.set_overview_active(false);
    }

    pub(crate) fn cancel_focus_cycle(&mut self) {
        if self.focus_cycle.take().is_some() {
            self.set_overview_active(false);
        }
    }
}
