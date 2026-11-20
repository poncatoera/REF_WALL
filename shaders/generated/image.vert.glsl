#version 300 es
// HAND-PORTED from shaders/image.slang until tools/build-shaders runs slangc.
precision highp float;
precision highp int;

layout(std140) uniform View {
    vec2 scale;
    vec2 offset;
    float pixel;
} view;

layout(location = 0) in vec4 rect;
layout(location = 1) in vec4 params;

out vec2 _vs2fs_location0;
out vec2 _vs2fs_location1;
out vec2 _vs2fs_location2;
out vec4 _vs2fs_location3;

vec2 quad_corner(uint vid) {
    uint i = vid < 3u ? vid : vid - 2u;
    return vec2(float(i & 1u), float(i >> 1u));
}

void main() {
    vec2 c = quad_corner(uint(gl_VertexID));
    vec2 half_size = rect.zw * 0.5;
    vec2 board = rect.xy + c * rect.zw;
    gl_Position = vec4(board * view.scale + view.offset, 0.0, 1.0);
    _vs2fs_location0 = c;
    _vs2fs_location1 = (c * 2.0 - 1.0) * half_size;
    _vs2fs_location2 = half_size;
    _vs2fs_location3 = params;
}
