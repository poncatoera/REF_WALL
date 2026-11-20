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

layout(location = 0) in vec4 rect;
layout(location = 1) in vec4 fill;
layout(location = 2) in vec4 stroke;
layout(location = 3) in vec2 shape;

out vec2 _vs2fs_location0;
out vec2 _vs2fs_location1;
out vec4 _vs2fs_location2;
out vec4 _vs2fs_location3;
out vec2 _vs2fs_location4;

vec2 quad_corner(uint vid) {
    uint i = vid < 3u ? vid : vid - 2u;
    return vec2(float(i & 1u), float(i >> 1u));
}

void main() {
    vec2 corner = quad_corner(uint(gl_VertexID));
    vec2 half_size = rect.zw * 0.5;
    vec2 local = (corner * 2.0 - 1.0) * (half_size + view.pixel);
    vec2 board = rect.xy + half_size + local;
    gl_Position = vec4(board * view.scale + view.offset, 0.0, 1.0);
    _vs2fs_location0 = local;
    _vs2fs_location1 = half_size;
    _vs2fs_location2 = fill;
    _vs2fs_location3 = stroke;
    _vs2fs_location4 = shape;
}
