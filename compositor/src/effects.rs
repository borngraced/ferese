use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use ferese_protocols::effects::v1::server::{
    ferese_effects_manager_v1::{self, FereseEffectsManagerV1},
    ferese_surface_effects_v1::{self, FereseSurfaceEffectsV1},
};
use smithay::{
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum, Weak,
        backend::ClientId, protocol::wl_surface::WlSurface,
    },
    wayland::compositor::with_states,
};

use crate::{Ferese, private_client::ClientCapabilities, state::ClientState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SemanticRole {
    Panel,
    PanelElevated,
    Popover,
    Menu,
    Notification,
    Hud,
    Modal,
}

#[derive(Debug)]
struct SurfaceEffectsState {
    attached: AtomicBool,
    generation: AtomicU64,
    role: Mutex<Option<SemanticRole>>,
}

impl SurfaceEffectsState {
    fn new() -> Self {
        Self {
            attached: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            role: Mutex::new(None),
        }
    }

    fn set_role(&self, role: Option<SemanticRole>) {
        let mut current = self.role.lock().unwrap();
        if *current != role {
            *current = role;
            self.generation.fetch_add(1, Ordering::Release);
        }
    }
}

#[derive(Debug)]
pub(crate) struct SurfaceEffectsUserData(Mutex<Weak<WlSurface>>);

impl SurfaceEffectsUserData {
    fn new(surface: WlSurface) -> Self {
        Self(Mutex::new(surface.downgrade()))
    }

    fn surface(&self) -> Option<WlSurface> {
        self.0.lock().unwrap().upgrade().ok()
    }
}

pub(crate) fn init_global(display: &DisplayHandle) {
    display.create_global::<Ferese, FereseEffectsManagerV1, _>(1, ());
}

// Consumed by the material renderer in the next M7 slice.
#[allow(dead_code)]
pub(crate) fn surface_role(surface: &WlSurface) -> Option<(SemanticRole, u64)> {
    with_states(surface, |states| {
        let effects = states.data_map.get::<SurfaceEffectsState>()?;
        let role = *effects.role.lock().unwrap();
        role.map(|role| (role, effects.generation.load(Ordering::Acquire)))
    })
}

impl GlobalDispatch<FereseEffectsManagerV1, ()> for Ferese {
    fn bind(
        _state: &mut Self,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<FereseEffectsManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _global_data: &()) -> bool {
        client
            .get_data::<ClientState>()
            .is_some_and(|state| state.capabilities.contains(ClientCapabilities::EFFECTS))
    }
}

impl Dispatch<FereseEffectsManagerV1, ()> for Ferese {
    fn request(
        _state: &mut Self,
        _client: &Client,
        manager: &FereseEffectsManagerV1,
        request: ferese_effects_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ferese_effects_manager_v1::Request::GetSurfaceEffects { id, surface } => {
                let already_attached = with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing_threadsafe(SurfaceEffectsState::new);
                    states
                        .data_map
                        .get::<SurfaceEffectsState>()
                        .unwrap()
                        .attached
                        .swap(true, Ordering::AcqRel)
                });

                if already_attached {
                    manager.post_error(
                        ferese_effects_manager_v1::Error::AlreadyConstructed,
                        "wl_surface already has a Ferese effects object",
                    );
                } else {
                    data_init.init(id, SurfaceEffectsUserData::new(surface));
                }
            }
            ferese_effects_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl Dispatch<FereseSurfaceEffectsV1, SurfaceEffectsUserData> for Ferese {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &FereseSurfaceEffectsV1,
        request: ferese_surface_effects_v1::Request,
        data: &SurfaceEffectsUserData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let Some(surface) = data.surface() else {
            return;
        };

        match request {
            ferese_surface_effects_v1::Request::SetRole { role } => {
                let role = match role {
                    WEnum::Value(ferese_surface_effects_v1::Role::Panel) => SemanticRole::Panel,
                    WEnum::Value(ferese_surface_effects_v1::Role::PanelElevated) => {
                        SemanticRole::PanelElevated
                    }
                    WEnum::Value(ferese_surface_effects_v1::Role::Popover) => SemanticRole::Popover,
                    WEnum::Value(ferese_surface_effects_v1::Role::Menu) => SemanticRole::Menu,
                    WEnum::Value(ferese_surface_effects_v1::Role::Notification) => {
                        SemanticRole::Notification
                    }
                    WEnum::Value(ferese_surface_effects_v1::Role::Hud) => SemanticRole::Hud,
                    WEnum::Value(ferese_surface_effects_v1::Role::Modal) => SemanticRole::Modal,
                    WEnum::Unknown(_) | WEnum::Value(_) => return,
                };
                set_surface_role(&surface, Some(role));
            }
            ferese_surface_effects_v1::Request::ClearRole => set_surface_role(&surface, None),
            ferese_surface_effects_v1::Request::Destroy => detach(&surface),
            _ => unreachable!(),
        }
    }

    fn destroyed(
        _state: &mut Self,
        _client: ClientId,
        _resource: &FereseSurfaceEffectsV1,
        data: &SurfaceEffectsUserData,
    ) {
        if let Some(surface) = data.surface() {
            detach(&surface);
        }
    }
}

fn set_surface_role(surface: &WlSurface, role: Option<SemanticRole>) {
    with_states(surface, |states| {
        if let Some(effects) = states.data_map.get::<SurfaceEffectsState>() {
            effects.set_role(role);
        }
    });
}

fn detach(surface: &WlSurface) {
    with_states(surface, |states| {
        if let Some(effects) = states.data_map.get::<SurfaceEffectsState>() {
            effects.set_role(None);
            effects.attached.store(false, Ordering::Release);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_changes_advance_generation_only_when_material_changes() {
        let effects = SurfaceEffectsState::new();
        effects.set_role(Some(SemanticRole::Panel));
        assert_eq!(effects.generation.load(Ordering::Acquire), 1);

        effects.set_role(Some(SemanticRole::Panel));
        assert_eq!(effects.generation.load(Ordering::Acquire), 1);

        effects.set_role(Some(SemanticRole::Popover));
        assert_eq!(effects.generation.load(Ordering::Acquire), 2);
    }
}
