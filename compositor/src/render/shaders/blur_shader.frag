#version 100
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
uniform vec4 visible_rect;
uniform float material_radius;
uniform vec2 texture_size;
uniform vec2 capture_origin;
uniform float blur_radius;
uniform float presentation_alpha;
uniform float background_opacity;
uniform vec4 tint;
varying vec2 v_coords;
void main() {
    vec2 half_size = visible_rect.zw * 0.5;
    vec2 d = abs(gl_FragCoord.xy - visible_rect.xy - half_size) - (half_size - vec2(material_radius));
    float sdf = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - material_radius;
    float coverage = alpha * presentation_alpha * (1.0 - smoothstep(-0.5, 0.5, sdf));
    if (coverage <= 0.0) { gl_FragColor = vec4(0.0); return; }
    vec2 coords = (gl_FragCoord.xy - capture_origin) / texture_size;
    vec4 color = vec4(0.0);
    float weights = 0.0;
    // A spiral avoids aligning the sampling lattice with wallpaper patterns.
    for (int i = 0; i < 256; i++) {
        float radius = sqrt((float(i) + 0.5) / 256.0);
        float angle = float(i) * 2.39996323;
        vec2 offset = vec2(cos(angle), sin(angle)) * radius * blur_radius / texture_size;
        float weight = exp(-4.5 * radius * radius);
        color += texture2D(tex, coords + offset) * weight;
        weights += weight;
    }
    vec3 background = color.rgb / weights;
    gl_FragColor = vec4(mix(background, tint.rgb, tint.a * background_opacity) * coverage, coverage);
}
