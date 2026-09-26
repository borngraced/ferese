use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::utils::on_commit_buffer_handler},
    reexports::wayland_server::{
        Client,
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        shm::{ShmHandler, ShmState},
    },
};

use super::{layer_shell, xdg_shell};
use crate::{Ferese, state::ClientState};

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
        on_commit_buffer_handler::<Self>(surface);
        self.invalidate_material_scene();
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            let window = {
                self.space
                    .elements()
                    .find(|window| {
                        window
                            .toplevel()
                            .is_some_and(|toplevel| toplevel.wl_surface() == &root)
                    })
                    .cloned()
            };

            if let Some(window) = window {
                if surface == &root {
                    xdg_shell::apply_initial_window_rules(self, &window);
                }
                window.on_commit();
                self.record_client_commit(&window);
            }
        }
        layer_shell::handle_commit(self, surface);
        xdg_shell::handle_commit(&mut self.popups, &mut self.space, surface);
        crate::backends::direct::render_all(self);
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        // A removed popup changes the backdrop even without a final buffer
        // commit. Invalidate expanded sampling regions as well as its bounds.
        self.invalidate_material_scene();
        crate::backends::direct::render_all(self);
        if self.idle_inhibitors.remove(surface).is_some() {
            self.idle_notifier_state
                .set_is_inhibited(!self.idle_inhibitors.is_empty());
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

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        self.queue_dmabuf_import(dmabuf, notifier);
        crate::backends::direct::render_all(self);
    }
}

impl ShmHandler for Ferese {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
