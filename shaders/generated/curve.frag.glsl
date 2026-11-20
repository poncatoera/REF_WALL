#version 300 es
// HAND-WRITTEN stand-in for the output of tools/build-shaders.ps1 from
// shaders/curve.slang (same uniform block and varying layout). Regenerate
// on a machine with slangc + naga to replace it.

precision highp float;
precision highp int;

struct View_std140_ {
    vec2 scale;
    vec2 offset;
    float pixel;
};
layout(std140) uniform View_std140_block_0 { View_std140_ _group_0_binding_0; };

smooth in vec2 _vs2fs_location0;
smooth in vec4 _vs2fs_location1;
smooth in vec4 _vs2fs_location2;
smooth in vec4 _vs2fs_location3;
layout(location = 0) out vec4 _fs2p_location0;

float dot2(vec2 v) { return dot(v, v); }

float sd_segment(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a, ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-12), 0.0, 1.0);
    return length(pa - ba * h);
}

float sd_bezier(vec2 pos, vec2 A, vec2 B, vec2 C) {
    vec2 a = B - A;
    vec2 b = A - 2.0 * B + C;
    if (dot(b, b) < 1e-8) return sd_segment(pos, A, C);
    vec2 c = a * 2.0;
    vec2 d = A - pos;
    float kk = 1.0 / dot(b, b);
    float kx = kk * dot(a, b);
    float ky = kk * (2.0 * dot(a, a) + dot(d, b)) / 3.0;
    float kz = kk * dot(d, a);
    float p = ky - kx * kx;
    float q = kx * (2.0 * kx * kx - 3.0 * ky) + kz;
    float h = q * q + 4.0 * p * p * p;
    float res;
    if (h >= 0.0) {
        h = sqrt(h);
        vec2 x = (vec2(h, -h) - q) / 2.0;
        vec2 uv = sign(x) * pow(abs(x), vec2(1.0 / 3.0));
        float t = clamp(uv.x + uv.y - kx, 0.0, 1.0);
        res = dot2(d + (c + b * t) * t);
    } else {
        float z = sqrt(-p);
        float v = acos(clamp(q / (p * z * 2.0), -1.0, 1.0)) / 3.0;
        float m = cos(v);
        float n = sin(v) * 1.732050808;
        float t0 = clamp((m + m) * z - kx, 0.0, 1.0);
        float t1 = clamp((-n - m) * z - kx, 0.0, 1.0);
        res = min(dot2(d + (c + b * t0) * t0), dot2(d + (c + b * t1) * t1));
    }
    return sqrt(res);
}

float sd_triangle(vec2 p, vec2 p0, vec2 p1, vec2 p2) {
    vec2 e0 = p1 - p0, e1 = p2 - p1, e2 = p0 - p2;
    vec2 v0 = p - p0, v1 = p - p1, v2 = p - p2;
    vec2 pq0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    vec2 pq1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    vec2 pq2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    float s = sign(e0.x * e2.y - e0.y * e2.x);
    vec2 d = min(min(vec2(dot(pq0, pq0), s * (v0.x * e0.y - v0.y * e0.x)),
                     vec2(dot(pq1, pq1), s * (v1.x * e1.y - v1.y * e1.x))),
                     vec2(dot(pq2, pq2), s * (v2.x * e2.y - v2.y * e2.x)));
    return -sqrt(d.x) * sign(d.y);
}

void main() {
    vec2 board = _vs2fs_location0;
    vec4 p01 = _vs2fs_location1;
    vec4 p2w = _vs2fs_location2;
    vec2 p0 = p01.xy, p1 = p01.zw, p2 = p2w.xy;
    float d = sd_bezier(board, p0, p1, p2) - p2w.z;
    if (p2w.w > 0.0) {
        vec2 dir = p2 - p1;
        if (dot(dir, dir) < 1e-8) dir = p2 - p0;
        dir = normalize(dir);
        vec2 side = vec2(-dir.y, dir.x) * p2w.w * 0.5;
        vec2 base = p2 - dir * p2w.w;
        d = min(d, sd_triangle(board, p2, base + side, base - side));
    }
    float aa = _group_0_binding_0.pixel;
    float coverage = 1.0 - smoothstep(-aa * 0.5, aa * 0.5, d);
    _fs2p_location0 = _vs2fs_location3 * coverage;
}
