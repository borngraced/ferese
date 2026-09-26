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
    Ferese,
    config::{GlassQuality, GlassSettings, MaterialStyle},
    private_client::ClientCapabilities,
    state::ClientState,
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
    pub blur: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub noise: f32,
    pub shadow: [f64; 3],
}

pub(crate) fn resolve_material(
    role: SemanticRole,
    style: MaterialStyle,
    settings: GlassSettings,
) -> ResolvedMaterial {
    // Restrained shell shadows in logical pixels: the full-width bar needs
    // less elevation than popovers, while dialogs retain a little more depth.
    let (opacity, blur, saturation, shadow) = match role {
        SemanticRole::Panel => (0.55, 24.0, 1.35, [1.0, 5.0, 0.04]),
        SemanticRole::PanelElevated => (0.55, 24.0, 1.35, [2.0, 8.0, 0.07]),
        SemanticRole::Popover => (0.55, 22.0, 1.35, [2.0, 8.0, 0.07]),
        SemanticRole::Menu => (0.60, 20.0, 1.30, [2.0, 8.0, 0.07]),
        SemanticRole::Hud => (0.65, 18.0, 1.25, [1.0, 4.0, 0.04]),
        SemanticRole::Notification => (0.58, 24.0, 1.35, [3.0, 10.0, 0.09]),
        SemanticRole::Modal => (0.62, 28.0, 1.40, [3.0, 10.0, 0.09]),
    };
    // Preserve the separately defined translucent family when selected
    // directly. Glass degradation keeps the glass role's contrast floor.
    let opacity = if style == MaterialStyle::Translucent {
        match role {
            SemanticRole::Panel => 0.78,
            SemanticRole::Hud | SemanticRole::Modal => 0.88,
            _ => 0.84,
        }
    } else {
        opacity
    };
    let style = if style == MaterialStyle::Glass {
        match settings.quality {
            GlassQuality::Solid => MaterialStyle::Solid,
            GlassQuality::Translucent => MaterialStyle::Translucent,
            _ => style,
        }
    } else {
        style
    };
    let glass = style == MaterialStyle::Glass;
    ResolvedMaterial {
        style,
        opacity: if style == MaterialStyle::Solid {
            1.0
        } else {
            opacity
        },
        blur: if !glass {
            0.0
        } else if settings.quality >= GlassQuality::ReducedBlur {
            (blur * settings.blur_scale).min(12.0)
        } else {
            blur * settings.blur_scale
        },
        saturation: if glass && settings.quality < GlassQuality::NoSaturation {
            saturation
        } else {
            1.0
        },
        brightness: 1.0,
        noise: if glass && settings.grain && settings.quality < GlassQuality::NoGrain {
            0.05
        } else {
            0.0
        },
        shadow,
    }
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
                    state.invalidate_material_scene();
                    crate::backends::direct::render_all(state);
                }
            }
            ferese_surface_effects_v1::Request::ClearRole => {
                if set_surface_role(&surface, None) {
                    state.invalidate_material_scene();
                    crate::backends::direct::render_all(state);
                }
            }
            ferese_surface_effects_v1::Request::Destroy => {
                if detach(&surface) {
                    state.invalidate_material_scene();
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
        let was_attached = effects.attached.swap(false, Ordering::AcqRel);
        role_changed || was_attached
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn styles_preserve_semantics_but_disable_unsupported_effects() {
        let glass = resolve_material(
            SemanticRole::Popover,
            MaterialStyle::Glass,
            GlassSettings::default(),
        );
        let translucent = resolve_material(
            SemanticRole::Popover,
            MaterialStyle::Translucent,
            GlassSettings::default(),
        );
        let solid = resolve_material(
            SemanticRole::Popover,
            MaterialStyle::Solid,
            GlassSettings::default(),
        );

        assert!(glass.blur > 0.0);
        assert!(glass.noise > 0.0);
        assert!(glass.opacity <= translucent.opacity);
        assert_eq!(translucent.blur, 0.0);
        assert_eq!(translucent.noise, 0.0);
        assert_eq!(solid.opacity, 1.0);
        assert_eq!(solid.blur, 0.0);
    }

    #[test]
    fn glass_presets_use_role_specific_tints_and_restrained_shadows() {
        for (role, blur, saturation, opacity, shadow) in [
            (SemanticRole::Panel, 24.0, 1.35, 0.55, [1.0, 5.0, 0.04]),
            (SemanticRole::Popover, 22.0, 1.35, 0.55, [2.0, 8.0, 0.07]),
            (SemanticRole::Menu, 20.0, 1.30, 0.60, [2.0, 8.0, 0.07]),
            (SemanticRole::Hud, 18.0, 1.25, 0.65, [1.0, 4.0, 0.04]),
            (
                SemanticRole::Notification,
                24.0,
                1.35,
                0.58,
                [3.0, 10.0, 0.09],
            ),
            (SemanticRole::Modal, 28.0, 1.40, 0.62, [3.0, 10.0, 0.09]),
        ] {
            let material = resolve_material(role, MaterialStyle::Glass, GlassSettings::default());
            assert_eq!(
                (
                    material.blur,
                    material.saturation,
                    material.opacity,
                    material.shadow
                ),
                (blur, saturation, opacity, shadow)
            );
        }
    }

    #[test]
    fn degradation_is_ordered_and_preserves_contrast_and_elevation() {
        let reference = resolve_material(
            SemanticRole::Modal,
            MaterialStyle::Glass,
            GlassSettings::default(),
        );
        for quality in [
            GlassQuality::Full,
            GlassQuality::NoGrain,
            GlassQuality::NoSaturation,
            GlassQuality::ReducedBlur,
            GlassQuality::Translucent,
            GlassQuality::Solid,
        ] {
            let material = resolve_material(
                SemanticRole::Modal,
                MaterialStyle::Glass,
                GlassSettings {
                    quality,
                    ..GlassSettings::default()
                },
            );
            assert!(material.opacity >= reference.opacity);
            assert_eq!(material.shadow, reference.shadow);
            if quality >= GlassQuality::NoGrain {
                assert_eq!(material.noise, 0.0);
            }
            if quality >= GlassQuality::NoSaturation {
                assert_eq!(material.saturation, 1.0);
            }
            if quality >= GlassQuality::ReducedBlur {
                assert!(material.blur <= 12.0);
            }
            if quality >= GlassQuality::Translucent {
                assert_eq!(material.blur, 0.0);
            }
            if quality == GlassQuality::Solid {
                assert_eq!(material.opacity, 1.0);
            }
        }
    }

    #[test]
    fn popover_uses_the_same_tint_with_a_smaller_blur_than_panel() {
        let panel = resolve_material(
            SemanticRole::Panel,
            MaterialStyle::Glass,
            GlassSettings::default(),
        );
        let popover = resolve_material(
            SemanticRole::Popover,
            MaterialStyle::Glass,
            GlassSettings::default(),
        );

        assert_eq!(popover.opacity, panel.opacity);
        assert!(popover.blur < panel.blur);
    }
}
