struct PushConstants {
    screen_size: vec2<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> screen: PushConstants;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

fn srgb_to_linear(rgb: vec3<f32>) -> vec3<f32> {
    let cutoff = rgb <= vec3<f32>(0.04045);
    let lower = rgb / 12.92;
    let higher = pow((rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(
        2.0 * input.position.x / screen.screen_size.x - 1.0,
        1.0 - 2.0 * input.position.y / screen.screen_size.y,
        0.0,
        1.0,
    );
    output.uv = input.uv;
    output.color = vec4<f32>(srgb_to_linear(input.color.rgb), input.color.a);
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color * textureSample(image, image_sampler, input.uv);
}
