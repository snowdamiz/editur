use std::{collections::HashMap, env, ffi::c_void, mem::size_of, ptr};

use egui::{
    ClippedPrimitive, ImageData, TextureFilter, TextureId, TextureOptions, TextureWrapMode,
    TexturesDelta,
    epaint::{ImageDelta, Primitive},
};
use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct3D::Fxc::D3DCompile,
            Direct3D::{D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob},
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{Interface, PCSTR},
};
use winit::{
    dpi::PhysicalSize,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

use super::choose_adapter;

const FRAME_COUNT: u32 = 2;
const MAX_TEXTURES: u32 = 1024;
const SHADER: &[u8] = br#"
cbuffer Screen : register(b0) { float2 screen_size; };
Texture2D image : register(t0);
SamplerState image_sampler : register(s0);

struct VertexInput {
    float2 position : POSITION;
    float2 uv : TEXCOORD;
    float4 color : COLOR;
};
struct VertexOutput {
    float4 position : SV_POSITION;
    float2 uv : TEXCOORD;
    float4 color : COLOR;
};

float srgb_channel_to_linear(float value) {
    return value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4);
}
float3 srgb_to_linear(float3 rgb) {
    return float3(
        srgb_channel_to_linear(rgb.r),
        srgb_channel_to_linear(rgb.g),
        srgb_channel_to_linear(rgb.b));
}

VertexOutput vertex_main(VertexInput input) {
    VertexOutput output;
    output.position = float4(
        2.0 * input.position.x / screen_size.x - 1.0,
        1.0 - 2.0 * input.position.y / screen_size.y,
        0.0,
        1.0);
    output.uv = input.uv;
    output.color = float4(srgb_to_linear(input.color.rgb), input.color.a);
    return output;
}

float4 fragment_main(VertexOutput input) : SV_TARGET {
    return input.color * image.Sample(image_sampler, input.uv);
}
"#;

struct TextureEntry {
    resource: ID3D12Resource,
    slot: u32,
    width: u32,
    height: u32,
}

pub struct Renderer {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    swapchain: IDXGISwapChain3,
    allocator: ID3D12CommandAllocator,
    commands: ID3D12GraphicsCommandList,
    root_signature: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    rtv_heap: ID3D12DescriptorHeap,
    srv_heap: ID3D12DescriptorHeap,
    sampler_heap: ID3D12DescriptorHeap,
    rtv_step: u32,
    srv_step: u32,
    sampler_step: u32,
    targets: Vec<ID3D12Resource>,
    fence: ID3D12Fence,
    fence_value: u64,
    textures: HashMap<TextureId, TextureEntry>,
    free_slots: Vec<u32>,
    next_slot: u32,
    size: PhysicalSize<u32>,
    adapter_name: String,
}

