use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::{CStr, CString, c_void},
    io::Cursor,
    mem::size_of,
    ptr,
};

use ash::{Entry, Instance, vk};
use egui::{
    ClippedPrimitive, ImageData, TextureFilter, TextureId, TextureOptions, TextureWrapMode,
    TexturesDelta,
    epaint::{ImageDelta, Primitive},
};
use winit::{
    dpi::PhysicalSize,
    raw_window_handle::{HasDisplayHandle, HasWindowHandle},
    window::Window,
};

use super::{buffer_capacity, choose_adapter};

const VERTEX_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/egui_vertex.spv"));
const FRAGMENT_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/egui_fragment.spv"));
const FRAMES_IN_FLIGHT: usize = 2;

struct TextureEntry {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    sampler: vk::Sampler,
    descriptor_set: vk::DescriptorSet,
    width: u32,
    height: u32,
}

struct Swapchain {
    handle: vk::SwapchainKHR,
    extent: vk::Extent2D,
    views: Vec<vk::ImageView>,
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    framebuffers: Vec<vk::Framebuffer>,
}

struct Frame {
    command: vk::CommandBuffer,
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    fence: vk::Fence,
    vertex: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    vertex_capacity: usize,
    index: vk::Buffer,
    index_memory: vk::DeviceMemory,
    index_capacity: usize,
    retained: HashMap<u64, super::RetainedUpload>,
}

pub struct Renderer {
    _entry: Entry,
    instance: Instance,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    surface_loader: ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
    physical_device: vk::PhysicalDevice,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    device: ash::Device,
    queue: vk::Queue,
    swapchain_loader: ash::khr::swapchain::Device,
    swapchain: Swapchain,
    command_pool: vk::CommandPool,
    descriptor_pool: vk::DescriptorPool,
    descriptor_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    uniform_buffer: vk::Buffer,
    uniform_memory: vk::DeviceMemory,
    screen: [f32; 2],
    frames: Vec<Frame>,
    next_frame: usize,
    textures: HashMap<TextureId, TextureEntry>,
    size: PhysicalSize<u32>,
    adapter_name: String,
}

