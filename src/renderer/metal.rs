use std::{collections::HashMap, env, ffi::c_void, mem::size_of_val};

use core_graphics_types::geometry::CGSize;
use egui::{
    ClippedPrimitive, ImageData, TextureFilter, TextureId, TextureOptions, TextureWrapMode,
    TexturesDelta,
    epaint::{ImageDelta, Primitive},
};
use metal::{
    CompileOptions, Device, MTLBlendFactor, MTLClearColor, MTLIndexType, MTLLoadAction,
    MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLResourceOptions, MTLSamplerAddressMode,
    MTLSamplerMinMagFilter, MTLScissorRect, MTLStorageMode, MTLStoreAction, MTLTextureType,
    MTLTextureUsage, MetalLayer, RenderPassDescriptor, RenderPipelineDescriptor,
    RenderPipelineState, SamplerDescriptor, SamplerState, Texture, TextureDescriptor,
};
use objc::{
    Message,
    runtime::{Object, Sel, YES},
};
use winit::{
    dpi::PhysicalSize,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

use super::choose_adapter;

const SHADER: &str = r#"
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
"#;

struct TextureEntry {
    texture: Texture,
    sampler: SamplerState,
}

pub struct Renderer {
    device: Device,
    command_queue: metal::CommandQueue,
    layer: MetalLayer,
    pipeline: RenderPipelineState,
    textures: HashMap<TextureId, TextureEntry>,
    size: PhysicalSize<u32>,
    adapter_name: String,
}

impl Renderer {
    pub fn new(window: &Window) -> Result<Self, String> {
        let devices = Device::all();
        let choices: Vec<_> = devices
            .iter()
            .map(|device| (device.name(), device.is_low_power(), device.is_headless()))
            .collect();
        let requested = env::var("EDITUR_GPU_DEVICE").ok();
        let index = choose_adapter(&choices, requested.as_deref()).ok_or_else(|| {
            requested.map_or_else(
                || "Metal: no non-headless device is available".to_owned(),
                |name| format!("Metal: no device matches EDITUR_GPU_DEVICE={name:?}"),
            )
        })?;
        let device = devices
            .into_iter()
            .nth(index)
            .ok_or_else(|| "Metal: selected device disappeared".to_owned())?;
        let adapter_name = device.name().to_owned();

        let compile_options = CompileOptions::new();
        let library = device
            .new_library_with_source(SHADER, &compile_options)
            .map_err(|error| format!("Metal shader compilation failed: {error}"))?;
        let vertex = library
            .get_function("vertex_main", None)
            .map_err(|error| format!("Metal vertex shader is unavailable: {error}"))?;
        let fragment = library
            .get_function("fragment_main", None)
            .map_err(|error| format!("Metal fragment shader is unavailable: {error}"))?;
        let descriptor = RenderPipelineDescriptor::new();
        descriptor.set_vertex_function(Some(&vertex));
        descriptor.set_fragment_function(Some(&fragment));
        let attachment = descriptor
            .color_attachments()
            .object_at(0)
            .ok_or_else(|| "Metal: color attachment 0 is unavailable".to_owned())?;
        attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm_sRGB);
        attachment.set_blending_enabled(true);
        attachment.set_source_rgb_blend_factor(MTLBlendFactor::One);
        attachment.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        attachment.set_source_alpha_blend_factor(MTLBlendFactor::One);
        attachment.set_destination_alpha_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        let pipeline = device
            .new_render_pipeline_state(&descriptor)
            .map_err(|error| format!("Metal pipeline creation failed: {error}"))?;

        let mut layer = MetalLayer::new();
        layer.set_device(&device);
        layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm_sRGB);
        layer.set_presents_with_transaction(false);
        attach_layer(window, &mut layer)?;
        let size = window.inner_size();
        layer.set_drawable_size(CGSize::new(size.width as f64, size.height as f64));

        let command_queue = device.new_command_queue();
        Ok(Self {
            device,
            command_queue,
            layer,
            pipeline,
            textures: HashMap::new(),
            size,
            adapter_name,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub const fn backend_name(&self) -> &'static str {
        "Metal"
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
        self.layer
            .set_drawable_size(CGSize::new(size.width as f64, size.height as f64));
    }

    pub fn render(
        &mut self,
        pixels_per_point: f32,
        primitives: &[ClippedPrimitive],
        textures_delta: &TexturesDelta,
    ) -> Result<(), String> {
        for (id, delta) in &textures_delta.set {
            self.update_texture(*id, delta)?;
        }

        if primitives
            .iter()
            .any(|primitive| matches!(primitive.primitive, Primitive::Callback(_)))
        {
            return Err("Metal: egui paint callbacks are unsupported".to_owned());
        }

        if self.size.width == 0 || self.size.height == 0 {
            self.free_textures(&textures_delta.free);
            return Ok(());
        }
        let Some(drawable) = self.layer.next_drawable() else {
            self.free_textures(&textures_delta.free);
            return Ok(());
        };

        let pass = RenderPassDescriptor::new();
        let attachment = pass
            .color_attachments()
            .object_at(0)
            .ok_or_else(|| "Metal: color attachment 0 is unavailable".to_owned())?;
        attachment.set_texture(Some(drawable.texture()));
        attachment.set_load_action(MTLLoadAction::Clear);
        attachment.set_clear_color(MTLClearColor::new(0.055, 0.063, 0.082, 1.0));
        attachment.set_store_action(MTLStoreAction::Store);

        let command_buffer = self.command_queue.new_command_buffer();
        let encoder = command_buffer.new_render_command_encoder(pass);
        encoder.set_render_pipeline_state(&self.pipeline);
        let screen_size = [
            self.size.width as f32 / pixels_per_point,
            self.size.height as f32 / pixels_per_point,
        ];
        encoder.set_vertex_bytes(
            1,
            size_of_val(&screen_size) as u64,
            screen_size.as_ptr().cast(),
        );

        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                continue;
            }
            let Some(scissor) = scissor_rect(primitive.clip_rect, pixels_per_point, self.size)
            else {
                continue;
            };
            let texture = self
                .textures
                .get(&mesh.texture_id)
                .ok_or_else(|| format!("Metal: missing egui texture {:?}", mesh.texture_id))?;
            let vertices: &[u8] = bytemuck::cast_slice(mesh.vertices.as_slice());
            let indices: &[u8] = bytemuck::cast_slice(mesh.indices.as_slice());
            let vertex_buffer = self.device.new_buffer_with_data(
                vertices.as_ptr().cast(),
                vertices.len() as u64,
                MTLResourceOptions::StorageModeShared,
            );
            let index_buffer = self.device.new_buffer_with_data(
                indices.as_ptr().cast(),
                indices.len() as u64,
                MTLResourceOptions::StorageModeShared,
            );
            encoder.set_scissor_rect(scissor);
            encoder.set_vertex_buffer(0, Some(&vertex_buffer), 0);
            encoder.set_fragment_texture(0, Some(&texture.texture));
            encoder.set_fragment_sampler_state(0, Some(&texture.sampler));
            encoder.draw_indexed_primitives(
                MTLPrimitiveType::Triangle,
                mesh.indices.len() as u64,
                MTLIndexType::UInt32,
                &index_buffer,
                0,
            );
        }

        encoder.end_encoding();
        command_buffer.present_drawable(drawable);
        command_buffer.commit();
        self.free_textures(&textures_delta.free);
        Ok(())
    }

    fn update_texture(&mut self, id: TextureId, delta: &ImageDelta) -> Result<(), String> {
        let ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        let bytes: &[u8] = bytemuck::cast_slice(image.pixels.as_slice());
        let [x, y] = delta.pos.unwrap_or([0, 0]);

        if delta.pos.is_none() {
            let descriptor = TextureDescriptor::new();
            descriptor.set_texture_type(MTLTextureType::D2);
            descriptor.set_pixel_format(MTLPixelFormat::RGBA8Unorm_sRGB);
            descriptor.set_width(width as u64);
            descriptor.set_height(height as u64);
            descriptor.set_storage_mode(MTLStorageMode::Shared);
            descriptor.set_usage(MTLTextureUsage::ShaderRead);
            let texture = self.device.new_texture(&descriptor);
            let sampler = self.sampler(delta.options);
            self.textures.insert(id, TextureEntry { texture, sampler });
        }

        let entry = self
            .textures
            .get_mut(&id)
            .ok_or_else(|| format!("Metal: partial update for missing egui texture {id:?}"))?;
        if x + width > entry.texture.width() as usize
            || y + height > entry.texture.height() as usize
        {
            return Err(format!(
                "Metal: egui texture update for {id:?} is out of bounds"
            ));
        }
        entry.texture.replace_region(
            MTLRegion::new_2d(x as u64, y as u64, width as u64, height as u64),
            0,
            bytes.as_ptr().cast::<c_void>(),
            (width * 4) as u64,
        );
        Ok(())
    }

    fn sampler(&self, options: TextureOptions) -> SamplerState {
        let descriptor = SamplerDescriptor::new();
        descriptor.set_mag_filter(filter(options.magnification));
        descriptor.set_min_filter(filter(options.minification));
        let address = match options.wrap_mode {
            TextureWrapMode::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
            TextureWrapMode::Repeat => MTLSamplerAddressMode::Repeat,
            TextureWrapMode::MirroredRepeat => MTLSamplerAddressMode::MirrorRepeat,
        };
        descriptor.set_address_mode_s(address);
        descriptor.set_address_mode_t(address);
        self.device.new_sampler(&descriptor)
    }

    fn free_textures(&mut self, ids: &[TextureId]) {
        for id in ids {
            self.textures.remove(id);
        }
    }
}