impl Renderer {
    pub fn new(window: &Window) -> Result<Self, String> {
        let validation = env::var("EDITUR_GPU_VALIDATION").as_deref() == Ok("1");
        if validation {
            unsafe {
                let mut debug = None;
                D3D12GetDebugInterface(&mut debug)
                    .map_err(|error| format!("D3D12: debug layer is unavailable: {error}"))?;
                let debug: ID3D12Debug =
                    debug.ok_or_else(|| "D3D12: debug interface returned no object".to_owned())?;
                debug.EnableDebugLayer();
            }
        }
        let flags = if validation {
            DXGI_CREATE_FACTORY_DEBUG
        } else {
            DXGI_CREATE_FACTORY_FLAGS(0)
        };
        let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory2(flags) }
            .map_err(|error| format!("D3D12: DXGI factory creation failed: {error}"))?;
        let requested = env::var("EDITUR_GPU_DEVICE").ok();
        let mut adapters = Vec::new();
        for index in 0.. {
            let adapter: IDXGIAdapter1 = match unsafe {
                factory.EnumAdapterByGpuPreference(index, DXGI_GPU_PREFERENCE_MINIMUM_POWER)
            } {
                Ok(adapter) => adapter,
                Err(_) => break,
            };
            let description = unsafe { adapter.GetDesc1() }
                .map_err(|error| format!("D3D12: cannot inspect adapter: {error}"))?;
            if description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let mut device: Option<ID3D12Device> = None;
            if unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }.is_ok() {
                let end = description
                    .Description
                    .iter()
                    .position(|character| *character == 0)
                    .unwrap_or(description.Description.len());
                let name = String::from_utf16_lossy(&description.Description[..end]);
                if let Some(device) = device {
                    adapters.push((adapter, device, name));
                }
            }
        }
        let choices: Vec<_> = adapters
            .iter()
            .enumerate()
            .map(|(index, (_, _, name))| (name.as_str(), index == 0, false))
            .collect();
        let selected = choose_adapter(&choices, requested.as_deref()).ok_or_else(|| {
            requested.map_or_else(
                || "D3D12: no hardware adapter supports feature level 11_0".to_owned(),
                |name| format!("D3D12: no usable adapter matches EDITUR_GPU_DEVICE={name:?}"),
            )
        })?;
        let (adapter, device, adapter_name) = adapters.swap_remove(selected);
        let queue_description = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            ..Default::default()
        };
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&queue_description) }
            .map_err(|error| format!("D3D12: command queue creation failed: {error}"))?;
        let hwnd = hwnd(window)?;
        let size = window.inner_size();
        let swapchain_description = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.width,
            Height: size.height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: FRAME_COUNT,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: 0,
        };
        let swapchain1 = unsafe {
            factory.CreateSwapChainForHwnd(&queue, hwnd, &swapchain_description, None, None)
        }
        .map_err(|error| format!("D3D12: swapchain creation failed: {error}"))?;
        unsafe { factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER) }
            .map_err(|error| format!("D3D12: cannot configure window association: {error}"))?;
        let swapchain: IDXGISwapChain3 = swapchain1
            .cast()
            .map_err(|error| format!("D3D12: IDXGISwapChain3 is unavailable: {error}"))?;
        let allocator: ID3D12CommandAllocator =
            unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }
                .map_err(|error| format!("D3D12: command allocator creation failed: {error}"))?;

        let ranges = [
            D3D12_DESCRIPTOR_RANGE {
                RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                NumDescriptors: 1,
                BaseShaderRegister: 0,
                RegisterSpace: 0,
                OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
            },
            D3D12_DESCRIPTOR_RANGE {
                RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
                NumDescriptors: 1,
                BaseShaderRegister: 0,
                RegisterSpace: 0,
                OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
            },
        ];
        let parameters = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        ShaderRegister: 0,
                        RegisterSpace: 0,
                        Num32BitValues: 2,
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &ranges[0],
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &ranges[1],
                    },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
        ];
        let root_description = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: parameters.len() as u32,
            pParameters: parameters.as_ptr(),
            Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
            ..Default::default()
        };
        let mut root_blob = None;
        let mut root_error = None;
        unsafe {
            D3D12SerializeRootSignature(
                &root_description,
                D3D_ROOT_SIGNATURE_VERSION_1,
                &mut root_blob,
                Some(&mut root_error),
            )
        }
        .map_err(|error| {
            format!(
                "D3D12: root signature serialization failed: {}",
                blob_error(root_error, error)
            )
        })?;
        let root_blob = root_blob
            .ok_or_else(|| "D3D12: root signature compiler returned no data".to_owned())?;
        let root_signature: ID3D12RootSignature = unsafe {
            device.CreateRootSignature(
                0,
                std::slice::from_raw_parts(
                    root_blob.GetBufferPointer().cast::<u8>(),
                    root_blob.GetBufferSize(),
                ),
            )
        }
        .map_err(|error| format!("D3D12: root signature creation failed: {error}"))?;
        let vertex_shader = compile_shader(b"vertex_main\0", b"vs_5_0\0")?;
        let fragment_shader = compile_shader(b"fragment_main\0", b"ps_5_0\0")?;
        let input_elements = [
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(c"POSITION".as_ptr().cast()),
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                ..Default::default()
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(c"TEXCOORD".as_ptr().cast()),
                Format: DXGI_FORMAT_R32G32_FLOAT,
                AlignedByteOffset: 8,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                ..Default::default()
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(c"COLOR".as_ptr().cast()),
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                AlignedByteOffset: 16,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                ..Default::default()
            },
        ];
        let mut blend = D3D12_BLEND_DESC::default();
        blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D12_BLEND_ONE,
            DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: D3D12_BLEND_ONE,
            DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
            ..Default::default()
        };
        let pipeline_description = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::ManuallyDrop::new(Some(root_signature.clone())),
            VS: bytecode(&vertex_shader),
            PS: bytecode(&fragment_shader),
            BlendState: blend,
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            },
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: false.into(),
                StencilEnable: false.into(),
                ..Default::default()
            },
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: input_elements.as_ptr(),
                NumElements: input_elements.len() as u32,
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            RTVFormats: {
                let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
                formats[0] = DXGI_FORMAT_B8G8R8A8_UNORM_SRGB;
                formats
            },
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        let pipeline: ID3D12PipelineState =
            unsafe { device.CreateGraphicsPipelineState(&pipeline_description) }
                .map_err(|error| format!("D3D12: graphics pipeline creation failed: {error}"))?;
        let commands: ID3D12GraphicsCommandList = unsafe {
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, &pipeline)
        }
        .map_err(|error| format!("D3D12: command list creation failed: {error}"))?;
        unsafe { commands.Close() }
            .map_err(|error| format!("D3D12: closing initial command list failed: {error}"))?;
        let rtv_heap =
            descriptor_heap(&device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, FRAME_COUNT, false)?;
        let srv_heap = descriptor_heap(
            &device,
            D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            MAX_TEXTURES,
            true,
        )?;
        let sampler_heap = descriptor_heap(
            &device,
            D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
            MAX_TEXTURES,
            true,
        )?;
        let rtv_step =
            unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) };
        let srv_step = unsafe {
            device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
        };
        let sampler_step =
            unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER) };
        let targets = create_targets(&device, &swapchain, &rtv_heap, rtv_step)?;
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }
            .map_err(|error| format!("D3D12: fence creation failed: {error}"))?;
        drop(adapter);

        Ok(Self {
            device,
            queue,
            swapchain,
            allocator,
            commands,
            root_signature,
            pipeline,
            rtv_heap,
            srv_heap,
            sampler_heap,
            rtv_step,
            srv_step,
            sampler_step,
            targets,
            fence,
            fence_value: 0,
            textures: HashMap::new(),
            free_slots: Vec::new(),
            next_slot: 0,
            size,
            adapter_name,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub const fn backend_name(&self) -> &'static str {
        "D3D12"
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
    }

    pub fn render(
        &mut self,
        pixels_per_point: f32,
        primitives: &[ClippedPrimitive],
        textures_delta: &TexturesDelta,
    ) -> Result<(), String> {
        if primitives
            .iter()
            .any(|primitive| matches!(primitive.primitive, Primitive::Callback(_)))
        {
            return Err("D3D12: egui paint callbacks are unsupported".to_owned());
        }
        for (id, delta) in &textures_delta.set {
            self.update_texture(*id, delta)?;
        }
        if self.size.width == 0 || self.size.height == 0 {
            self.free_textures(&textures_delta.free);
            return Ok(());
        }
        let description = unsafe { self.swapchain.GetDesc1() }
            .map_err(|error| format!("D3D12: cannot inspect swapchain: {error}"))?;
        if description.Width != self.size.width || description.Height != self.size.height {
            self.recreate_swapchain()?;
        }
        unsafe {
            self.allocator
                .Reset()
                .map_err(|error| format!("D3D12: command allocator reset failed: {error}"))?;
            self.commands
                .Reset(&self.allocator, &self.pipeline)
                .map_err(|error| format!("D3D12: command list reset failed: {error}"))?;
            self.commands.SetGraphicsRootSignature(&self.root_signature);
            self.commands.SetDescriptorHeaps(&[
                Some(self.srv_heap.clone()),
                Some(self.sampler_heap.clone()),
            ]);
            let screen = [
                self.size.width as f32 / pixels_per_point,
                self.size.height as f32 / pixels_per_point,
            ];
            self.commands
                .SetGraphicsRoot32BitConstants(0, 2, screen.as_ptr().cast::<c_void>(), 0);
            self.commands
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        }
        let frame = unsafe { self.swapchain.GetCurrentBackBufferIndex() };
        transition(
            &self.commands,
            &self.targets[frame as usize],
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        );
        let rtv = cpu_handle(&self.rtv_heap, self.rtv_step, frame);
        unsafe {
            self.commands.OMSetRenderTargets(1, Some(&rtv), false, None);
            self.commands
                .ClearRenderTargetView(rtv, &[0.055, 0.063, 0.082, 1.0], None);
            self.commands.RSSetViewports(&[D3D12_VIEWPORT {
                Width: self.size.width as f32,
                Height: self.size.height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            }]);
        }
        let mut uploads = Vec::new();
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
                .ok_or_else(|| format!("D3D12: missing egui texture {:?}", mesh.texture_id))?;
            let vertex_bytes: &[u8] = bytemuck::cast_slice(mesh.vertices.as_slice());
            let index_bytes: &[u8] = bytemuck::cast_slice(mesh.indices.as_slice());
            let vertex = upload_resource(&self.device, vertex_bytes)?;
            let index = upload_resource(&self.device, index_bytes)?;
            let vertex_view = D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: unsafe { vertex.GetGPUVirtualAddress() },
                SizeInBytes: vertex_bytes.len() as u32,
                StrideInBytes: size_of::<egui::epaint::Vertex>() as u32,
            };
            let index_view = D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: unsafe { index.GetGPUVirtualAddress() },
                SizeInBytes: index_bytes.len() as u32,
                Format: DXGI_FORMAT_R32_UINT,
            };
            unsafe {
                self.commands.RSSetScissorRects(&[scissor]);
                self.commands.IASetVertexBuffers(0, Some(&[vertex_view]));
                self.commands.IASetIndexBuffer(Some(&index_view));
                self.commands.SetGraphicsRootDescriptorTable(
                    1,
                    gpu_handle(&self.srv_heap, self.srv_step, texture.slot),
                );
                self.commands.SetGraphicsRootDescriptorTable(
                    2,
                    gpu_handle(&self.sampler_heap, self.sampler_step, texture.slot),
                );
                self.commands
                    .DrawIndexedInstanced(mesh.indices.len() as u32, 1, 0, 0, 0);
            }
            uploads.push(vertex);
            uploads.push(index);
        }
        transition(
            &self.commands,
            &self.targets[frame as usize],
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_PRESENT,
        );
        unsafe {
            self.commands
                .Close()
                .map_err(|error| format!("D3D12: closing command list failed: {error}"))?;
            let list: ID3D12CommandList = self
                .commands
                .cast()
                .map_err(|error| format!("D3D12: command list interface failed: {error}"))?;
            self.queue.ExecuteCommandLists(&[Some(list)]);
            self.swapchain
                .Present(1, DXGI_PRESENT(0))
                .ok()
                .map_err(|error| format!("D3D12: presenting frame failed: {error}"))?;
        }
        self.wait_gpu()?;
        drop(uploads);
        self.free_textures(&textures_delta.free);
        Ok(())
    }

    fn recreate_swapchain(&mut self) -> Result<(), String> {
        self.wait_gpu()?;
        self.targets.clear();
        unsafe {
            self.swapchain.ResizeBuffers(
                FRAME_COUNT,
                self.size.width,
                self.size.height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )
        }
        .map_err(|error| format!("D3D12: resizing swapchain failed: {error}"))?;
        self.targets =
            create_targets(&self.device, &self.swapchain, &self.rtv_heap, self.rtv_step)?;
        Ok(())
    }

    fn update_texture(&mut self, id: TextureId, delta: &ImageDelta) -> Result<(), String> {
        let ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        if delta.pos.is_none() {
            if let Some(old) = self.textures.remove(&id) {
                self.free_slots.push(old.slot);
            }
            let slot = self.free_slots.pop().unwrap_or_else(|| {
                let slot = self.next_slot;
                self.next_slot += 1;
                slot
            });
            if slot >= MAX_TEXTURES {
                return Err(format!(
                    "D3D12: egui texture limit of {MAX_TEXTURES} exceeded"
                ));
            }
            let resource = texture_resource(&self.device, width as u32, height as u32)?;
            let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            unsafe {
                self.device.CreateShaderResourceView(
                    &resource,
                    Some(&srv),
                    cpu_handle(&self.srv_heap, self.srv_step, slot),
                );
                self.device.CreateSampler(
                    &sampler_description(delta.options),
                    cpu_handle(&self.sampler_heap, self.sampler_step, slot),
                );
            }
            self.textures.insert(
                id,
                TextureEntry {
                    resource,
                    slot,
                    width: width as u32,
                    height: height as u32,
                },
            );
        }
        let [x, y] = delta.pos.unwrap_or([0, 0]);
        let entry = self
            .textures
            .get(&id)
            .ok_or_else(|| format!("D3D12: partial update for missing egui texture {id:?}"))?;
        if x + width > entry.width as usize || y + height > entry.height as usize {
            return Err(format!(
                "D3D12: egui texture update for {id:?} is out of bounds"
            ));
        }
        let pixels: &[u8] = bytemuck::cast_slice(image.pixels.as_slice());
        let resource = entry.resource.clone();
        self.upload_texture(
            &resource,
            delta.pos.is_none(),
            [x as u32, y as u32],
            [width as u32, height as u32],
            pixels,
        )
    }

    fn upload_texture(
        &mut self,
        texture: &ID3D12Resource,
        is_full: bool,
        offset: [u32; 2],
        size: [u32; 2],
        pixels: &[u8],
    ) -> Result<(), String> {
        let row_pitch = (size[0] * 4).div_ceil(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT)
            * D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;
        let upload_size = row_pitch as usize * size[1] as usize;
        let upload = committed_buffer(&self.device, upload_size as u64)?;
        unsafe {
            let mut mapped = ptr::null_mut();
            upload
                .Map(0, None, Some(&mut mapped))
                .map_err(|error| format!("D3D12: mapping texture upload failed: {error}"))?;
            for row in 0..size[1] as usize {
                ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(row * size[0] as usize * 4),
                    mapped.cast::<u8>().add(row * row_pitch as usize),
                    size[0] as usize * 4,
                );
            }
            upload.Unmap(0, None);
            self.allocator
                .Reset()
                .map_err(|error| format!("D3D12: upload allocator reset failed: {error}"))?;
            self.commands
                .Reset(&self.allocator, None)
                .map_err(|error| format!("D3D12: upload command reset failed: {error}"))?;
        }
        if !is_full {
            transition(
                &self.commands,
                texture,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_COPY_DEST,
            );
        }
        let source = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::ManuallyDrop::new(Some(upload.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                        Width: size[0],
                        Height: size[1],
                        Depth: 1,
                        RowPitch: row_pitch,
                    },
                },
            },
        };
        let destination = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::ManuallyDrop::new(Some(texture.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        unsafe {
            self.commands
                .CopyTextureRegion(&destination, offset[0], offset[1], 0, &source, None);
        }
        transition(
            &self.commands,
            texture,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        );
        unsafe {
            self.commands
                .Close()
                .map_err(|error| format!("D3D12: closing texture upload failed: {error}"))?;
            let list: ID3D12CommandList = self
                .commands
                .cast()
                .map_err(|error| format!("D3D12: upload command interface failed: {error}"))?;
            self.queue.ExecuteCommandLists(&[Some(list)]);
        }
        self.wait_gpu()?;
        drop(upload);
        Ok(())
    }

    fn wait_gpu(&mut self) -> Result<(), String> {
        self.fence_value = self.fence_value.wrapping_add(1);
        unsafe { self.queue.Signal(&self.fence, self.fence_value) }
            .map_err(|error| format!("D3D12: signaling fence failed: {error}"))?;
        while unsafe { self.fence.GetCompletedValue() } < self.fence_value {
            std::thread::yield_now();
        }
        Ok(())
    }

    fn free_textures(&mut self, ids: &[TextureId]) {
        for id in ids {
            if let Some(texture) = self.textures.remove(id) {
                self.free_slots.push(texture.slot);
            }
        }
    }
}

