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

use crate::{
    Ferese, config::MaterialStyle, private_client::ClientCapabilities, state::ClientState,
};

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedMaterial {
    pub style: MaterialStyle,
    pub opacity: f32,
    pub shadow: [f64; 3],
}

pub(crate) fn resolve_material(
    role: SemanticRole,
    style: MaterialStyle,
    opacity: f32,
) -> ResolvedMaterial {
    let shadow = match role {
        SemanticRole::Panel => [1.0, 5.0, 0.04],
        SemanticRole::PanelElevated | SemanticRole::Popover | SemanticRole::Menu => {
            [2.0, 8.0, 0.07]
        }
        SemanticRole::Hud => [1.0, 4.0, 0.04],
        SemanticRole::Notification | SemanticRole::Modal => [3.0, 10.0, 0.09],
    };
    let opacity = match style {
        MaterialStyle::Solid => 1.0,
        MaterialStyle::Translucent => opacity.clamp(0.0, 1.0),
    };
    ResolvedMaterial {
        style,
        opacity,
        shadow,
    }
}

#[derive(Debug)]
struct SurfaceEffectsState {
    attached: AtomicBool,
    generation: AtomicU64,
    role: Mutex<Option<SemanticRole>>,
    regions: Mutex<Option<Vec<[i32; 5]>>>,
    opacity: Mutex<u32>,
    presentation_supported: AtomicBool,
    dismissing: AtomicBool,
}

impl SurfaceEffectsState {
    fn new() -> Self {
        Self {
            attached: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            role: Mutex::new(None),
            regions: Mutex::new(None),
            opacity: Mutex::new(1000),
            presentation_supported: AtomicBool::new(false),
            dismissing: AtomicBool::new(false),
        }
    }

    fn set_role(&self, role: Option<SemanticRole>) -> bool {
        let mut current = self.role.lock().unwrap();
        if *current != role {
            *current = role;
            self.generation.fetch_add(1, Ordering::Release);
            true
        } else {
            false
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
    display.create_global::<Ferese, FereseEffectsManagerV1, _>(3, ());
}

pub(crate) fn surface_regions(surface: &WlSurface) -> Option<Vec<[i32; 5]>> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<SurfaceEffectsState>()?
            .regions
            .lock()
            .unwrap()
            .clone()
    })
}

fn decode_regions(bytes: &[u8]) -> Option<Vec<[i32; 5]>> {
    if !bytes.len().is_multiple_of(20) || bytes.len() > 32 * 20 {
        return None;
    }
    bytes
        .as_chunks::<20>()
        .0
        .iter()
        .map(|tuple| {
            let values: [i32; 5] =
                std::array::from_fn(|index| i32::from_ne_bytes(tuple.as_chunks::<4>().0[index]));
            (values[2] > 0
                && values[3] > 0
                && values[4] >= 0
                && values.iter().all(|v| v.abs_diff(0) <= 32768))
            .then_some(values)
        })
        .collect()
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
                    let effects = data_init.init(id, SurfaceEffectsUserData::new(surface.clone()));
                    with_states(&surface, |states| {
                        states
                            .data_map
                            .get::<SurfaceEffectsState>()
                            .unwrap()
                            .presentation_supported
                            .store(effects.version() >= 3, Ordering::Release)
                    });
                }
            }
            ferese_effects_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl Dispatch<FereseSurfaceEffectsV1, SurfaceEffectsUserData> for Ferese {
    fn request(
        state: &mut Self,
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
            ferese_surface_effects_v1::Request::SetOpacity { opacity } => {
                let changed = with_states(&surface, |states| {
                    let Some(effects) = states.data_map.get::<SurfaceEffectsState>() else {
                        return false;
                    };
                    if effects.dismissing.load(Ordering::Acquire) {
                        return false;
                    }
                    let mut current = effects.opacity.lock().unwrap();
                    let opacity = opacity.min(1000);
                    if *current == opacity {
                        return false;
                    }
                    *current = opacity;
                    effects.generation.fetch_add(1, Ordering::Release);
                    true
                });
                if changed {
                    crate::backends::direct::render_all(state);
                }
            }
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
                if set_surface_role(&surface, Some(role)) {
                    crate::backends::direct::render_all(state);
                }
            }
            ferese_surface_effects_v1::Request::SetRegions { regions } => {
                let Some(regions) = decode_regions(&regions) else {
                    return;
                };
                let changed = with_states(&surface, |states| {
                    let Some(effects) = states.data_map.get::<SurfaceEffectsState>() else {
                        return false;
                    };
                    let mut old = effects.regions.lock().unwrap();
                    if old.as_ref() == Some(&regions) {
                        return false;
                    }
                    *old = Some(regions);
                    effects.generation.fetch_add(1, Ordering::Release);
                    true
                });
                if changed {
                    crate::backends::direct::render_all(state);
                }
            }
            ferese_surface_effects_v1::Request::ClearRole => {
                if set_surface_role(&surface, None) {
                    crate::backends::direct::render_all(state);
                }
            }
            ferese_surface_effects_v1::Request::Destroy => {
                if detach(&surface) {
                    crate::backends::direct::render_all(state);
                }
            }
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

fn set_surface_role(surface: &WlSurface, role: Option<SemanticRole>) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<SurfaceEffectsState>()
            .is_some_and(|effects| effects.set_role(role))
    })
}

