#include <metal_stdlib>
using namespace metal;

struct Vertex {
    packed_float2 pos;
    packed_float2 uv;
    uchar4 color;
};

struct Raster {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

float srgb_channel_to_linear(float value) {
    return value <= 0.04045
        ? value / 12.92
        : pow((value + 0.055) / 1.055, 2.4);
}

float3 srgb_to_linear(float3 rgb) {
    return float3(
        srgb_channel_to_linear(rgb.r),
        srgb_channel_to_linear(rgb.g),
        srgb_channel_to_linear(rgb.b));
}

vertex Raster vertex_main(
    device const Vertex *vertices [[buffer(0)]],
    constant float2 &screen_size [[buffer(1)]],
    uint vertex_id [[vertex_id]])
{
    Vertex v = vertices[vertex_id];
    Raster out;
    out.position = float4(
        2.0 * v.pos.x / screen_size.x - 1.0,
        1.0 - 2.0 * v.pos.y / screen_size.y,
        0.0,
        1.0);
    out.uv = v.uv;
    float4 color = float4(v.color) / 255.0;
    out.color = float4(srgb_to_linear(color.rgb), color.a);
    return out;
}

fragment float4 fragment_main(
    Raster in [[stage_in]],
    texture2d<float> texture [[texture(0)]],
    sampler texture_sampler [[sampler(0)]])
{
    return in.color * texture.sample(texture_sampler, in.uv);
}
