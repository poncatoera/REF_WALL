// Rounded boxes drawn with an analytic signed distance field.
// Used for cards, notes, selection outlines and UI panels: crisp at any zoom,
// one quad per box, no textures.
//
// Build (not yet run, see README): slangc -> SPIR-V -> naga -> GLSL ES 3.00.

struct View
{
    // Board-to-clip transform: clip = p * scale + offset.
    float2 scale;
    float2 offset;
    // Size of one screen pixel in board units, for anti-aliasing.
    float pixel;
};

[[vk::binding(0, 0)]]
ConstantBuffer<View> view;

struct BoxInstance
{
    float4 rect : RECT;       // x, y, w, h in board units
    float4 fill : FILL;       // premultiplied RGBA
    float4 stroke : STROKE;   // premultiplied RGBA
    float2 shape : SHAPE;     // corner radius, stroke width
};

struct VsOut
{
    float4 position : SV_Position;
    float2 local : LOCAL;      // position relative to the box centre
    float2 half_size : HALF;
    float4 fill : FILL;
    float4 stroke : STROKE;
    float2 shape : SHAPE;
};

// Distance from p to a rounded box centred at the origin.
float sd_round_box(float2 p, float2 half_size, float radius)
{
    float2 q = abs(p) - half_size + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

// Corner of a two-triangle quad for vertex ids 0..5: corners 0,1,2 then 1,2,3,
// where corner i = (i & 1, i >> 1).
float2 quad_corner(uint vid)
{
    uint i = vid < 3u ? vid : vid - 2u;
    return float2(float(i & 1u), float(i >> 1u));
}

[shader("vertex")]
VsOut vs_main(uint vid : SV_VulkanVertexID, BoxInstance box)
{
    float2 corner = quad_corner(vid);
    float2 half_size = box.rect.zw * 0.5;
    // Grow by one pixel so the anti-aliased edge isn't clipped.
    float2 local = (corner * 2.0 - 1.0) * (half_size + view.pixel);
    float2 board = box.rect.xy + half_size + local;

    VsOut o;
    o.position = float4(board * view.scale + view.offset, 0.0, 1.0);
    o.local = local;
    o.half_size = half_size;
    o.fill = box.fill;
    o.stroke = box.stroke;
    o.shape = box.shape;
    return o;
}

[shader("fragment")]
float4 fs_main(VsOut i) : SV_Target
{
    float radius = min(i.shape.x, min(i.half_size.x, i.half_size.y));
    float d = sd_round_box(i.local, i.half_size, radius);
    float aa = view.pixel;
    float coverage = 1.0 - smoothstep(-aa * 0.5, aa * 0.5, d);
    float in_stroke = smoothstep(-aa * 0.5, aa * 0.5, d + i.shape.y);
    float4 color = lerp(i.fill, i.stroke, in_stroke);
    return color * coverage;
}
