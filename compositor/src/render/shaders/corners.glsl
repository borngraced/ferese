// Coordinates and radius are physical pixels. Rust clamps the outer radius.
float rounded_rect_distance(vec2 point, vec4 rect, float radius) {
    vec2 half_size = rect.zw * 0.5;
    vec2 d = abs(point - rect.xy - half_size) - (half_size - vec2(radius));
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
}

float edge_coverage(float distance) {
    // Match Iced/WGPU quad edges: a one-physical-pixel linear ramp.
    return clamp(0.5 - distance, 0.0, 1.0);
}
