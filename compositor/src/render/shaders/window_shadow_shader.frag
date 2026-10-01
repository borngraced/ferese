
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

//_CORNERS_

void main() {
    float signed_distance = rounded_rect_distance(gl_FragCoord.xy, shadow_rect, radius);
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