fn hwnd(window: &Window) -> Result<HWND, String> {
    let handle = window
        .window_handle()
        .map_err(|error| format!("D3D12: cannot obtain Win32 window handle: {error}"))?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("D3D12: winit did not provide a Win32 window handle".to_owned());
    };
    Ok(HWND(handle.hwnd.get() as *mut c_void))
}

fn descriptor_heap(
    device: &ID3D12Device,
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
    count: u32,
    shader_visible: bool,
) -> Result<ID3D12DescriptorHeap, String> {
    unsafe {
        device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: kind,
            NumDescriptors: count,
            Flags: if shader_visible {
                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
            } else {
                D3D12_DESCRIPTOR_HEAP_FLAG_NONE
            },
            NodeMask: 0,
        })
    }
    .map_err(|error| format!("D3D12: descriptor heap creation failed: {error}"))
}

fn create_targets(
    device: &ID3D12Device,
    swapchain: &IDXGISwapChain3,
    heap: &ID3D12DescriptorHeap,
    step: u32,
) -> Result<Vec<ID3D12Resource>, String> {
    (0..FRAME_COUNT)
        .map(|index| {
            let target: ID3D12Resource = unsafe { swapchain.GetBuffer(index) }
                .map_err(|error| format!("D3D12: cannot obtain back buffer {index}: {error}"))?;
            let description = D3D12_RENDER_TARGET_VIEW_DESC {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
                ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                ..Default::default()
            };
            unsafe {
                device.CreateRenderTargetView(
                    &target,
                    Some(&description),
                    cpu_handle(heap, step, index),
                );
            }
            Ok(target)
        })
        .collect()
}

