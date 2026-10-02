use smithay::desktop::Window;

use super::*;

pub(crate) fn animated_window_elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Vec<AnimatedWindowRenderElement> {
    output_elements(state, renderer, output, true)
}

pub(crate) fn frame_effect_metrics(_elements: &[AnimatedWindowRenderElement], _scale: f64) -> FrameEffectMetrics {
    FrameEffectMetrics
}

pub(crate) fn output_elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    include_cursor: bool,
) -> Vec<AnimatedWindowRenderElement> {
    let frame = state.sample_frame(output, Duration::ZERO);

    sampled_output_elements(state, renderer, output, include_cursor, &frame)
}

pub(crate) fn sampled_output_elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    include_cursor: bool,
    frame: &crate::state::FrameScene,
) -> Vec<AnimatedWindowRenderElement> {
    if state.session_lock.active() {
        let Some(geometry) = state.space.output_geometry(output) else {
            return Vec::new();
        };
        let scale = output.current_scale().fractional_scale();
        let background = state.session_lock.backgrounds.entry(output.clone()).or_insert_with(|| {
            smithay::backend::renderer::element::solid::SolidColorBuffer::new(geometry.size, [0.0, 0.0, 0.0, 1.0])
        });
        background.resize(geometry.size);
        let mut elements = Vec::new();
        let opacity = state.session_lock.idle_opacity;
        if opacity > 0.0 {
            let overlay = state
                .session_lock
                .idle_overlays
                .entry(output.clone())
                .or_insert_with(|| {
                    smithay::backend::renderer::element::solid::SolidColorBuffer::new(
                        geometry.size,
                        [0.0, 0.0, 0.0, 1.0],
                    )
                });
            overlay.resize(geometry.size);
            elements.push(
                SolidColorRenderElement::from_buffer(overlay, (0, 0), scale, opacity, RenderElementKind::Unspecified)
                    .into(),
            );
        }
        if let Some(surface) = state
            .session_lock
            .surfaces
            .get(output)
            .filter(|surface| surface.alive())
        {
            elements.extend(
                render_elements_from_surface_tree::<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>(
                    renderer,
                    surface.wl_surface(),
                    (0, 0),
                    scale,
                    1.0,
                    RenderElementKind::Unspecified,
                )
                .into_iter()
                .map(AnimatedWindowRenderElement::from),
            );
        }
        elements.push(
            SolidColorRenderElement::from_buffer(background, (0, 0), scale, 1.0, RenderElementKind::Unspecified).into(),
        );
        return elements;
    }
    let Some(output_geometry) = state.space.output_geometry(output) else {
        return Vec::new();
    };
    let scale = output.current_scale().fractional_scale();
    let rounded_clip_program = rounded_clip_program(&mut state.render, renderer);

    let mut elements = if include_cursor {
        cursor_elements(state, renderer, output_geometry, scale)
    } else {
        Vec::new()
    };
    let upper_layers: &[Layer] = if state.output_has_fullscreen_for_frame(output, frame.overview.is_presenting()) {
        &[Layer::Overlay]
    } else {
        &[Layer::Overlay, Layer::Top]
    };
    elements.extend(layer_elements(state, renderer, output, upper_layers));
    if frame.overview.is_presenting() {
        elements.extend(overview_strip_elements(
            state,
            renderer,
            output,
            output_geometry,
            scale,
            frame,
        ));
    }
    let prepare_window = |window: &Window| {
        // Configure immediately, but present the frame/shadow only after
        // the first buffer commit has settled the actual client geometry.
        if !state.window_content_ready(window) {
            return None;
        }
        let id = *state.windows.ids().get(window)?;
        // Scrolling columns may sit outside their monitor's rectangle.
        // They must not reappear on a neighboring output just because
        // their global animated coordinates overlap it.
        if !state.window_belongs_to_output(id, output) {
            return None;
        }
        let sample = frame.windows.get(&id)?;
        let visual = scaled_visual_rect(sample.rect, sample.close_scale);
        let close_alpha = sample.close_alpha;
        let decoration_progress = sample.geometry.decorations.clamp(0.0, 1.0);

        Some((window.clone(), id, sample, visual, decoration_progress, close_alpha))
    };
    let windows = if state.overview.is_active() {
        state
            .windows
            .overview_windows()
            .filter_map(prepare_window)
            .collect::<Vec<_>>()
    } else {
        state
            .space
            .elements()
            .rev()
            .filter_map(prepare_window)
            .collect::<Vec<_>>()
    };

    for (window, id, sample, visual, decoration_progress, close_alpha) in windows {
        let constrain = rounded_visual_rect(visual, output_geometry.loc);
        let material_surface = window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface())
            .filter(|surface| {
                crate::effects::surface_role(surface)
                    .is_some_and(|(role, _)| role == crate::effects::SemanticRole::Modal)
            });
        let window_radius = if material_surface.is_some() {
            state.theme_settings.material_radius
        } else {
            state.theme_settings.window_radius
        } * decoration_progress;
        let corners = RoundedRect::new(visual, output_geometry.loc, scale, window_radius);
        let pixels = corners.rect;
        // Only overview/close intentionally scale the complete application.
        let scale_content = frame.overview.is_presenting() || sample.close_scale != 1.0;
        let behavior = resize_content_behavior(scale_content);

        let dim = sample.dim;
        if let Some(overlay) = window_tint_element(
            &mut state.render,
            renderer,
            id,
            constrain,
            corners,
            [0.0, 0.0, 0.0, dim as f32 * close_alpha],
            false,
            output,
        ) {
            // Front-to-back: dim the application and its border, not its shadow
            // or other windows/layer surfaces. The overlay is input-transparent.
            elements.push(overlay.into());
        }

        if let Some(programs) = rounded_clip_program.clone() {
            let shadow_offset_y = state.theme_settings.shadow_offset_y;
            let shadow_blur = state.theme_settings.shadow_blur;
            let shadow_opacity = state.theme_settings.shadow_opacity;
            let shadow_color = state.theme_settings.shadow_color.0;
            let focus = sample.focus;
            let border_width = state.theme_settings.border_width
                + (state.theme_settings.focus_ring_width - state.theme_settings.border_width) * focus;
            let border_color = state.theme_settings.border_color.0;
            let gradient = state.theme_settings.border_gradient;

            if let Some(border) = window_border_element(
                &mut state.render,
                &state.theme_settings,
                renderer,
                id,
                constrain,
                corners,
                scale,
                border_width,
                border_color,
                gradient,
                focus as f32,
                close_alpha * decoration_progress as f32,
                output,
                &programs,
            ) {
                elements.push(border.into());
            }
            let shadow = window_shadow_element(
                &mut state.render,
                renderer,
                id,
                constrain,
                corners,
                scale,
                shadow_offset_y,
                shadow_blur,
                shadow_opacity * f64::from(close_alpha) * decoration_progress,
                shadow_color,
                output,
                &programs,
            );
            if !scale_content
                && let Some(snapshot) = state.render.snapshot(&id)
                && snapshot.context == renderer.context_id().erased()
                && (snapshot.scale - scale).abs() < 0.001
            {
                let size = snapshot.texture.size();
                let visible = Rectangle::new(pixels.loc, (size.w, size.h).into()).intersection(pixels);
                if let Some(visible) = visible {
                    let clip = framebuffer_clip_rect(
                        pixels,
                        output.current_mode().unwrap().size,
                        output.current_transform().invert(),
                    );
                    elements.push(
                        NativeTextureElement {
                            id: snapshot.id.clone(),
                            commit: snapshot.commit,
                            texture: snapshot.texture.clone(),
                            geometry: visible,
                            source: Rectangle::from_size(Size::from((
                                f64::from(visible.size.w),
                                f64::from(visible.size.h),
                            ))),
                            alpha: crate::presentation::handoff_alpha(snapshot.elapsed) * close_alpha,
                            program: Some(programs.texture.clone()),
                            uniforms: vec![Uniform::new("clip_rect", clip), Uniform::new("radius", corners.radius)],
                        }
                        .into(),
                    );
                }
            }
            elements.extend(rounded_window_elements(
                renderer,
                &window,
                corners,
                scale,
                close_alpha,
                sample.geometry.presentation_changed,
                output,
                programs.clone(),
                behavior,
            ));
            let source = window.geometry().size;
            if material_surface.is_none()
                && !scale_content
                && (source.w < constrain.size.w || source.h < constrain.size.h)
            {
                let mut color = state.theme_settings.surface_base_color.0;
                color[3] = close_alpha;
                if let Some(fill) =
                    window_tint_element(&mut state.render, renderer, id, constrain, corners, color, true, output)
                {
                    // Front-to-back: fill uncovered strips behind the native
                    // content instead of stretching it or exposing wallpaper.
                    elements.push(fill.into());
                }
            } else {
                state.render.clear_tint(id, true);
            }
            if let Some(surface) = material_surface
                && let Some((background, _)) = material_element(
                    state,
                    renderer,
                    output,
                    surface,
                    MaterialSurface {
                        geometry: constrain,
                        corners,
                        index: 0,
                        capture_geometry: constrain,
                        alpha: close_alpha,
                    },
                )
            {
                elements.push(background);
            }
            if let Some(shadow) = shadow {
                elements.push(shadow.into());
            }
        } else {
            elements.extend(constrain_space_element::<GlesRenderer, _, AnimatedWindowRenderElement>(
                renderer,
                &window,
                constrain.loc,
                close_alpha,
                scale,
                constrain,
                ConstrainBehavior {
                    reference: ConstrainReference::Geometry,
                    behavior,
                    align: ConstrainAlign::TOP | ConstrainAlign::LEFT,
                },
            ));
        }
    }
    elements.extend(layer_elements(
        state,
        renderer,
        output,
        &[Layer::Bottom, Layer::Background],
    ));
    state.prepare_theme_wallpaper(renderer, output);
    let progress = state.theme_progress();
    let wallpaper = state.wallpaper.element(renderer, output);
    let has_wallpaper = wallpaper.is_some();
    if let Some(mut wallpaper) = wallpaper {
        wallpaper.alpha = progress;
        elements.push(wallpaper.into());
    }
    if let Some(mut previous) = state.previous_theme_wallpaper(renderer, output) {
        if !has_wallpaper {
            previous.alpha = 1. - progress;
        }
        elements.push(previous.into());
    }
    backdrop::update(&elements, scale);
    elements
}

