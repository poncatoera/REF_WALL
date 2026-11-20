#version 300 es
// HAND-PORTED from shaders/glyph.slang until tools/build-shaders runs slangc.
precision highp float;
precision highp int;

layout(std140) uniform View {
    vec2 scale;
    vec2 offset;
    float pixel;
} view;

layout(location = 0) in vec4 rect;
layout(location = 1) in vec4 em;
layout(location = 2) in vec4 color;
layout(location = 3) in vec4 data;

out vec2 _vs2fs_location0;
out vec4 _vs2fs_location1;
out vec4 _vs2fs_location2;
out vec4 _vs2fs_location3;

vec2 quad_corner(uint vid) {
    uint i = vid < 3u ? vid : vid - 2u;
    return vec2(float(i & 1u), float(i >> 1u));
}

void main() {
    vec2 c = quad_corner(uint(gl_VertexID));
    vec2 em_per_board = (em.zw - em.xy) / max(rect.zw, vec2(1e-6));
    vec2 grow = vec2(view.pixel);
    vec2 board = rect.xy - grow + c * (rect.zw + 2.0 * grow);
    vec2 em_pos;
    em_pos.x = em.x + (board.x - rect.x) * em_per_board.x;
    em_pos.y = em.w - (board.y - rect.y) * em_per_board.y;
    gl_Position = vec4(board * view.scale + view.offset, 0.0, 1.0);
    _vs2fs_location0 = em_pos;
    _vs2fs_location1 = em;
    _vs2fs_location2 = color;
    _vs2fs_location3 = data;
}