fn compile_shader(entry: &[u8], target: &[u8]) -> Result<ID3DBlob, String> {
    let mut shader = None;
    let mut errors = None;
    unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            PCSTR(c"editur-egui.hlsl".as_ptr().cast()),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            0,
            0,
            &mut shader,
            Some(&mut errors),
        )
    }
    .map_err(|error| {
        format!(
            "D3D12: shader compilation failed: {}",
            blob_error(errors, error)
        )
    })?;
    shader.ok_or_else(|| "D3D12: shader compiler returned no data".to_owned())
}

fn blob_error(blob: Option<ID3DBlob>, fallback: windows::core::Error) -> String {
    blob.map_or_else(
        || fallback.to_string(),
        |blob| unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                blob.GetBufferPointer().cast::<u8>(),
                blob.GetBufferSize(),
            ))
            .trim()
            .to_owned()
        },
    )
}

fn bytecode(blob: &ID3DBlob) -> D3D12_SHADER_BYTECODE {
    D3D12_SHADER_BYTECODE {
        pShaderBytecode: unsafe { blob.GetBufferPointer() },
        BytecodeLength: unsafe { blob.GetBufferSize() },
    }
}

fn upload_resource(device: &ID3D12Device, bytes: &[u8]) -> Result<ID3D12Resource, String> {
    let resource = committed_buffer(device, bytes.len() as u64)?;
    unsafe {
        let mut mapped = ptr::null_mut();
        resource
            .Map(0, None, Some(&mut mapped))
            .map_err(|error| format!("D3D12: mapping upload buffer failed: {error}"))?;
        ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast(), bytes.len());
        resource.Unmap(0, None);
    }
    Ok(resource)
}

