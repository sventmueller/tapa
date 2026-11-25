#include <metal_stdlib>
using namespace metal;

// Mirrors `sprites::Quad`.
struct Quad {
    float4 rect;
    float4 uv;
    float4 color;
    uint4 mode;
};

struct Fragment {
    float4 position [[position]];
    float2 uv;
    float4 color;
    uint mode [[flat]];
};

vertex Fragment quad_vertex(uint corner [[vertex_id]],
                            uint index [[instance_id]],
                            const device Quad *quads [[buffer(0)]],
                            constant float2 &viewport [[buffer(1)]]) {
    Quad quad = quads[index];
    float2 at = float2(corner & 1, corner >> 1);
    float2 point = quad.rect.xy + at * quad.rect.zw;
    Fragment out;
    out.position = float4(point.x / viewport.x * 2.0 - 1.0, 1.0 - point.y / viewport.y * 2.0, 0.0, 1.0);
    out.uv = quad.uv.xy + at * quad.uv.zw;
    out.color = quad.color;
    out.mode = quad.mode.x;
    return out;
}

fragment float4 quad_fragment(Fragment in [[stage_in]], texture2d<float> atlas [[texture(0)]]) {
    constexpr sampler pixels(coord::pixel, filter::nearest);
    if (in.mode == 0) {
        return float4(in.color.rgb * in.color.a, in.color.a);
    }
    float4 sprite = atlas.sample(pixels, in.uv);
    if (in.mode == 1) {
        float cover = sprite.a * in.color.a;
        return float4(in.color.rgb * cover, cover);
    }
    return sprite;
}
