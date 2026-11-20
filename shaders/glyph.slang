// Glyphs drawn straight from their quadratic Bézier outlines (Slug method,
// Lengyel, JCGT 2017). One quad per glyph; the fragment shader counts how
// the glyph's curves cross a horizontal and a vertical ray from the pixel.
// CPU twin with tests: crates/wall-text/src/coverage.rs. Keep them in step.

struct View
{
    float2 scale;
    float2 offset;
    float pixel;
};

[[vk::binding(0, 0)]] ConstantBuffer<View> view;
// RGBA32F, 2 texels per curve: [p0 p1] [p2 - -]
[[vk::binding(1, 0)]] Texture2D<float4> curve_tex;
// R32F: per glyph (offset, count) per band, then curve index lists.
[[vk::binding(2, 0)]] Texture2D<float> band_tex;

static const uint TEX_WIDTH = 4096;

struct GlyphInstance
{
    float4 rect : RECT;      // glyph box on the board: x, y, w, h (y down)
    float4 em : EM;          // glyph bounds in em: x0, y0, x1, y1 (y up)
    float4 color : COLOR;    // premultiplied RGBA
    float4 data : DATA;      // curve_base, band_base, bands_h, bands_v
};

struct VsOut
{
    float4 position : SV_Position;
    float2 em_pos : EMPOS;
    float4 em_bounds : EMBOUNDS;
    float4 color : COLOR;
    float4 data : DATA;
};

float2 quad_corner(uint vid)
{
    uint i = vid < 3u ? vid : vid - 2u;
    return float2(float(i & 1u), float(i >> 1u));
}

[shader("vertex")]
VsOut vs_main(uint vid : SV_VulkanVertexID, GlyphInstance g)
{
    float2 c = quad_corner(vid);
    // Grow by one screen pixel so edge pixels get shaded.
    float2 em_per_board = (g.em.zw - g.em.xy) / max(g.rect.zw, float2(1e-6, 1e-6));
    float2 grow = float2(view.pixel, view.pixel);
    float2 board = g.rect.xy - grow + c * (g.rect.zw + 2.0 * grow);
    float2 em_pos;
    em_pos.x = g.em.x + (board.x - g.rect.x) * em_per_board.x;
    em_pos.y = g.em.w - (board.y - g.rect.y) * em_per_board.y;

    VsOut o;
    o.position = float4(board * view.scale + view.offset, 0.0, 1.0);
    o.em_pos = em_pos;
    o.em_bounds = g.em;
    o.color = g.color;
    o.data = g.data;
    return o;
}

int2 texel(uint i) { return int2(int(i % TEX_WIDTH), int(i / TEX_WIDTH)); }

uint root_code(float a, float b, float c)
{
    uint shift = (a > 0.0 ? 2u : 0u) + (b > 0.0 ? 4u : 0u) + (c > 0.0 ? 8u : 0u);
    return (0x2E74u >> shift) & 3u;
}

float2 solve(float2 p0, float2 p1, float2 p2)
{
    float2 a = p0 - p1 * 2.0 + p2;
    float2 b = p0 - p1;
    float t1, t2;
    if (abs(a.y) < 1.0 / 65536.0)
    {
        t1 = p0.y * 0.5 / b.y;
        t2 = t1;
    }
    else
    {
        float d = sqrt(max(b.y * b.y - a.y * p0.y, 0.0));
        t1 = (b.y - d) / a.y;
        t2 = (b.y + d) / a.y;
    }
    return float2((a.x * t1 - b.x * 2.0) * t1 + p0.x, (a.x * t2 - b.x * 2.0) * t2 + p0.x);
}

uint band_index(float v, float lo, float hi, uint n)
{
    return uint(clamp(int(floor((v - lo) / (hi - lo) * float(n))), 0, int(n) - 1));
}

// Accumulates coverage along one ray. swap = true casts the ray along +y.
void cast_ray(float2 p, float ppe, uint curve_base, uint band_base, uint band, bool swap,
              inout float cov, inout float wgt)
{
    uint off = uint(band_tex.Load(int3(texel(band_base + 2u * band), 0)));
    uint count = uint(band_tex.Load(int3(texel(band_base + 2u * band + 1u), 0)));
    for (uint k = 0u; k < count; k++)
    {
        uint ci = curve_base + uint(band_tex.Load(int3(texel(band_base + off + k), 0)));
        float4 c01 = curve_tex.Load(int3(texel(ci * 2u), 0));
        float4 c2 = curve_tex.Load(int3(texel(ci * 2u + 1u), 0));
        float2 a = c01.xy - p, b = c01.zw - p, c = c2.xy - p;
        if (swap) { a = a.yx; b = b.yx; c = c.yx; }
        if (max(max(a.x, b.x), c.x) * ppe < -0.5) break;
        uint code = root_code(a.y, b.y, c.y);
        if (code != 0u)
        {
            float2 r = solve(a, b, c) * ppe;
            float s = swap ? -1.0 : 1.0;
            if ((code & 1u) != 0u) { cov += s * saturate(r.x + 0.5); wgt = max(wgt, saturate(1.0 - abs(r.x) * 2.0)); }
            if (code > 1u)         { cov -= s * saturate(r.y + 0.5); wgt = max(wgt, saturate(1.0 - abs(r.y) * 2.0)); }
        }
    }
}

[shader("fragment")]
float4 fs_main(VsOut i) : SV_Target
{
    float2 ppe = 1.0 / max(fwidth(i.em_pos), float2(1e-7, 1e-7));
    uint curve_base = uint(i.data.x), band_base = uint(i.data.y);
    uint bands_h = uint(i.data.z), bands_v = uint(i.data.w);
    float4 eb = i.em_bounds;

    float xcov = 0.0, xwgt = 0.0, ycov = 0.0, ywgt = 0.0;
    cast_ray(i.em_pos, ppe.x, curve_base, band_base, band_index(i.em_pos.y, eb.y, eb.w, bands_h), false, xcov, xwgt);
    cast_ray(i.em_pos, ppe.y, curve_base, band_base, bands_h + band_index(i.em_pos.x, eb.x, eb.z, bands_v), true, ycov, ywgt);

    float blended = abs(xcov * xwgt + ycov * ywgt) / max(xwgt + ywgt, 1.0 / 65536.0);
    float coverage = saturate(max(blended, min(abs(xcov), abs(ycov))));
    return i.color * coverage;
}
