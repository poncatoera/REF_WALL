// Pictures on the board: one textured quad per image, with optional rounded
// corners (SDF mask) and opacity, turned / mirrored / black and white.
// Texture holds straight-alpha RGBA8 with mips.

struct View
{
    float2 scale;
    float2 offset;
    float pixel;
};

[[vk::binding(0, 0)]] ConstantBuffer<View> view;
// Separate texture + sampler: naga cannot read Slang's combined Sampler2D
// (OpTypeSampledImage variable). In GL the sampler drops out and filtering
// comes from the texture object.
[[vk::binding(1, 0)]] Texture2D image_tex;
[[vk::binding(2, 0)]] SamplerState image_smp;

struct ImageInstance
{
    float4 rect : RECT;      // x, y, w, h on the board
    float4 params : PARAMS;  // opacity, corner radius, look (turns + 4 mirror + 8 flip), grayscale
    float4 uv_rect : UVRECT; // visible part of the texture: u, v, width, height (crop)
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

// Which point of the (cropped) source shows at corner c of the quad:
// undo the on-screen mirror / flip, then turn back.
float2 source_corner(float2 c, uint look)
{
    if ((look & 4u) != 0u) c.x = 1.0 - c.x;
    if ((look & 8u) != 0u) c.y = 1.0 - c.y;
    uint turns = look & 3u;
    if (turns == 1u) return float2(c.y, 1.0 - c.x);
    if (turns == 2u) return float2(1.0 - c.x, 1.0 - c.y);
    if (turns == 3u) return float2(1.0 - c.y, c.x);
    return c;
}

[shader("vertex")]
VsOut vs_main(uint vid : SV_VulkanVertexID, ImageInstance img)
{
    float2 c = quad_corner(vid);
    float2 half_size = img.rect.zw * 0.5;
    float2 board = img.rect.xy + c * img.rect.zw;
    VsOut o;
    o.position = float4(board * view.scale + view.offset, 0.0, 1.0);
    o.uv = img.uv_rect.xy + source_corner(c, uint(img.params.z + 0.5)) * img.uv_rect.zw;
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
    float4 c = image_tex.Sample(image_smp, i.uv);
    if (i.params.w > 0.5) c.rgb = float3(dot(c.rgb, float3(0.2126, 0.7152, 0.0722)));
    float radius = min(i.params.y, min(i.half_size.x, i.half_size.y));
    float d = sd_round_box(i.local, i.half_size, radius);
    float mask = 1.0 - smoothstep(-view.pixel * 0.5, view.pixel * 0.5, d);
    float a = c.a * i.params.x * mask;
    return float4(c.rgb * a, a);
}
