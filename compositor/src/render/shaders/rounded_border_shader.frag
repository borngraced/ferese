
precision highp float;

uniform float alpha;
uniform vec4 clip_rect;
uniform float radius;
uniform float border_width;
uniform vec4 border_color;
uniform vec4 border_color_to;
uniform vec4 gradient_line;
uniform vec4 focus_color;
uniform vec4 focus_color_to;
uniform vec4 focus_gradient_line;
uniform float focus_mix;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec2 point = gl_FragCoord.xy - clip_rect.xy;
    vec2 half_size = clip_rect.zw * 0.5;
    vec2 distance = abs(point - half_size) - (half_size - vec2(radius));
    float signed_distance = length(max(distance, 0.0))
        + min(max(distance.x, distance.y), 0.0)
        - radius;
    float outer_coverage = 1.0 - smoothstep(-0.5, 0.5, signed_distance);

    vec2 inner_half_size = max(half_size - vec2(border_width), vec2(0.0));
    float inner_radius = max(radius - border_width, 0.0);
    vec2 inner_distance = abs(point - half_size)
        - (inner_half_size - vec2(inner_radius));
    float inner_signed_distance = length(max(inner_distance, 0.0))
        + min(max(inner_distance.x, inner_distance.y), 0.0)
        - inner_radius;
    float inner_coverage = 1.0 - smoothstep(-0.5, 0.5, inner_signed_distance);
    float coverage = max(outer_coverage - inner_coverage, 0.0);
    float progress = clamp(dot(gl_FragCoord.xy - gradient_line.xy, gradient_line.zw), 0.0, 1.0);
    // Interpolate premultiplied endpoints: a transparent endpoint must not
    // leak its RGB into the visible border or create a dark halo.
    vec4 from = vec4(border_color.rgb * border_color.a, border_color.a);
    vec4 to = vec4(border_color_to.rgb * border_color_to.a, border_color_to.a);
    float focus_progress = clamp(dot(gl_FragCoord.xy - focus_gradient_line.xy, focus_gradient_line.zw), 0.0, 1.0);
    vec4 focus_from = vec4(focus_color.rgb * focus_color.a, focus_color.a);
    vec4 focus_to = vec4(focus_color_to.rgb * focus_color_to.a, focus_color_to.a);
    vec4 color = mix(mix(from, to, progress), mix(focus_from, focus_to, focus_progress), focus_mix) * coverage * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
