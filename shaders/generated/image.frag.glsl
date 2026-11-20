#version 300 es
// HAND-PORTED from shaders/image.slang until tools/build-shaders runs slangc.
precision highp float;
precision highp int;

layout(std140) uniform View {
    vec2 scale;
    vec2 offset;
    float pixel;
} view;

uniform sampler2D image_tex;

in vec2 _vs2fs_location0;
in vec2 _vs2fs_location1;
in vec2 _vs2fs_location2;
in vec4 _vs2fs_location3;

layout(location = 0) out vec4 out_color;

float sd_round_box(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

void main() {
    vec4 c = texture(image_tex, _vs2fs_location0);
    vec2 half_size = _vs2fs_location2;
    float radius = min(_vs2fs_location3.y, min(half_size.x, half_size.y));
    float d = sd_round_box(_vs2fs_location1, half_size, radius);
    float mask = 1.0 - smoothstep(-view.pixel * 0.5, view.pixel * 0.5, d);
    float a = c.a * _vs2fs_location3.x * mask;
    out_color = vec4(c.rgb * a, a);
}
