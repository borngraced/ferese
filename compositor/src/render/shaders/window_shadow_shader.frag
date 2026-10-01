
precision mediump float;

uniform float alpha;
uniform vec4 shadow_rect;
uniform float radius;
uniform float blur;
uniform float opacity;
uniform vec4 shadow_color;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec2 point = gl_FragCoord.xy - shadow_rect.xy;
    vec2 half_size = shadow_rect.zw * 0.5;
    vec2 distance = abs(point - half_size) - (half_size - vec2(radius));
    float signed_distance = length(max(distance, 0.0))
        + min(max(distance.x, distance.y), 0.0)
        - radius;
    float sigma = max(blur * 0.5, 0.5);
    float normalized_distance = max(signed_distance, 0.0) / sigma;
    float coverage = exp(-0.5 * normalized_distance * normalized_distance);
    vec4 color = vec4(shadow_color.rgb * shadow_color.a, shadow_color.a)
        * coverage * opacity * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