fn filter(filter: TextureFilter) -> MTLSamplerMinMagFilter {
    match filter {
        TextureFilter::Nearest => MTLSamplerMinMagFilter::Nearest,
        TextureFilter::Linear => MTLSamplerMinMagFilter::Linear,
    }
}

fn scissor_rect(
    rect: egui::Rect,
    pixels_per_point: f32,
    size: PhysicalSize<u32>,
) -> Option<MTLScissorRect> {
    let min_x = (rect.min.x * pixels_per_point)
        .round()
        .clamp(0.0, size.width as f32) as u64;
    let min_y = (rect.min.y * pixels_per_point)
        .round()
        .clamp(0.0, size.height as f32) as u64;
    let max_x = (rect.max.x * pixels_per_point)
        .round()
        .clamp(min_x as f32, size.width as f32) as u64;
    let max_y = (rect.max.y * pixels_per_point)
        .round()
        .clamp(min_y as f32, size.height as f32) as u64;
    (max_x > min_x && max_y > min_y).then_some(MTLScissorRect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

fn attach_layer(window: &Window, layer: &mut MetalLayer) -> Result<(), String> {
    let handle = window
        .window_handle()
        .map_err(|error| format!("Metal: cannot obtain the AppKit window handle: {error}"))?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err("Metal: winit did not provide an AppKit window handle".to_owned());
    };
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<Object>() };
    unsafe {
        view.send_message::<_, ()>(Sel::register("setWantsLayer:"), (YES,))
            .map_err(|error| format!("Metal: cannot enable the AppKit backing layer: {error}"))?;
        view.send_message::<_, ()>(
            Sel::register("setLayer:"),
            ((layer.as_mut() as *mut metal::MetalLayerRef).cast::<Object>(),),
        )
        .map_err(|error| format!("Metal: cannot attach CAMetalLayer: {error}"))?;
    }
    Ok(())
}
