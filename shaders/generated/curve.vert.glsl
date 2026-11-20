#version 300 es
// HAND-WRITTEN stand-in for the output of tools/build-shaders.ps1 from
// shaders/curve.slang (same uniform block and attribute layout). Regenerate
// on a machine with slangc + naga to replace it.

precision highp float;
precision highp int;

struct View_std140_ {
    vec2 scale;
    vec2 offset;
    float pixel;
};
layout(std140) uniform View_std140_block_0 { View_std140_ _group_0_binding_0; };

layout(location = 0) in vec4 _p2vs_location0;
layout(location = 1) in vec4 _p2vs_location1;
layout(location = 2) in vec4 _p2vs_location2;
smooth out vec2 _vs2fs_location0;
smooth out vec4 _vs2fs_location1;
smooth out vec4 _vs2fs_location2;
smooth out vec4 _vs2fs_location3;

void main() {
    uint vid = uint(gl_VertexID);
    uint i = vid < 3u ? vid : vid - 2u;
    vec2 corner = vec2(float(i & 1u), float(i >> 1u));
    vec4 p01 = _p2vs_location0;
    vec4 p2w = _p2vs_location1;
    float pad = max(p2w.z, p2w.w) + _group_0_binding_0.pixel * 2.0;
    vec2 lo = min(min(p01.xy, p01.zw), p2w.xy) - pad;
    vec2 hi = max(max(p01.xy, p01.zw), p2w.xy) + pad;
    vec2 board = mix(lo, hi, corner);
    gl_Position = vec4(board * _group_0_binding_0.scale + _group_0_binding_0.offset, 0.0, 1.0);
    _vs2fs_location0 = board;
    _vs2fs_location1 = p01;
    _vs2fs_location2 = p2w;
    _vs2fs_location3 = _p2vs_location2;
}