fn committed_buffer(device: &ID3D12Device, size: u64) -> Result<ID3D12Resource, String> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        ..Default::default()
    };
    let description = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size.max(1),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut resource = None;
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &description,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut resource,
        )
    }
    .map_err(|error| format!("D3D12: upload resource creation failed: {error}"))?;
    resource.ok_or_else(|| "D3D12: resource creation returned no object".to_owned())
}

fn texture_resource(
    device: &ID3D12Device,
    width: u32,
    height: u32,
) -> Result<ID3D12Resource, String> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let description = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: width as u64,
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        ..Default::default()
    };
    let mut resource = None;
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &description,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut resource,
        )
    }
    .map_err(|error| format!("D3D12: texture creation failed: {error}"))?;
    resource.ok_or_else(|| "D3D12: texture creation returned no object".to_owned())
}

fn sampler_description(options: TextureOptions) -> D3D12_SAMPLER_DESC {
    let nearest = |filter| filter == TextureFilter::Nearest;
    let filter = match (
        nearest(options.minification),
        nearest(options.magnification),
    ) {
        (true, true) => D3D12_FILTER_MIN_MAG_MIP_POINT,
        (true, false) => D3D12_FILTER_MIN_POINT_MAG_LINEAR_MIP_POINT,
        (false, true) => D3D12_FILTER_MIN_LINEAR_MAG_MIP_POINT,
        (false, false) => D3D12_FILTER_MIN_MAG_LINEAR_MIP_POINT,
    };
    let address = match options.wrap_mode {
        TextureWrapMode::ClampToEdge => D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        TextureWrapMode::Repeat => D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        TextureWrapMode::MirroredRepeat => D3D12_TEXTURE_ADDRESS_MODE_MIRROR,
    };
    D3D12_SAMPLER_DESC {
        Filter: filter,
        AddressU: address,
        AddressV: address,
        AddressW: address,
        MinLOD: 0.0,
        MaxLOD: 0.0,
        ..Default::default()
    }
}