impl Renderer {
    pub fn new(window: &Window) -> Result<Self, String> {
        let entry = unsafe { Entry::load() }
            .map_err(|error| format!("Vulkan: cannot load the Vulkan loader: {error}"))?;
        let display = window
            .display_handle()
            .map_err(|error| format!("Vulkan: cannot obtain display handle: {error}"))?;
        let window_handle = window
            .window_handle()
            .map_err(|error| format!("Vulkan: cannot obtain window handle: {error}"))?;
        let mut extensions = ash_window::enumerate_required_extensions(display.as_raw())
            .map_err(|error| format!("Vulkan: display surface is unsupported: {error}"))?
            .to_vec();
        let validation = env::var("EDITUR_GPU_VALIDATION").as_deref() == Ok("1");
        let validation_name = CString::new("VK_LAYER_KHRONOS_validation")
            .map_err(|error| format!("Vulkan: invalid validation-layer name: {error}"))?;
        let validation_available = validation
            && unsafe { entry.enumerate_instance_layer_properties() }
                .map_err(|error| format!("Vulkan: cannot enumerate instance layers: {error}"))?
                .iter()
                .any(|layer| unsafe { CStr::from_ptr(layer.layer_name.as_ptr()) } == validation_name.as_c_str());
        if validation_available {
            extensions.push(ash::ext::debug_utils::NAME.as_ptr());
        }
        let layers = validation_available
            .then_some(vec![validation_name.as_ptr()])
            .unwrap_or_default();
        let application_name = CString::new("Editur")
            .map_err(|error| format!("Vulkan: invalid application name: {error}"))?;
        let application = vk::ApplicationInfo::default()
            .application_name(&application_name)
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(&application_name)
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::API_VERSION_1_1);
        let instance_info = vk::InstanceCreateInfo::default()
            .application_info(&application)
            .enabled_extension_names(&extensions)
            .enabled_layer_names(&layers);
        let instance = unsafe { entry.create_instance(&instance_info, None) }
            .map_err(|error| format!("Vulkan: instance creation failed: {error}"))?;
        let debug = if validation_available {
            let loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                        | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(debug_callback));
            let messenger =
                unsafe { loader.create_debug_utils_messenger(&info, None) }.map_err(|error| {
                    format!("Vulkan: validation messenger creation failed: {error}")
                })?;
            Some((loader, messenger))
        } else {
            None
        };
        let surface = unsafe {
            ash_window::create_surface(
                &entry,
                &instance,
                display.as_raw(),
                window_handle.as_raw(),
                None,
            )
        }
        .map_err(|error| format!("Vulkan: surface creation failed: {error}"))?;
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);

        let requested = env::var("EDITUR_GPU_DEVICE").ok();
        let candidates = unsafe { instance.enumerate_physical_devices() }
            .map_err(|error| format!("Vulkan: cannot enumerate devices: {error}"))?;
        let mut usable = Vec::new();
        for physical_device in candidates {
            let properties = unsafe { instance.get_physical_device_properties(physical_device) };
            let name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            let queue_family = unsafe {
                instance
                    .get_physical_device_queue_family_properties(physical_device)
                    .iter()
                    .enumerate()
                    .find_map(|(index, properties)| {
                        let graphics = properties.queue_flags.contains(vk::QueueFlags::GRAPHICS);
                        let present = surface_loader
                            .get_physical_device_surface_support(
                                physical_device,
                                index as u32,
                                surface,
                            )
                            .unwrap_or(false);
                        (graphics && present).then_some(index as u32)
                    })
            };
            if let Some(queue_family) = queue_family {
                usable.push((
                    physical_device,
                    queue_family,
                    name,
                    properties.device_type == vk::PhysicalDeviceType::INTEGRATED_GPU,
                ));
            }
        }
        let choices: Vec<_> = usable
            .iter()
            .map(|(_, _, name, integrated)| (name.as_str(), *integrated, false))
            .collect();
        let index = choose_adapter(&choices, requested.as_deref()).ok_or_else(|| {
            requested.map_or_else(
                || "Vulkan: no device has a graphics queue with present support".to_owned(),
                |name| format!("Vulkan: no usable device matches EDITUR_GPU_DEVICE={name:?}"),
            )
        })?;
        let (physical_device, queue_family, adapter_name, _) = usable.swap_remove(index);
        let priorities = [1.0];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let device_extensions = [ash::khr::swapchain::NAME.as_ptr()];
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&device_extensions);
        let device = unsafe { instance.create_device(physical_device, &device_info, None) }
            .map_err(|error| format!("Vulkan: logical device creation failed: {error}"))?;
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let command_pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family)
            .flags(
                vk::CommandPoolCreateFlags::TRANSIENT
                    | vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER,
            );
        let command_pool = unsafe { device.create_command_pool(&command_pool_info, None) }
            .map_err(|error| format!("Vulkan: command pool creation failed: {error}"))?;

        let descriptor_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX),
        ];
        let descriptor_layout_info =
            vk::DescriptorSetLayoutCreateInfo::default().bindings(&descriptor_bindings);
        let descriptor_layout =
            unsafe { device.create_descriptor_set_layout(&descriptor_layout_info, None) }
                .map_err(|error| format!("Vulkan: descriptor layout creation failed: {error}"))?;
        let layouts = [descriptor_layout];
        let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&pipeline_layout_info, None) }
            .map_err(|error| format!("Vulkan: pipeline layout creation failed: {error}"))?;
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1024),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLER)
                .descriptor_count(1024),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1024),
        ];
        let descriptor_pool_info = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
            .max_sets(1024)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&descriptor_pool_info, None) }
            .map_err(|error| format!("Vulkan: descriptor pool creation failed: {error}"))?;
        let (uniform_buffer, uniform_memory) = create_buffer(
            &device,
            &memory_properties,
            8,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let command_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT as u32);
        let commands = unsafe { device.allocate_command_buffers(&command_info) }
            .map_err(|error| format!("Vulkan: frame command allocation failed: {error}"))?;
        let semaphore_info = vk::SemaphoreCreateInfo::default();
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let frames = commands
            .into_iter()
            .map(|command| {
                Ok(Frame {
                    command,
                    image_available: unsafe { device.create_semaphore(&semaphore_info, None) }
                        .map_err(|error| format!("Vulkan: semaphore creation failed: {error}"))?,
                    render_finished: unsafe { device.create_semaphore(&semaphore_info, None) }
                        .map_err(|error| format!("Vulkan: semaphore creation failed: {error}"))?,
                    fence: unsafe { device.create_fence(&fence_info, None) }
                        .map_err(|error| format!("Vulkan: fence creation failed: {error}"))?,
                    vertex: vk::Buffer::null(),
                    vertex_memory: vk::DeviceMemory::null(),
                    vertex_capacity: 0,
                    index: vk::Buffer::null(),
                    index_memory: vk::DeviceMemory::null(),
                    index_capacity: 0,
                    retained: HashMap::new(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let size = window.inner_size();
        let swapchain = create_swapchain(
            &device,
            physical_device,
            &surface_loader,
            surface,
            &swapchain_loader,
            pipeline_layout,
            size,
            vk::SwapchainKHR::null(),
        )?;

        Ok(Self {
            _entry: entry,
            instance,
            debug,
            surface_loader,
            surface,
            physical_device,
            memory_properties,
            device,
            queue,
            swapchain_loader,
            swapchain,
            command_pool,
            descriptor_pool,
            descriptor_layout,
            pipeline_layout,
            uniform_buffer,
            uniform_memory,
            screen: [0.0; 2],
            frames,
            next_frame: 0,
            textures: HashMap::new(),
            size,
            adapter_name,
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub const fn backend_name(&self) -> &'static str {
        "Vulkan"
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
        for frame in &mut self.frames {
            super::invalidate_retained_uploads_on_texture_replace(
                &mut frame.retained,
                textures_delta,
            );
        }
        for (id, delta) in &textures_delta.set {
            self.update_texture(*id, delta)?;
        }
        if self.size.width == 0 || self.size.height == 0 {
            self.free_textures(&textures_delta.free);
            return Ok(());
        }
        if self.size.width != self.swapchain.extent.width
            || self.size.height != self.swapchain.extent.height
        {
            self.recreate_swapchain()?;
        }
        let screen = [
            self.swapchain.extent.width as f32 / pixels_per_point,
            self.swapchain.extent.height as f32 / pixels_per_point,
        ];
        if screen != self.screen {
            unsafe {
                self.device
                    .device_wait_idle()
                    .map_err(|error| format!("Vulkan: waiting to resize screen failed: {error}"))?;
                let mapped = self
                    .device
                    .map_memory(self.uniform_memory, 0, 8, vk::MemoryMapFlags::empty())
                    .map_err(|error| {
                        format!("Vulkan: mapping screen-size buffer failed: {error}")
                    })?;
                ptr::copy_nonoverlapping(screen.as_ptr().cast::<u8>(), mapped.cast(), 8);
                self.device.unmap_memory(self.uniform_memory);
            }
            self.screen = screen;
        }
        let frame_index = self.next_frame;
        let frame_fence = self.frames[frame_index].fence;
        let image_available = self.frames[frame_index].image_available;
        unsafe {
            self.device
                .wait_for_fences(&[frame_fence], true, u64::MAX)
                .map_err(|error| format!("Vulkan: waiting for frame failed: {error}"))?;
        }
        let image_index = match unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain.handle,
                u64::MAX,
                image_available,
                vk::Fence::null(),
            )
        } {
            Ok((index, _)) => index,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate_swapchain()?;
                return Ok(());
            }
            Err(error) => return Err(format!("Vulkan: acquiring swapchain image failed: {error}")),
        };
        let frame = &mut self.frames[frame_index];
        let vertex_size = primitives
            .iter()
            .filter_map(|primitive| match &primitive.primitive {
                Primitive::Mesh(mesh) => {
                    Some(mesh.vertices.len() * size_of::<egui::epaint::Vertex>())
                }
                Primitive::Callback(_) => None,
            })
            .sum();
        let index_size = primitives
            .iter()
            .filter_map(|primitive| match &primitive.primitive {
                Primitive::Mesh(mesh) => Some(mesh.indices.len() * size_of::<u32>()),
                Primitive::Callback(_) => None,
            })
            .sum();
        let vertex_capacity = frame.vertex_capacity;
        ensure_frame_buffer(
            &self.device,
            &self.memory_properties,
            &mut frame.vertex,
            &mut frame.vertex_memory,
            &mut frame.vertex_capacity,
            vertex_size,
            vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        let index_capacity = frame.index_capacity;
        ensure_frame_buffer(
            &self.device,
            &self.memory_properties,
            &mut frame.index,
            &mut frame.index_memory,
            &mut frame.index_capacity,
            index_size,
            vk::BufferUsageFlags::INDEX_BUFFER,
        )?;
        if vertex_capacity != frame.vertex_capacity || index_capacity != frame.index_capacity {
            frame.retained.clear();
        }
        copy_meshes(
            &self.device,
            primitives,
            frame.vertex_memory,
            frame.index_memory,
            &mut frame.retained,
        )?;
        unsafe {
            self.device
                .reset_fences(&[frame.fence])
                .map_err(|error| format!("Vulkan: resetting frame fence failed: {error}"))?;
            self.device
                .reset_command_buffer(frame.command, vk::CommandBufferResetFlags::empty())
                .map_err(|error| format!("Vulkan: resetting frame commands failed: {error}"))?;
            let begin = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device
                .begin_command_buffer(frame.command, &begin)
                .map_err(|error| format!("Vulkan: beginning frame commands failed: {error}"))?;
        }
        let command = frame.command;
        let clear = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.078, 0.078, 0.086, 1.0],
            },
        }];
        let pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.swapchain.render_pass)
            .framebuffer(self.swapchain.framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: self.swapchain.extent,
            })
            .clear_values(&clear);
        unsafe {
            self.device
                .cmd_begin_render_pass(command, &pass_info, vk::SubpassContents::INLINE);
            self.device.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                self.swapchain.pipeline,
            );
            self.device.cmd_set_viewport(
                command,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: self.swapchain.extent.width as f32,
                    height: self.swapchain.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
        }
        let mut vertex_offset = 0;
        let mut index_offset = 0;
        for primitive in primitives {
            let mesh = match &primitive.primitive {
                Primitive::Mesh(mesh) => mesh,
                Primitive::Callback(_) => {
                    super::retained_paint(&primitive.primitive)?;
                    continue;
                }
            };
            if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                continue;
            }
            let vertex_bytes: &[u8] = bytemuck::cast_slice(mesh.vertices.as_slice());
            let index_bytes: &[u8] = bytemuck::cast_slice(mesh.indices.as_slice());
            let Some(scissor) =
                scissor_rect(primitive.clip_rect, pixels_per_point, self.swapchain.extent)
            else {
                vertex_offset += vertex_bytes.len();
                index_offset += index_bytes.len();
                continue;
            };
            let texture = self
                .textures
                .get(&mesh.texture_id)
                .ok_or_else(|| format!("Vulkan: missing egui texture {:?}", mesh.texture_id))?;
            unsafe {
                self.device.cmd_set_scissor(command, 0, &[scissor]);
                self.device.cmd_bind_vertex_buffers(
                    command,
                    0,
                    &[frame.vertex],
                    &[vertex_offset as u64],
                );
                self.device.cmd_bind_index_buffer(
                    command,
                    frame.index,
                    index_offset as u64,
                    vk::IndexType::UINT32,
                );
                self.device.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[texture.descriptor_set],
                    &[],
                );
                self.device
                    .cmd_draw_indexed(command, mesh.indices.len() as u32, 1, 0, 0, 0);
            }
            vertex_offset += vertex_bytes.len();
            index_offset += index_bytes.len();
        }
        unsafe {
            self.device.cmd_end_render_pass(command);
            self.device
                .end_command_buffer(command)
                .map_err(|error| format!("Vulkan: ending command buffer failed: {error}"))?;
            let wait_semaphores = [frame.image_available];
            let signal_semaphores = [frame.render_finished];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let commands = [command];
            let submit = [vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&commands)
                .signal_semaphores(&signal_semaphores)];
            self.device
                .queue_submit(self.queue, &submit, frame.fence)
                .map_err(|error| format!("Vulkan: queue submission failed: {error}"))?;
            let swapchains = [self.swapchain.handle];
            let image_indices = [image_index];
            let present = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            let recreate = match self.swapchain_loader.queue_present(self.queue, &present) {
                Ok(suboptimal) => suboptimal,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => true,
                Err(error) => return Err(format!("Vulkan: presenting frame failed: {error}")),
            };
            if recreate {
                self.recreate_swapchain()?;
            }
        }
        self.next_frame = (frame_index + 1) % self.frames.len();
        if !textures_delta.free.is_empty() {
            unsafe {
                self.device
                    .device_wait_idle()
                    .map_err(|error| format!("Vulkan: waiting to free textures failed: {error}"))?;
            }
        }
        self.free_textures(&textures_delta.free);
        Ok(())
    }

    fn recreate_swapchain(&mut self) -> Result<(), String> {
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }
        unsafe {
            self.device.device_wait_idle().map_err(|error| {
                format!("Vulkan: waiting before swapchain recreation failed: {error}")
            })?;
        }
        let old_handle = self.swapchain.handle;
        let replacement = create_swapchain(
            &self.device,
            self.physical_device,
            &self.surface_loader,
            self.surface,
            &self.swapchain_loader,
            self.pipeline_layout,
            self.size,
            old_handle,
        )?;
        let old = std::mem::replace(&mut self.swapchain, replacement);
        unsafe { destroy_swapchain(&self.device, &self.swapchain_loader, old) };
        Ok(())
    }

    fn update_texture(&mut self, id: TextureId, delta: &ImageDelta) -> Result<(), String> {
        let ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        if delta.pos.is_none() {
            if self.textures.contains_key(&id) {
                unsafe {
                    self.device.device_wait_idle().map_err(|error| {
                        format!("Vulkan: waiting to replace texture failed: {error}")
                    })?;
                }
            }
            if let Some(old) = self.textures.remove(&id) {
                self.destroy_texture(old);
            }
            let entry = self.create_texture(width as u32, height as u32, delta.options)?;
            self.textures.insert(id, entry);
        }
        let [x, y] = delta.pos.unwrap_or([0, 0]);
        let entry = self
            .textures
            .get(&id)
            .ok_or_else(|| format!("Vulkan: partial update for missing egui texture {id:?}"))?;
        if x + width > entry.width as usize || y + height > entry.height as usize {
            return Err(format!(
                "Vulkan: egui texture update for {id:?} is out of bounds"
            ));
        }
        let bytes: &[u8] = bytemuck::cast_slice(image.pixels.as_slice());
        self.upload_texture(
            entry.image,
            delta.pos.is_none(),
            [x as u32, y as u32],
            [width as u32, height as u32],
            bytes,
        )
    }

    fn create_texture(
        &self,
        width: u32,
        height: u32,
        options: TextureOptions,
    ) -> Result<TextureEntry, String> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_SRGB)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { self.device.create_image(&image_info, None) }
            .map_err(|error| format!("Vulkan: texture creation failed: {error}"))?;
        let requirements = unsafe { self.device.get_image_memory_requirements(image) };
        let memory_type = memory_type(
            &self.memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let allocation = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type);
        let memory = unsafe { self.device.allocate_memory(&allocation, None) }
            .map_err(|error| format!("Vulkan: texture memory allocation failed: {error}"))?;
        unsafe { self.device.bind_image_memory(image, memory, 0) }
            .map_err(|error| format!("Vulkan: binding texture memory failed: {error}"))?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_SRGB)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { self.device.create_image_view(&view_info, None) }
            .map_err(|error| format!("Vulkan: texture view creation failed: {error}"))?;
        let filter = |filter| match filter {
            TextureFilter::Nearest => vk::Filter::NEAREST,
            TextureFilter::Linear => vk::Filter::LINEAR,
        };
        let address = match options.wrap_mode {
            TextureWrapMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            TextureWrapMode::Repeat => vk::SamplerAddressMode::REPEAT,
            TextureWrapMode::MirroredRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
        };
        let sampler_info = vk::SamplerCreateInfo::default()
            .mag_filter(filter(options.magnification))
            .min_filter(filter(options.minification))
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .max_lod(0.0);
        let sampler = unsafe { self.device.create_sampler(&sampler_info, None) }
            .map_err(|error| format!("Vulkan: texture sampler creation failed: {error}"))?;
        let layouts = [self.descriptor_layout];
        let allocation = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.descriptor_pool)
            .set_layouts(&layouts);
        let descriptor_set = unsafe { self.device.allocate_descriptor_sets(&allocation) }
            .map_err(|error| format!("Vulkan: texture descriptor allocation failed: {error}"))?[0];
        let image_info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let buffer_info = [vk::DescriptorBufferInfo::default()
            .buffer(self.uniform_buffer)
            .offset(0)
            .range(8)];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(descriptor_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&image_info),
            vk::WriteDescriptorSet::default()
                .dst_set(descriptor_set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(&sampler_info),
            vk::WriteDescriptorSet::default()
                .dst_set(descriptor_set)
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info),
        ];
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };
        Ok(TextureEntry {
            image,
            memory,
            view,
            sampler,
            descriptor_set,
            width,
            height,
        })
    }

    fn upload_texture(
        &self,
        image: vk::Image,
        is_full: bool,
        offset: [u32; 2],
        size: [u32; 2],
        bytes: &[u8],
    ) -> Result<(), String> {
        let staging = self.upload_buffer(bytes, vk::BufferUsageFlags::TRANSFER_SRC)?;
        let command = self.begin_commands()?;
        let old_layout = if is_full {
            vk::ImageLayout::UNDEFINED
        } else {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        };
        transition_image(
            &self.device,
            command,
            image,
            old_layout,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let copy = vk::BufferImageCopy::default()
            .image_offset(vk::Offset3D {
                x: offset[0] as i32,
                y: offset[1] as i32,
                z: 0,
            })
            .image_extent(vk::Extent3D {
                width: size[0],
                height: size[1],
                depth: 1,
            })
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            });
        unsafe {
            self.device.cmd_copy_buffer_to_image(
                command,
                staging.0,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            );
        }
        transition_image(
            &self.device,
            command,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        self.submit_and_wait(command)?;
        unsafe {
            self.device.destroy_buffer(staging.0, None);
            self.device.free_memory(staging.1, None);
        }
        Ok(())
    }

    fn upload_buffer(
        &self,
        bytes: &[u8],
        usage: vk::BufferUsageFlags,
    ) -> Result<(vk::Buffer, vk::DeviceMemory), String> {
        let result = create_buffer(
            &self.device,
            &self.memory_properties,
            bytes.len() as u64,
            usage,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        unsafe {
            let mapped = self
                .device
                .map_memory(result.1, 0, bytes.len() as u64, vk::MemoryMapFlags::empty())
                .map_err(|error| format!("Vulkan: mapping upload buffer failed: {error}"))?;
            ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast(), bytes.len());
            self.device.unmap_memory(result.1);
        }
        Ok(result)
    }

    fn begin_commands(&self) -> Result<vk::CommandBuffer, String> {
        let allocation = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command = unsafe { self.device.allocate_command_buffers(&allocation) }
            .map_err(|error| format!("Vulkan: command buffer allocation failed: {error}"))?[0];
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { self.device.begin_command_buffer(command, &begin) }
            .map_err(|error| format!("Vulkan: beginning command buffer failed: {error}"))?;
        Ok(command)
    }

    fn submit_and_wait(&self, command: vk::CommandBuffer) -> Result<(), String> {
        unsafe {
            self.device
                .end_command_buffer(command)
                .map_err(|error| format!("Vulkan: ending upload commands failed: {error}"))?;
            let commands = [command];
            let submits = [vk::SubmitInfo::default().command_buffers(&commands)];
            self.device
                .queue_submit(self.queue, &submits, vk::Fence::null())
                .and_then(|()| self.device.queue_wait_idle(self.queue))
                .map_err(|error| format!("Vulkan: texture upload failed: {error}"))?;
            self.device
                .free_command_buffers(self.command_pool, &[command]);
        }
        Ok(())
    }

    fn free_textures(&mut self, ids: &[TextureId]) {
        for id in ids {
            if let Some(texture) = self.textures.remove(id) {
                self.destroy_texture(texture);
            }
        }
    }

    fn destroy_texture(&self, texture: TextureEntry) {
        unsafe {
            let _ = self
                .device
                .free_descriptor_sets(self.descriptor_pool, &[texture.descriptor_set]);
            self.device.destroy_sampler(texture.sampler, None);
            self.device.destroy_image_view(texture.view, None);
            self.device.destroy_image(texture.image, None);
            self.device.free_memory(texture.memory, None);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
        }
        for (_, texture) in std::mem::take(&mut self.textures) {
            self.destroy_texture(texture);
        }
        unsafe {
            for framebuffer in self.swapchain.framebuffers.drain(..) {
                self.device.destroy_framebuffer(framebuffer, None);
            }
            self.device.destroy_pipeline(self.swapchain.pipeline, None);
            self.device
                .destroy_render_pass(self.swapchain.render_pass, None);
            for view in self.swapchain.views.drain(..) {
                self.device.destroy_image_view(view, None);
            }
            self.swapchain_loader
                .destroy_swapchain(self.swapchain.handle, None);
            for frame in self.frames.drain(..) {
                if frame.vertex != vk::Buffer::null() {
                    self.device.destroy_buffer(frame.vertex, None);
                    self.device.free_memory(frame.vertex_memory, None);
                }
                if frame.index != vk::Buffer::null() {
                    self.device.destroy_buffer(frame.index, None);
                    self.device.free_memory(frame.index_memory, None);
                }
                self.device.destroy_fence(frame.fence, None);
                self.device.destroy_semaphore(frame.render_finished, None);
                self.device.destroy_semaphore(frame.image_available, None);
            }
            self.device.destroy_buffer(self.uniform_buffer, None);
            self.device.free_memory(self.uniform_memory, None);
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_layout, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            if let Some((loader, messenger)) = &self.debug {
                loader.destroy_debug_utils_messenger(*messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn create_swapchain(
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    surface_loader: &ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
    loader: &ash::khr::swapchain::Device,
    pipeline_layout: vk::PipelineLayout,
    requested_size: PhysicalSize<u32>,
    old_swapchain: vk::SwapchainKHR,
) -> Result<Swapchain, String> {
    let capabilities = unsafe {
        surface_loader.get_physical_device_surface_capabilities(physical_device, surface)
    }
    .map_err(|error| format!("Vulkan: cannot query surface capabilities: {error}"))?;
    let formats =
        unsafe { surface_loader.get_physical_device_surface_formats(physical_device, surface) }
            .map_err(|error| format!("Vulkan: cannot query surface formats: {error}"))?;
    let present_modes = unsafe {
        surface_loader.get_physical_device_surface_present_modes(physical_device, surface)
    }
    .map_err(|error| format!("Vulkan: cannot query surface present modes: {error}"))?;
    let surface_format = formats
        .iter()
        .copied()
        .find(|format| {
            format.format == vk::Format::B8G8R8A8_SRGB
                && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .or_else(|| formats.first().copied())
        .ok_or_else(|| "Vulkan: surface exposes no image formats".to_owned())?;
    let extent = if capabilities.current_extent.width != u32::MAX {
        capabilities.current_extent
    } else {
        vk::Extent2D {
            width: requested_size.width.clamp(
                capabilities.min_image_extent.width,
                capabilities.max_image_extent.width,
            ),
            height: requested_size.height.clamp(
                capabilities.min_image_extent.height,
                capabilities.max_image_extent.height,
            ),
        }
    };
    let mut image_count = capabilities.min_image_count.saturating_add(1);
    if capabilities.max_image_count > 0 {
        image_count = image_count.min(capabilities.max_image_count);
    }
    let info = vk::SwapchainCreateInfoKHR::default()
        .surface(surface)
        .min_image_count(image_count)
        .image_format(surface_format.format)
        .image_color_space(surface_format.color_space)
        .image_extent(extent)
        .image_array_layers(1)
        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(capabilities.current_transform)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        .present_mode(choose_present_mode(&present_modes))
        .clipped(true)
        .old_swapchain(old_swapchain);
    let handle = unsafe { loader.create_swapchain(&info, None) }
        .map_err(|error| format!("Vulkan: swapchain creation failed: {error}"))?;
    let images = unsafe { loader.get_swapchain_images(handle) }
        .map_err(|error| format!("Vulkan: cannot obtain swapchain images: {error}"))?;
    let views = images
        .iter()
        .map(|image| {
            let info = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(surface_format.format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            unsafe { device.create_image_view(&info, None) }
                .map_err(|error| format!("Vulkan: swapchain image view creation failed: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let attachment = [vk::AttachmentDescription::default()
        .format(surface_format.format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
    let color_reference = [vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    }];
    let subpass = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_reference)];
    let dependency = [vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
    let render_pass_info = vk::RenderPassCreateInfo::default()
        .attachments(&attachment)
        .subpasses(&subpass)
        .dependencies(&dependency);
    let render_pass = unsafe { device.create_render_pass(&render_pass_info, None) }
        .map_err(|error| format!("Vulkan: render pass creation failed: {error}"))?;
    let pipeline = create_pipeline(device, render_pass, pipeline_layout)?;
    let framebuffers = views
        .iter()
        .map(|view| {
            let attachments = [*view];
            let info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            unsafe { device.create_framebuffer(&info, None) }
                .map_err(|error| format!("Vulkan: framebuffer creation failed: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Swapchain {
        handle,
        extent,
        views,
        render_pass,
        pipeline,
        framebuffers,
    })
}

fn choose_present_mode(modes: &[vk::PresentModeKHR]) -> vk::PresentModeKHR {
    [
        vk::PresentModeKHR::MAILBOX,
        vk::PresentModeKHR::IMMEDIATE,
        vk::PresentModeKHR::FIFO,
    ]
    .into_iter()
    .find(|mode| modes.contains(mode))
    .unwrap_or(vk::PresentModeKHR::FIFO)
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, String> {
    let vertex_words = ash::util::read_spv(&mut Cursor::new(VERTEX_SHADER))
        .map_err(|error| format!("Vulkan: invalid embedded vertex shader: {error}"))?;
    let fragment_words = ash::util::read_spv(&mut Cursor::new(FRAGMENT_SHADER))
        .map_err(|error| format!("Vulkan: invalid embedded fragment shader: {error}"))?;
    let vertex_module = unsafe {
        device.create_shader_module(
            &vk::ShaderModuleCreateInfo::default().code(&vertex_words),
            None,
        )
    }
    .map_err(|error| format!("Vulkan: vertex shader creation failed: {error}"))?;
    let fragment_module = unsafe {
        device.create_shader_module(
            &vk::ShaderModuleCreateInfo::default().code(&fragment_words),
            None,
        )
    }
    .map_err(|error| format!("Vulkan: fragment shader creation failed: {error}"))?;
    let vertex_main = CString::new("vertex_main")
        .map_err(|error| format!("Vulkan: invalid vertex entry: {error}"))?;
    let fragment_main = CString::new("fragment_main")
        .map_err(|error| format!("Vulkan: invalid fragment entry: {error}"))?;
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex_module)
            .name(&vertex_main),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment_module)
            .name(&fragment_main),
    ];
    let binding = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: size_of::<egui::epaint::Vertex>() as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];
    let attributes = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 8,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R8G8B8A8_UNORM,
            offset: 16,
        },
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&binding)
        .vertex_attribute_descriptions(&attributes);
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::ONE)
        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .alpha_blend_op(vk::BlendOp::ADD)
        .color_write_mask(vk::ColorComponentFlags::RGBA)];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
    let dynamic = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic);
    let info = [vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport)
        .rasterization_state(&rasterization)
        .multisample_state(&multisample)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic_state)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0)];
    let result =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &info, None) }
            .map_err(|(_, error)| format!("Vulkan: graphics pipeline creation failed: {error}"))?
            [0];
    unsafe {
        device.destroy_shader_module(fragment_module, None);
        device.destroy_shader_module(vertex_module, None);
    }
    Ok(result)
}

