use std::sync::Arc;

use ferese_protocols::material::v1::client::{
    ferese_material_manager_v1::FereseMaterialManagerV1,
    ferese_surface_material_v1::FereseSurfaceMaterialV1,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_surface},
};

/// Keeps a compositor-owned dialog material attached for the window lifetime.
#[derive(Clone, Debug)]
pub struct ModalMaterial {
    _binding: Arc<Binding>,
}

#[derive(Debug)]
struct Binding {
    connection: Connection,
    manager: FereseMaterialManagerV1,
    material: FereseSurfaceMaterialV1,
}

impl ModalMaterial {
    /// Attach while Iced owns the native window; keep its background transparent on success.
    /// Other compositors return an error so the caller can retain its own surface fill.
    pub fn attach(window: &dyn cosmic::iced::window::Window) -> Result<Self, String> {
        use cosmic::iced::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};

        let display = window.display_handle().map_err(|error| error.to_string())?;
        let handle = window.window_handle().map_err(|error| error.to_string())?;
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) =
            (display.as_raw(), handle.as_raw())
        else {
            return Err("Dialog materials require Wayland".into());
        };
        // Iced lends live native handles; this backend borrows its connection.
        let backend = unsafe {
            wayland_client::backend::Backend::from_foreign_display(display.display.as_ptr().cast())
        };
        let connection = Connection::from_backend(backend);
        let id = unsafe {
            wayland_client::backend::ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                handle.surface.as_ptr().cast(),
            )
        }
        .map_err(|error| error.to_string())?;
        let surface =
            wl_surface::WlSurface::from_id(&connection, id).map_err(|error| error.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<MaterialState>(&connection).map_err(|error| error.to_string())?;
        let qh = queue.handle();
        let manager = globals
            .bind::<FereseMaterialManagerV1, _, _>(&qh, 1..=1, ())
            .map_err(|error| error.to_string())?;
        let material = manager.get_surface_material(&surface, &qh, ());
        let binding = Binding {
            connection,
            manager,
            material,
        };
        let mut state = MaterialState::default();
        queue
            .roundtrip(&mut state)
            .map_err(|error| error.to_string())?;
        if !state.ready {
            return Err("Dialog material attachment was not acknowledged".into());
        }
        Ok(Self {
            _binding: Arc::new(binding),
        })
    }
}

impl Drop for Binding {
    fn drop(&mut self) {
        self.material.destroy();
        self.manager.destroy();
        let _ = self.connection.flush();
    }
}

#[derive(Default)]
struct MaterialState {
    ready: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for MaterialState {
    fn event(
        _state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<FereseSurfaceMaterialV1, ()> for MaterialState {
    fn event(
        state: &mut Self,
        _material: &FereseSurfaceMaterialV1,
        event: ferese_protocols::material::v1::client::ferese_surface_material_v1::Event,
        _data: &(),
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if matches!(
            event,
            ferese_protocols::material::v1::client::ferese_surface_material_v1::Event::Ready
        ) {
            state.ready = true;
        }
    }
}

delegate_noop!(MaterialState: ignore FereseMaterialManagerV1);