fn transition(
    commands: &ID3D12GraphicsCommandList,
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) {
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
        ..Default::default()
    };
    unsafe { commands.ResourceBarrier(&[barrier]) };
}

fn cpu_handle(heap: &ID3D12DescriptorHeap, step: u32, index: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
    let mut handle = unsafe { heap.GetCPUDescriptorHandleForHeapStart() };
    handle.ptr += step as usize * index as usize;
    handle
}

fn gpu_handle(heap: &ID3D12DescriptorHeap, step: u32, index: u32) -> D3D12_GPU_DESCRIPTOR_HANDLE {
    let mut handle = unsafe { heap.GetGPUDescriptorHandleForHeapStart() };
    handle.ptr += step as u64 * index as u64;
    handle
}

fn scissor_rect(
    rect: egui::Rect,
    pixels_per_point: f32,
    size: PhysicalSize<u32>,
) -> Option<windows::Win32::Foundation::RECT> {
    let left = (rect.min.x * pixels_per_point)
        .round()
        .clamp(0.0, size.width as f32) as i32;
    let top = (rect.min.y * pixels_per_point)
        .round()
        .clamp(0.0, size.height as f32) as i32;
    let right = (rect.max.x * pixels_per_point)
        .round()
        .clamp(left as f32, size.width as f32) as i32;
    let bottom = (rect.max.y * pixels_per_point)
        .round()
        .clamp(top as f32, size.height as f32) as i32;
    (right > left && bottom > top).then_some(windows::Win32::Foundation::RECT {
        left,
        top,
        right,
        bottom,
    })
}
