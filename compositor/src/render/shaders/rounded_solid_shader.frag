precision highp float;
uniform float alpha;
uniform vec4 clip_rect;
uniform float radius;
uniform vec4 color;

//_CORNERS_

void main() {
    float distance = rounded_rect_distance(gl_FragCoord.xy, clip_rect, radius);
    // Single-pixel buffer colors are already premultiplied.
    gl_FragColor = color * alpha * edge_coverage(distance);
}
