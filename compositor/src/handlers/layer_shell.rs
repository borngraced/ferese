use smithay::{
    desktop::{
        LayerSurface as DesktopLayerSurface, PopupKind, WindowSurfaceType, layer_map_for_output,
    },
    output::Output,
    reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    wayland::shell::{
        wlr_layer::{
            KeyboardInteractivity, Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState,
        },
        xdg::PopupSurface,
    },
};

use crate::Ferese;

impl WlrLayerShellHandler for Ferese {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.space.outputs().next().cloned());
        let Some(output) = output else {
            tracing::warn!(%namespace, "layer surface has no available output");
            return;
        };
        let layer = DesktopLayerSurface::new(surface, namespace.clone());

        if let Err(error) = layer_map_for_output(&output).map_layer(&layer) {
            tracing::warn!(%namespace, %error, "failed to map layer surface");
            return;
        }

        tracing::debug!(%namespace, output = %output.name(), "mapped layer surface");
        super::set_surface_tree_scale(
            layer.wl_surface(),
            output.current_scale().fractional_scale(),
        );
        self.relayout();
        crate::backends::direct::render_all(self);
    }

    fn new_popup(&mut self, parent: LayerSurface, popup: PopupSurface) {
        let scale = self.space.outputs().find_map(|output| {
            layer_map_for_output(output)
                .layers()
                .any(|layer| layer.wl_surface() == parent.wl_surface())
                .then(|| output.current_scale().fractional_scale())
        });
        if let Some(scale) = scale {
            super::set_surface_tree_scale(popup.wl_surface(), scale);
        }
        self.unconstrain_popup(&popup);
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            tracing::warn!(?error, "failed to track layer-shell popup");
        }
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        let Some(layer) = self
            .space
            .layer_for_surface(surface.wl_surface(), WindowSurfaceType::TOPLEVEL)
        else {
            return;
        };
        let restore_focus = layer_has_keyboard_focus(self, &layer);
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();

        for output in outputs {
            layer_map_for_output(&output).unmap_layer(&layer);
        }

        self.relayout();
        if restore_focus {
            self.restore_keyboard_focus();
        }
        crate::backends::direct::render_all(self);
    }
}

pub fn handle_commit(state: &mut Ferese, surface: &WlSurface) {
    let Some(layer) = state
        .space
        .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
    else {
        return;
    };
    let outputs = state.space.outputs().cloned().collect::<Vec<_>>();

    for output in outputs {
        let mut map = layer_map_for_output(&output);
        if map.layer_geometry(&layer).is_some() {
            map.arrange();
            layer.layer_surface().send_pending_configure();
            break;
        }
    }

    let policy = layer.cached_state();
    if !state.session_lock.active
        && policy.keyboard_interactivity == KeyboardInteractivity::Exclusive
        && matches!(policy.layer, Layer::Top | Layer::Overlay)
    {
        state
            .seat
            .get_keyboard()
            .expect("seat has a keyboard")
            .set_focus(
                state,
                Some(layer.wl_surface().clone()),
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
    } else if !layer.can_receive_keyboard_focus() && layer_has_keyboard_focus(state, &layer) {
        state.restore_keyboard_focus();
    }

    state.relayout();
    if let Err(error) = state.display_handle.flush_clients() {
        tracing::debug!(%error, "failed to flush layer-surface configure");
    }
}

fn layer_has_keyboard_focus(state: &Ferese, layer: &DesktopLayerSurface) -> bool {
    state
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .and_then(|surface| {
            state
                .space
                .layer_for_surface(&surface, WindowSurfaceType::ALL)
        })
        .is_some_and(|focused| focused == *layer)
}