unsafe fn destroy_swapchain(
    device: &ash::Device,
    loader: &ash::khr::swapchain::Device,
    swapchain: Swapchain,
) {
    for framebuffer in swapchain.framebuffers {
        unsafe { device.destroy_framebuffer(framebuffer, None) };
    }
    unsafe {
        device.destroy_pipeline(swapchain.pipeline, None);
        device.destroy_render_pass(swapchain.render_pass, None);
    }
    for view in swapchain.views {
        unsafe { device.destroy_image_view(view, None) };
    }
    unsafe { loader.destroy_swapchain(swapchain.handle, None) };
}

fn create_buffer(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    size: u64,
    usage: vk::BufferUsageFlags,
    properties: vk::MemoryPropertyFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory), String> {
    let info = vk::BufferCreateInfo::default()
        .size(size.max(1))
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&info, None) }
        .map_err(|error| format!("Vulkan: buffer creation failed: {error}"))?;
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let memory_type = memory_type(memory_properties, requirements.memory_type_bits, properties)?;
    let allocation = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    let memory = unsafe { device.allocate_memory(&allocation, None) }
        .map_err(|error| format!("Vulkan: buffer memory allocation failed: {error}"))?;
    unsafe { device.bind_buffer_memory(buffer, memory, 0) }
        .map_err(|error| format!("Vulkan: binding buffer memory failed: {error}"))?;
    Ok((buffer, memory))
}

