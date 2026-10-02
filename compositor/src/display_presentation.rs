//! Visibility of client content on displayed outputs, independent of capture.
use std::collections::{HashMap, HashSet};

use smithay::backend::renderer::element::{
    Id, RenderElementPresentationState, RenderElementState, RenderElementStates,
};
use smithay::output::Output;

#[derive(Default)]
struct OutputPresentation {
    displayed: HashSet<Id>,
    queued: Option<HashSet<Id>>,
}

#[derive(Default)]
pub(crate) struct DisplayPresentation {
    outputs: HashMap<Output, OutputPresentation>,
}

pub(crate) fn is_visible(state: RenderElementState) -> bool {
    state.visible_area > 0 && state.presentation_state != RenderElementPresentationState::Skipped
}

fn visible(states: &RenderElementStates) -> HashSet<Id> {
    states
        .states
        .iter()
        .filter_map(|(id, state)| is_visible(*state).then(|| id.clone()))
        .collect()
}

impl DisplayPresentation {
    // Call only after a successful submission, never from an offscreen capture.
    pub fn queued(&mut self, output: &Output, states: &RenderElementStates) {
        self.outputs.entry(output.clone()).or_default().queued = Some(visible(states));
    }

    pub fn presented(&mut self, output: &Output) {
        if let Some(record) = self.outputs.get_mut(output)
            && let Some(queued) = record.queued.take()
        {
            record.displayed = queued;
        }
    }

    pub fn visible(&self, surface: &Id) -> bool {
        self.outputs.values().any(|record| record.displayed.contains(surface))
    }

    // Membership describes every output; callback ownership is a separate choice.
    // A stable tie-break avoids alternating owners when refresh rates match.
    pub fn callback_output(&self, surface: &Id) -> Option<Output> {
        self.outputs
            .iter()
            .filter(|(_, record)| record.displayed.contains(surface))
            .map(|(output, _)| output)
            .max_by_key(|output| (output.current_mode().map_or(0, |mode| mode.refresh), output.name()))
            .cloned()
    }

    pub fn remove_surface(&mut self, surface: &Id) {
        for record in self.outputs.values_mut() {
            record.displayed.remove(surface);
            if let Some(queued) = &mut record.queued {
                queued.remove(surface);
            }
        }
    }

    pub fn remove_output(&mut self, output: &Output) {
        self.outputs.remove(output);
    }

    pub fn clear(&mut self) {
        self.outputs.clear();
    }
}

#[cfg(test)]
mod tests {
    use smithay::backend::renderer::element::RenderElementState;
    use smithay::output::{PhysicalProperties, Subpixel};

    use super::*;

    fn output(name: &str) -> Output {
        Output::new(
            name.into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
            },
        )
    }

    fn scene(id: &Id) -> RenderElementStates {
        let mut states = RenderElementStates::default();
        states.states.insert(
            id.clone(),
            RenderElementState {
                visible_area: 20,
                presentation_state: RenderElementPresentationState::ZeroCopy,
            },
        );
        states
    }

    #[test]
    fn visibility_changes_only_when_a_submission_is_presented() {
        let mut record = DisplayPresentation::default();
        let output = output("a");
        let id = Id::new();
        record.queued(&output, &scene(&id));
        assert!(!record.visible(&id));
        record.presented(&output);
        assert!(record.visible(&id));
        record.presented(&output); // No new submission keeps the last displayed scene.
        assert!(record.visible(&id));

        record.queued(&output, &RenderElementStates::default());
        assert!(record.visible(&id));
        record.presented(&output);
        assert!(!record.visible(&id));
    }

    #[test]
    fn outputs_are_independent_and_removal_cannot_resurrect_a_surface() {
        let mut record = DisplayPresentation::default();
        let a = output("a");
        let b = output("b");
        let id = Id::new();
        for output in [&a, &b] {
            record.queued(output, &scene(&id));
            record.presented(output);
        }
        record.remove_output(&a);
        assert!(record.visible(&id));
        record.queued(&b, &scene(&id));
        record.remove_surface(&id);
        record.presented(&b);
        assert!(!record.visible(&id));
        record.clear();
        assert!(!record.visible(&id));
    }

    #[test]
    fn occluded_and_skipped_content_do_not_count_as_visible() {
        let id = Id::new();
        let mut states = scene(&id);
        states.states.get_mut(&id).unwrap().visible_area = 0;
        assert!(visible(&states).is_empty());
        states.states.get_mut(&id).unwrap().visible_area = 20;
        states.states.get_mut(&id).unwrap().presentation_state = RenderElementPresentationState::Skipped;
        assert!(visible(&states).is_empty());
    }
    #[test]
    fn callback_ownership_is_stable_and_moves_when_an_output_disappears() {
        let mut record = DisplayPresentation::default();
        let a = output("a");
        let b = output("b");
        let id = Id::new();
        for output in [&a, &b] {
            record.queued(output, &scene(&id));
            record.presented(output);
        }
        assert_eq!(record.callback_output(&id), Some(b.clone()));
        record.remove_output(&b);
        assert_eq!(record.callback_output(&id), Some(a.clone()));
        record.remove_surface(&id);
        assert_eq!(record.callback_output(&id), None);
    }
}
