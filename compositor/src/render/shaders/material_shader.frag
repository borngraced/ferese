
precision highp float;
uniform float alpha;
uniform vec4 tint;
uniform vec4 visible_rect;
uniform float material_radius;
uniform float paint_mode;
uniform vec4 shadow_rect;
uniform vec2 shadow_values;
varying vec2 v_coords;

float rounded_distance(vec2 point, vec4 rect) {
    vec2 half_size = rect.zw * 0.5;
    float radius = min(material_radius, min(half_size.x, half_size.y));
    vec2 d = abs(point - rect.xy - half_size) - (half_size - vec2(radius));
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
}
void main() {
    float sdf = rounded_distance(gl_FragCoord.xy, visible_rect);
    float coverage = 1.0 - smoothstep(-0.5, 0.5, sdf);
    if (paint_mode > 0.5) {
        float distance = max(rounded_distance(gl_FragCoord.xy, shadow_rect), 0.0);
        float sigma = max(shadow_values.x * 0.5, 0.5);
        float opacity = exp(-0.5 * distance * distance / (sigma * sigma))
            * shadow_values.y * (1.0 - coverage) * alpha;
        gl_FragColor = vec4(0.0, 0.0, 0.0, opacity);
    } else {
        float opacity = tint.a * coverage * alpha;
        gl_FragColor = vec4(tint.rgb * opacity, opacity);
    }
}
