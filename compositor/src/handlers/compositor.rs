use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::protocol::wl_buffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface,
};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::shm::{ShmHandler, ShmState};

use super::{layer_shell, xdg_shell};
use crate::Ferese;
use crate::state::ClientState;

fn layer_affects_backdrop(layer: smithay::wayland::shell::wlr_layer::Layer) -> bool {
    matches!(
        layer,
        smithay::wayland::shell::wlr_layer::Layer::Background | smithay::wayland::shell::wlr_layer::Layer::Bottom
    )
}

impl Ferese {
    fn surface_affects_backdrop(&self, surface: &WlSurface) -> bool {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if matches!(&self.cursor_status, smithay::input::pointer::CursorImageStatus::Surface(cursor) if cursor == &root)
        {
            return false;
        }
        self.space
            .layer_for_surface(&root, smithay::desktop::WindowSurfaceType::ALL)
            .is_none_or(|layer| layer_affects_backdrop(layer.cached_state().layer))
    }
}

impl CompositorHandler for Ferese {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("all clients have Ferese client state")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        self.capture_resize_before_commit(surface);
        if self.surface_affects_backdrop(surface) {
            self.backdrop_generation = self.backdrop_generation.wrapping_add(1);
        }
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            let window = {
                self.space
                    .elements()
                    .find(|window| window.toplevel().is_some_and(|toplevel| toplevel.wl_surface() == &root))
                    .cloned()
            };

            if let Some(window) = window {
                window.on_commit();
                if surface == &root {
                    if crate::state::window_has_buffer(&window) {
                        xdg_shell::apply_initial_window_rules(self, &window);
                    } else if self.window_ids.contains_key(&window) {
                        self.remove_tiled_window(&window);
                        self.space.map_element(window.clone(), (0, 0), false);
                        self.restore_keyboard_focus();
                    }
                    if window
                        .toplevel()
                        .is_some_and(|toplevel| !xdg_shell::initial_configure_sent(toplevel))
                    {
                        self.restore_initial_floating_size(&window);
                    }
                }
                self.record_client_commit(&window);
            }
        }
        layer_shell::handle_commit(self, surface);
        xdg_shell::handle_commit(&mut self.popups, &mut self.space, surface);
        crate::backends::direct::render_all(self);
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        if self.session_lock.active {
            self.session_lock
                .surfaces
                .retain(|_, lock| lock.wl_surface() != surface);
            self.focus_lock_surface();
        }
        self.backdrop_generation = self.backdrop_generation.wrapping_add(1);
        crate::backends::direct::render_all(self);
        if self.idle_inhibitors.remove(surface).is_some() {
            self.refresh_idle_inhibition();
        }
        if matches!(&self.cursor_status, smithay::input::pointer::CursorImageStatus::Surface(cursor) if cursor == surface)
        {
            self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
            crate::backends::direct::render_all(self);
        }
    }
}

impl BufferHandler for Ferese {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl DmabufHandler for Ferese {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        self.queue_dmabuf_import(dmabuf, notifier);
        crate::backends::direct::render_all(self);
    }
}

impl ShmHandler for Ferese {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

#[cfg(test)]
mod tests {
    use smithay::wayland::shell::wlr_layer::Layer;

    use super::*;
    #[test]
    fn panel_commits_do_not_invalidate_their_own_backdrop() {
        assert!(!layer_affects_backdrop(Layer::Top));
        assert!(!layer_affects_backdrop(Layer::Overlay));
        assert!(layer_affects_backdrop(Layer::Bottom));
        assert!(layer_affects_backdrop(Layer::Background));
    }
}
