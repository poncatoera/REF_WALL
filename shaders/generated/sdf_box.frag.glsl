#version 300 es
// HAND-PORTED from shaders/sdf_box.slang until tools/build-shaders runs slangc.
// Do not edit by hand once the generated version replaces it.
precision highp float;
precision highp int;

layout(std140) uniform View {
    vec2 scale;
    vec2 offset;
    float pixel;
} view;

in vec2 _vs2fs_location0;
in vec2 _vs2fs_location1;
in vec4 _vs2fs_location2;
in vec4 _vs2fs_location3;
in vec2 _vs2fs_location4;

layout(location = 0) out vec4 out_color;

float sd_round_box(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

void main() {
    vec2 local = _vs2fs_location0;
    vec2 half_size = _vs2fs_location1;
    vec4 fill = _vs2fs_location2;
    vec4 stroke = _vs2fs_location3;
    vec2 shape = _vs2fs_location4;

    float radius = min(shape.x, min(half_size.x, half_size.y));
    float d = sd_round_box(local, half_size, radius);
    float aa = view.pixel;
    float coverage = 1.0 - smoothstep(-aa * 0.5, aa * 0.5, d);
    float in_stroke = smoothstep(-aa * 0.5, aa * 0.5, d + shape.y);
    out_color = mix(fill, stroke, in_stroke) * coverage;
}