pub(crate) fn layer_surfaces(output: &Output) -> Vec<LayerSurface> {
    layer_map_for_output(output).layers().cloned().collect()
}

pub(super) fn layer_elements(
    state: &mut Ferese,
    renderer: &mut GlesRenderer,
    output: &Output,
    requested_layers: &[Layer],
) -> Vec<AnimatedWindowRenderElement> {
    let scale = output.current_scale().fractional_scale();
    let Some(mode) = output.current_mode() else {
        return Vec::new();
    };
    let output_crop = Rectangle::<i32, Physical>::from_size(mode.size);
    let layers = {
        let map = layer_map_for_output(output);

        requested_layers
            .iter()
            .flat_map(|requested| {
                map.layers_on(*requested)
                    .rev()
                    .filter_map(|layer| map.layer_geometry(layer).map(|geometry| (geometry, layer.clone())))
            })
            .collect::<Vec<_>>()
    };

    state.render.surfaces.retain(|surface, _| surface.is_alive());

    let mut elements = Vec::new();
    for (geometry, layer) in layers {
        for (popup, offset) in PopupManager::popups_for_surface(layer.wl_surface()) {
            let mut bounds = popup.geometry();
            let content_origin = geometry.loc + offset - bounds.loc;
            bounds.loc = geometry.loc + offset;
            append_material_surface(
                state,
                renderer,
                output,
                popup.wl_surface(),
                bounds,
                content_origin,
                scale,
                output_crop,
                &mut elements,
            );
        }
        append_material_surface(
            state,
            renderer,
            output,
            layer.wl_surface(),
            geometry,
            geometry.loc,
            scale,
            output_crop,
            &mut elements,
        );
    }

    elements
}

