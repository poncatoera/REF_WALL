#version 300 es
// HAND-PORTED from shaders/glyph.slang until tools/build-shaders runs slangc.
precision highp float;
precision highp int;
precision highp sampler2D;

uniform sampler2D curve_tex;
uniform sampler2D band_tex;

in vec2 _vs2fs_location0;
in vec4 _vs2fs_location1;
in vec4 _vs2fs_location2;
in vec4 _vs2fs_location3;

layout(location = 0) out vec4 out_color;

const uint TEX_WIDTH = 4096u;

ivec2 texel(uint i) { return ivec2(int(i % TEX_WIDTH), int(i / TEX_WIDTH)); }

uint root_code(float a, float b, float c) {
    uint shift = (a > 0.0 ? 2u : 0u) + (b > 0.0 ? 4u : 0u) + (c > 0.0 ? 8u : 0u);
    return (0x2E74u >> shift) & 3u;
}

vec2 solve(vec2 p0, vec2 p1, vec2 p2) {
    vec2 a = p0 - p1 * 2.0 + p2;
    vec2 b = p0 - p1;
    float t1, t2;
    if (abs(a.y) < 1.0 / 65536.0) {
        t1 = p0.y * 0.5 / b.y;
        t2 = t1;
    } else {
        float d = sqrt(max(b.y * b.y - a.y * p0.y, 0.0));
        t1 = (b.y - d) / a.y;
        t2 = (b.y + d) / a.y;
    }
    return vec2((a.x * t1 - b.x * 2.0) * t1 + p0.x, (a.x * t2 - b.x * 2.0) * t2 + p0.x);
}

uint band_index(float v, float lo, float hi, uint n) {
    return uint(clamp(int(floor((v - lo) / (hi - lo) * float(n))), 0, int(n) - 1));
}

void cast_ray(vec2 p, float ppe, uint curve_base, uint band_base, uint band, bool swap,
              inout float cov, inout float wgt) {
    uint off = uint(texelFetch(band_tex, texel(band_base + 2u * band), 0).r);
    uint count = uint(texelFetch(band_tex, texel(band_base + 2u * band + 1u), 0).r);
    for (uint k = 0u; k < count; k++) {
        uint ci = curve_base + uint(texelFetch(band_tex, texel(band_base + off + k), 0).r);
        vec4 c01 = texelFetch(curve_tex, texel(ci * 2u), 0);
        vec4 c2 = texelFetch(curve_tex, texel(ci * 2u + 1u), 0);
        vec2 a = c01.xy - p, b = c01.zw - p, c = c2.xy - p;
        if (swap) { a = a.yx; b = b.yx; c = c.yx; }
        if (max(max(a.x, b.x), c.x) * ppe < -0.5) break;
        uint code = root_code(a.y, b.y, c.y);
        if (code != 0u) {
            vec2 r = solve(a, b, c) * ppe;
            float s = swap ? -1.0 : 1.0;
            if ((code & 1u) != 0u) { cov += s * clamp(r.x + 0.5, 0.0, 1.0); wgt = max(wgt, clamp(1.0 - abs(r.x) * 2.0, 0.0, 1.0)); }
            if (code > 1u)         { cov -= s * clamp(r.y + 0.5, 0.0, 1.0); wgt = max(wgt, clamp(1.0 - abs(r.y) * 2.0, 0.0, 1.0)); }
        }
    }
}

void main() {
    vec2 em_pos = _vs2fs_location0;
    vec4 eb = _vs2fs_location1;
    vec4 color = _vs2fs_location2;
    vec4 data = _vs2fs_location3;

    vec2 ppe = 1.0 / max(fwidth(em_pos), vec2(1e-7));
    uint curve_base = uint(data.x), band_base = uint(data.y);
    uint bands_h = uint(data.z), bands_v = uint(data.w);

    float xcov = 0.0, xwgt = 0.0, ycov = 0.0, ywgt = 0.0;
    cast_ray(em_pos, ppe.x, curve_base, band_base, band_index(em_pos.y, eb.y, eb.w, bands_h), false, xcov, xwgt);
    cast_ray(em_pos, ppe.y, curve_base, band_base, bands_h + band_index(em_pos.x, eb.x, eb.z, bands_v), true, ycov, ywgt);

    float blended = abs(xcov * xwgt + ycov * ywgt) / max(xwgt + ywgt, 1.0 / 65536.0);
    float coverage = clamp(max(blended, min(abs(xcov), abs(ycov))), 0.0, 1.0);
    out_color = color * coverage;
}