#[allow(clippy::too_many_arguments)]
fn ensure_frame_buffer(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    buffer: &mut vk::Buffer,
    memory: &mut vk::DeviceMemory,
    capacity: &mut usize,
    needed: usize,
    usage: vk::BufferUsageFlags,
) -> Result<(), String> {
    let new_capacity = buffer_capacity(*capacity, needed);
    if new_capacity == *capacity {
        return Ok(());
    }
    let (new_buffer, new_memory) = create_buffer(
        device,
        memory_properties,
        new_capacity as u64,
        usage,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )?;
    if *buffer != vk::Buffer::null() {
        unsafe {
            device.destroy_buffer(*buffer, None);
            device.free_memory(*memory, None);
        }
    }
    (*buffer, *memory) = (new_buffer, new_memory);
    *capacity = new_capacity;
    Ok(())
}

fn copy_meshes(
    device: &ash::Device,
    primitives: &[ClippedPrimitive],
    vertex_memory: vk::DeviceMemory,
    index_memory: vk::DeviceMemory,
    retained_uploads: &mut HashMap<u64, super::RetainedUpload>,
) -> Result<(), String> {
    if vertex_memory == vk::DeviceMemory::null() || index_memory == vk::DeviceMemory::null() {
        return Ok(());
    }
    let mut retained = None;
    let mut vertex_offset = 0;
    let mut index_offset = 0;
    let mut uploads = Vec::new();
    let mut active_retained = HashSet::new();
    for primitive in primitives {
        let mesh = match &primitive.primitive {
            Primitive::Mesh(mesh) => mesh,
            Primitive::Callback(_) => {
                retained = super::retained_paint(&primitive.primitive)?;
                continue;
            }
        };
        let vertex_bytes: &[u8] = bytemuck::cast_slice(mesh.vertices.as_slice());
        let index_bytes: &[u8] = bytemuck::cast_slice(mesh.indices.as_slice());
        let upload = retained.take().map(|paint| {
            active_retained.insert(paint.key);
            (
                paint.key,
                super::RetainedUpload {
                    revision: paint.revision,
                    vertex_offset,
                    vertex_bytes: vertex_bytes.len(),
                    index_offset,
                    index_bytes: index_bytes.len(),
                },
            )
        });
        let dirty = upload
            .as_ref()
            .is_none_or(|(key, next)| super::upload_required(retained_uploads.get(key), *next));
        uploads.push((mesh, upload, dirty, vertex_offset, index_offset));
        vertex_offset += vertex_bytes.len();
        index_offset += index_bytes.len();
    }
    super::retain_active_uploads(retained_uploads, &active_retained);
    if !uploads.iter().any(|(_, _, dirty, _, _)| *dirty) {
        return Ok(());
    }
    unsafe {
        let vertex_mapped = device
            .map_memory(
                vertex_memory,
                0,
                vk::WHOLE_SIZE,
                vk::MemoryMapFlags::empty(),
            )
            .map_err(|error| format!("Vulkan: mapping frame buffer failed: {error}"))?;
        let index_mapped = device
            .map_memory(index_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            .map_err(|error| {
                device.unmap_memory(vertex_memory);
                format!("Vulkan: mapping frame buffer failed: {error}")
            })?;
        for (mesh, upload, dirty, vertex_offset, index_offset) in uploads {
            if !dirty {
                continue;
            }
            let vertices: &[u8] = bytemuck::cast_slice(mesh.vertices.as_slice());
            let indices: &[u8] = bytemuck::cast_slice(mesh.indices.as_slice());
            ptr::copy_nonoverlapping(
                vertices.as_ptr(),
                vertex_mapped.cast::<u8>().add(vertex_offset),
                vertices.len(),
            );
            ptr::copy_nonoverlapping(
                indices.as_ptr(),
                index_mapped.cast::<u8>().add(index_offset),
                indices.len(),
            );
            if let Some((key, upload)) = upload {
                retained_uploads.insert(key, upload);
            }
        }
        device.unmap_memory(vertex_memory);
        device.unmap_memory(index_memory);
    }
    Ok(())
}

fn memory_type(
    properties: &vk::PhysicalDeviceMemoryProperties,
    bits: u32,
    required: vk::MemoryPropertyFlags,
) -> Result<u32, String> {
    (0..properties.memory_type_count)
        .find(|index| {
            bits & (1 << index) != 0
                && properties.memory_types[*index as usize]
                    .property_flags
                    .contains(required)
        })
        .ok_or_else(|| format!("Vulkan: no memory type supports {required:?}"))
}

fn transition_image(
    device: &ash::Device,
    command: vk::CommandBuffer,
    image: vk::Image,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
) {
    let (source_stage, source_access) = if old == vk::ImageLayout::UNDEFINED {
        (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
        )
    } else {
        (
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        )
    };
    let (destination_stage, destination_access) = if new == vk::ImageLayout::TRANSFER_DST_OPTIMAL {
        (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
        )
    } else {
        (
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        )
    };
    let barrier = [vk::ImageMemoryBarrier::default()
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
        .src_access_mask(source_access)
        .dst_access_mask(destination_access)];
    unsafe {
        device.cmd_pipeline_barrier(
            command,
            source_stage,
            destination_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &barrier,
        );
    }
}

fn scissor_rect(
    rect: egui::Rect,
    pixels_per_point: f32,
    extent: vk::Extent2D,
) -> Option<vk::Rect2D> {
    let min_x = (rect.min.x * pixels_per_point)
        .round()
        .clamp(0.0, extent.width as f32) as i32;
    let min_y = (rect.min.y * pixels_per_point)
        .round()
        .clamp(0.0, extent.height as f32) as i32;
    let max_x = (rect.max.x * pixels_per_point)
        .round()
        .clamp(min_x as f32, extent.width as f32) as i32;
    let max_y = (rect.max.y * pixels_per_point)
        .round()
        .clamp(min_y as f32, extent.height as f32) as i32;
    (max_x > min_x && max_y > min_y).then_some(vk::Rect2D {
        offset: vk::Offset2D { x: min_x, y: min_y },
        extent: vk::Extent2D {
            width: (max_x - min_x) as u32,
            height: (max_y - min_y) as u32,
        },
    })
}

unsafe extern "system" fn debug_callback(
    _severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    callback: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> vk::Bool32 {
    if !callback.is_null() {
        let message = unsafe { CStr::from_ptr((*callback).p_message) };
        eprintln!("editur: Vulkan validation: {}", message.to_string_lossy());
    }
    vk::FALSE
}

#[cfg(test)]
mod tests {
    use super::{choose_present_mode, vk};

    #[test]
    fn chooses_the_lowest_latency_supported_present_mode() {
        assert_eq!(
            choose_present_mode(&[vk::PresentModeKHR::FIFO, vk::PresentModeKHR::MAILBOX]),
            vk::PresentModeKHR::MAILBOX
        );
        assert_eq!(
            choose_present_mode(&[vk::PresentModeKHR::FIFO, vk::PresentModeKHR::IMMEDIATE]),
            vk::PresentModeKHR::IMMEDIATE
        );
        assert_eq!(
            choose_present_mode(&[vk::PresentModeKHR::FIFO]),
            vk::PresentModeKHR::FIFO
        );
    }
}