pub(super) fn cursor_elements(
    state: &Ferese,
    renderer: &mut GlesRenderer,
    output_geometry: Rectangle<i32, Logical>,
    scale: f64,
) -> Vec<AnimatedWindowRenderElement> {
    if state.input_capture.active() {
        return Vec::new();
    }
    let Some(pointer) = state.seat.get_pointer() else {
        return Vec::new();
    };
    let output_crop = Rectangle::<i32, Logical>::from_size(output_geometry.size).to_physical_precise_round(scale);
    let origin = Point::<i32, Physical>::default();
    let pointer_location = pointer.current_location() - output_geometry.loc.to_f64();
    match &state.cursor_status {
        CursorImageStatus::Surface(surface) => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<CursorImageSurfaceData>()
                    .map(|attributes| attributes.lock().unwrap().hotspot)
                    .unwrap_or_default()
            });
            let physical_location = (pointer_location - hotspot.to_f64()).to_physical_precise_round(scale);

            render_elements_from_surface_tree::<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>(
                renderer,
                surface,
                physical_location,
                scale,
                1.0,
                RenderElementKind::Cursor,
            )
            .into_iter()
            .filter_map(|element| {
                let element = RescaleRenderElement::from_element(element, origin, 1.0);
                let element = RelocateRenderElement::from_element(element, origin, Relocate::Relative);

                CropRenderElement::from_element(element, scale, output_crop).map(Into::into)
            })
            .collect()
        }
        CursorImageStatus::Named(icon) => {
            let Some(cursor) = state.named_cursors.get(icon) else {
                return Vec::new();
            };
            let hotspot = Point::<i32, Physical>::from((cursor.hotspot.x, cursor.hotspot.y));
            let physical_location = pointer_location.to_physical_precise_round(scale) - hotspot;
            let Ok(element) = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                physical_location.to_f64(),
                &cursor.buffer,
                None,
                None,
                None,
                RenderElementKind::Cursor,
            ) else {
                return Vec::new();
            };

            let element = RescaleRenderElement::from_element(element, origin, 1.0);
            let element = RelocateRenderElement::from_element(element, origin, Relocate::Relative);

            CropRenderElement::from_element(element, scale, output_crop)
                .map(|element| vec![element.into()])
                .unwrap_or_default()
        }
        CursorImageStatus::Hidden => Vec::new(),
    }
}
