// Arrows and connectors: one quadratic Bézier stroke per instance, with an
// optional arrowhead at the end, drawn from an exact signed distance (no
// tessellation), so it stays crisp at any zoom.

struct View
{
    float2 scale;
    float2 offset;
    float pixel;
};

[[vk::binding(0, 0)]] ConstantBuffer<View> view;

struct CurveInstance
{
    float4 p01 : P01;      // start xy, control xy (board units)
    float4 p2w : P2W;      // end xy, half stroke width, arrowhead length (0 = none)
    float4 color : COLOR;  // premultiplied RGBA
};

struct VsOut
{
    float4 position : SV_Position;
    float2 board : BOARD;
    // The same for every corner: not interpolated, so a nearly straight
    // piece stays exactly as given (interpolation noise made it bend).
    nointerpolation float4 p01 : P01;
    nointerpolation float4 p2w : P2W;
    nointerpolation float4 color : COLOR;
};

float2 quad_corner(uint vid)
{
    uint i = vid < 3u ? vid : vid - 2u;
    return float2(float(i & 1u), float(i >> 1u));
}

[shader("vertex")]
VsOut vs_main(uint vid : SV_VulkanVertexID, CurveInstance c)
{
    // The curve lies inside the hull of its three points; pad by the stroke,
    // the arrowhead and a pixel for anti-aliasing.
    float pad = max(c.p2w.z, c.p2w.w) + view.pixel * 2.0;
    float2 lo = min(min(c.p01.xy, c.p01.zw), c.p2w.xy) - pad;
    float2 hi = max(max(c.p01.xy, c.p01.zw), c.p2w.xy) + pad;
    float2 board = lerp(lo, hi, quad_corner(vid));
    VsOut o;
    o.position = float4(board * view.scale + view.offset, 0.0, 1.0);
    o.board = board;
    o.p01 = c.p01;
    o.p2w = c.p2w;
    o.color = c.color;
    return o;
}

float dot2(float2 v) { return dot(v, v); }

float sd_segment(float2 p, float2 a, float2 b)
{
    float2 pa = p - a, ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-12), 0.0, 1.0);
    return length(pa - ba * h);
}

// Exact distance to a quadratic Bézier (after Inigo Quilez).
float sd_bezier(float2 pos, float2 A, float2 B, float2 C)
{
    float2 a = B - A;
    float2 b = A - 2.0 * B + C;
    if (dot(b, b) < 1e-8)
        return sd_segment(pos, A, C);
    float2 c = a * 2.0;
    float2 d = A - pos;
    float kk = 1.0 / dot(b, b);
    float kx = kk * dot(a, b);
    float ky = kk * (2.0 * dot(a, a) + dot(d, b)) / 3.0;
    float kz = kk * dot(d, a);
    float p = ky - kx * kx;
    float q = kx * (2.0 * kx * kx - 3.0 * ky) + kz;
    float h = q * q + 4.0 * p * p * p;
    float res;
    if (h >= 0.0)
    {
        h = sqrt(h);
        float2 x = (float2(h, -h) - q) / 2.0;
        float2 uv = sign(x) * pow(abs(x), float2(1.0 / 3.0, 1.0 / 3.0));
        float t = clamp(uv.x + uv.y - kx, 0.0, 1.0);
        res = dot2(d + (c + b * t) * t);
    }
    else
    {
        float z = sqrt(-p);
        float v = acos(clamp(q / (p * z * 2.0), -1.0, 1.0)) / 3.0;
        float m = cos(v);
        float n = sin(v) * 1.732050808;
        float t0 = clamp((m + m) * z - kx, 0.0, 1.0);
        float t1 = clamp((-n - m) * z - kx, 0.0, 1.0);
        res = min(dot2(d + (c + b * t0) * t0), dot2(d + (c + b * t1) * t1));
    }
    // The closed form loses precision when the piece barely bends (thin pen
    // lines showed gaps), so also refine from the nearest point on the chord.
    float2 ch = C - A;
    float t = clamp(dot(pos - A, ch) / max(dot(ch, ch), 1e-12), 0.0, 1.0);
    for (int k = 0; k < 3; k++)
    {
        float2 q = d + (c + b * t) * t;
        float2 dq = c + 2.0 * b * t;
        t = clamp(t - dot(q, dq) / max(dot(dq, dq) + 2.0 * dot(q, b), 1e-12), 0.0, 1.0);
    }
    res = min(res, dot2(d + (c + b * t) * t));
    return sqrt(res);
}

// Signed distance to a triangle (negative inside).
float sd_triangle(float2 p, float2 p0, float2 p1, float2 p2)
{
    float2 e0 = p1 - p0, e1 = p2 - p1, e2 = p0 - p2;
    float2 v0 = p - p0, v1 = p - p1, v2 = p - p2;
    float2 pq0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    float2 pq1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    float2 pq2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    float s = sign(e0.x * e2.y - e0.y * e2.x);
    float2 d = min(min(float2(dot(pq0, pq0), s * (v0.x * e0.y - v0.y * e0.x)),
                       float2(dot(pq1, pq1), s * (v1.x * e1.y - v1.y * e1.x))),
                       float2(dot(pq2, pq2), s * (v2.x * e2.y - v2.y * e2.x)));
    return -sqrt(d.x) * sign(d.y);
}

[shader("fragment")]
float4 fs_main(VsOut i) : SV_Target
{
    float2 p0 = i.p01.xy, p1 = i.p01.zw, p2 = i.p2w.xy;
    float half_width = i.p2w.z, head = i.p2w.w;
    float d = sd_bezier(i.board, p0, p1, p2) - half_width;
    if (head > 0.0)
    {
        float2 dir = p2 - p1;
        if (dot(dir, dir) < 1e-8) dir = p2 - p0;
        dir = normalize(dir);
        float2 side = float2(-dir.y, dir.x) * head * 0.5;
        float2 base = p2 - dir * head;
        d = min(d, sd_triangle(i.board, p2, base + side, base - side));
    }
    float aa = view.pixel;
    float coverage = 1.0 - smoothstep(-aa * 0.5, aa * 0.5, d);
    // A negative fourth number: the solid inside of a see-through stroke
    // (drawn once through the stencil, then its soft edge).
    if (head < 0.0)
    {
        if (coverage < 0.5)
            discard;
        coverage = 1.0;
    }
    if (coverage <= 0.0)
        discard;
    return i.color * coverage;
}
