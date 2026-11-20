// Pictures on the board: one textured quad per image, with optional rounded
// corners (SDF mask) and opacity. Texture holds straight-alpha RGBA8 with mips.

struct View
{
    float2 scale;
    float2 offset;
    float pixel;
};

[[vk::binding(0, 0)]] ConstantBuffer<View> view;
[[vk::binding(1, 0)]] Sampler2D image_tex;

struct ImageInstance
{
    float4 rect : RECT;      // x, y, w, h on the board
    float4 params : PARAMS;  // opacity, corner radius, -, -
};

struct VsOut
{
    float4 position : SV_Position;
    float2 uv : UV;
    float2 local : LOCAL;
    float2 half_size : HALF;
    float4 params : PARAMS;
};

float2 quad_corner(uint vid)
{
    uint i = vid < 3u ? vid : vid - 2u;
    return float2(float(i & 1u), float(i >> 1u));
}

[shader("vertex")]
VsOut vs_main(uint vid : SV_VulkanVertexID, ImageInstance img)
{
    float2 c = quad_corner(vid);
    float2 half_size = img.rect.zw * 0.5;
    float2 board = img.rect.xy + c * img.rect.zw;
    VsOut o;
    o.position = float4(board * view.scale + view.offset, 0.0, 1.0);
    o.uv = c;
    o.local = (c * 2.0 - 1.0) * half_size;
    o.half_size = half_size;
    o.params = img.params;
    return o;
}

float sd_round_box(float2 p, float2 half_size, float radius)
{
    float2 q = abs(p) - half_size + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

[shader("fragment")]
float4 fs_main(VsOut i) : SV_Target
{
    float4 c = image_tex.Sample(i.uv);
    float radius = min(i.params.y, min(i.half_size.x, i.half_size.y));
    float d = sd_round_box(i.local, i.half_size, radius);
    float mask = 1.0 - smoothstep(-view.pixel * 0.5, view.pixel * 0.5, d);
    float a = c.a * i.params.x * mask;
    return float4(c.rgb * a, a);
}
