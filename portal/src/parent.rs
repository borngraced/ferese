pub(crate) type Attachment = Result<
    (
        Option<std::sync::Arc<Parent>>,
        Result<ferese_theme_client::material::ModalMaterial, String>,
    ),
    String,
>;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_imported_v2;
use wayland_protocols::xdg::foreign::zv2::client::zxdg_imported_v2::ZxdgImportedV2;
use wayland_protocols::xdg::foreign::zv2::client::zxdg_importer_v2::ZxdgImporterV2;

#[derive(Debug)]
pub(crate) struct Parent {
    connection: Connection,
    imported: ZxdgImportedV2,
    importer: ZxdgImporterV2,
}

impl Parent {
    pub(crate) fn attach(window: &dyn cosmic::iced::window::Window, parent: &str) -> Result<Option<Self>, String> {
        use cosmic::iced::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
        if parent.is_empty() {
            return Ok(None);
        }
        let handle = parent
            .strip_prefix("wayland:")
            .filter(|handle| !handle.is_empty() && handle.len() <= 4096)
            .ok_or("Unsupported portal parent window identifier")?;
        let display = window.display_handle().map_err(|error| error.to_string())?;
        let surface = window.window_handle().map_err(|error| error.to_string())?;
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(surface)) =
            (display.as_raw(), surface.as_raw())
        else {
            return Err("Portal parenting requires Wayland".into());
        };
        // The window lends live handles; the borrowed backend never owns the display.
        let backend =
            unsafe { wayland_client::backend::Backend::from_foreign_display(display.display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let id = unsafe {
            wayland_client::backend::ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                surface.surface.as_ptr().cast(),
            )
        }
        .map_err(|error| error.to_string())?;
        let surface = wl_surface::WlSurface::from_id(&connection, id).map_err(|error| error.to_string())?;
        let (globals, mut queue) = registry_queue_init::<State>(&connection).map_err(|error| error.to_string())?;
        let qh = queue.handle();
        let importer = globals
            .bind::<ZxdgImporterV2, _, _>(&qh, 1..=1, ())
            .map_err(|error| error.to_string())?;
        let imported = importer.import_toplevel(handle.into(), &qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|error| error.to_string())?;
        if state.invalid {
            imported.destroy();
            importer.destroy();
            let _ = connection.flush();
            return Err("Portal parent window no longer exists".into());
        }
        imported.set_parent_of(&surface);
        connection.flush().map_err(|error| error.to_string())?;
        Ok(Some(Self {
            connection,
            imported,
            importer,
        }))
    }
}

impl Drop for Parent {
    fn drop(&mut self) {
        self.imported.destroy();
        self.importer.destroy();
        let _ = self.connection.flush();
    }
}

#[derive(Default)]
struct State {
    invalid: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgImportedV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgImportedV2,
        event: zxdg_imported_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, zxdg_imported_v2::Event::Destroyed) {
            state.invalid = true;
        }
    }
}

delegate_noop!(State: ignore ZxdgImporterV2);