fn detach(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        let Some(effects) = states.data_map.get::<SurfaceEffectsState>() else {
            return false;
        };
        let role_changed = effects.set_role(None);
        effects.dismissing.store(false, Ordering::Release);
        let mut opacity = effects.opacity.lock().unwrap();
        let opacity_changed = *opacity != 1000;
        *opacity = 1000;
        let was_attached = effects.attached.swap(false, Ordering::AcqRel);
        role_changed || was_attached || opacity_changed
    })
}

pub(crate) fn surface_opacity(surface: &WlSurface) -> f32 {
    with_states(surface, |states| {
        states
            .data_map
            .get::<SurfaceEffectsState>()
            .map_or(1.0, |effects| {
                *effects.opacity.lock().unwrap() as f32 / 1000.0
            })
    })
}

pub(crate) fn begin_surface_dismiss(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        states
            .data_map
            .get::<SurfaceEffectsState>()
            .is_some_and(|effects| {
                effects.presentation_supported.load(Ordering::Acquire)
                    && effects.attached.load(Ordering::Acquire)
                    && !effects.dismissing.swap(true, Ordering::AcqRel)
            })
    })
}

pub(crate) fn fade_dismissed_surface(surface: &WlSurface, opacity: f64) {
    with_states(surface, |states| {
        if let Some(effects) = states.data_map.get::<SurfaceEffectsState>() {
            *effects.opacity.lock().unwrap() = (opacity.clamp(0.0, 1.0) * 1000.0).round() as u32;
            effects.generation.fetch_add(1, Ordering::Release);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_regions_are_bounded_and_validated() {
        let region = [12_i32, 8, 120, 48, 11];
        let bytes: Vec<_> = region.into_iter().flat_map(i32::to_ne_bytes).collect();
        assert_eq!(decode_regions(&bytes), Some(vec![region]));
        assert_eq!(decode_regions(&[]), Some(vec![]));
        assert!(decode_regions(&bytes[..19]).is_none());
        assert!(decode_regions(&bytes.repeat(33)).is_none());
        for invalid in [
            [0_i32, 0, 0, 48, 11],
            [0, 0, 120, 48, -1],
            [i32::MIN, 0, 120, 48, 11],
        ] {
            assert!(
                decode_regions(
                    &invalid
                        .into_iter()
                        .flat_map(i32::to_ne_bytes)
                        .collect::<Vec<_>>()
                )
                .is_none()
            );
        }
    }

    #[test]
    fn role_changes_advance_generation_only_when_material_changes() {
        let effects = SurfaceEffectsState::new();
        assert!(effects.set_role(Some(SemanticRole::Panel)));
        assert_eq!(effects.generation.load(Ordering::Acquire), 1);

        assert!(!effects.set_role(Some(SemanticRole::Panel)));
        assert_eq!(effects.generation.load(Ordering::Acquire), 1);

        assert!(effects.set_role(Some(SemanticRole::Popover)));
        assert_eq!(effects.generation.load(Ordering::Acquire), 2);
    }

    #[test]
    fn solid_materials_are_opaque_and_keep_role_specific_elevation() {
        for role in [
            SemanticRole::Panel,
            SemanticRole::Popover,
            SemanticRole::Menu,
            SemanticRole::Hud,
            SemanticRole::Notification,
            SemanticRole::Modal,
        ] {
            let solid = resolve_material(role, MaterialStyle::Solid, 0.6);
            let translucent = resolve_material(role, MaterialStyle::Translucent, 0.6);
            assert_eq!(solid.opacity, 1.0);
            assert_eq!(translucent.opacity, 0.6);
            assert_eq!(solid.shadow, translucent.shadow);
        }
    }
}
