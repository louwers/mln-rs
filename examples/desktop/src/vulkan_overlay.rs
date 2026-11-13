// The controls' overlay on Vulkan: egui's meshes drawn over the map's frame,
// in its command buffer, as egui-wgpu draws them on WebGPU (overlay.rs): in
// gamma space, premultiplied, with egui-wgpu's blending, but no dithering.
//
// The textures and the frames' vertex and index buffers are the backend's
// (`Texture2D`, `BufferResource`), which destroy themselves once the frames
// that use them are done. What the painter makes itself (the pipeline, its
// layouts and the descriptor pools) goes through the context's deletion
// queue for the same reason, and so does a freed texture's descriptor set
// on its way to the next texture.

// Vulkan through ash is the FFI here.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use ash::vk;
use mln_gfx::texture2d::{SamplerState, Texture2D as _};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_vulkan::Context;
use mln_vulkan::renderer_backend::BackendImpl;
use mln_vulkan::shaders::compiler::{Stage, compile};
use mln_vulkan::surface_target::{Overlay, OverlayTarget};
use mln_vulkan::texture2d::Texture2D;

use crate::overlay::Paint;

// `screen_size` is in points, as the vertices are. Vulkan's clip space has y
// down, as egui's points have.
const VERTEX_SHADER: &str = r"#version 450
layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color;

layout(location = 0) out vec2 frag_uv;
layout(location = 1) out vec4 frag_color;

layout(push_constant) uniform Constants {
    vec2 screen_size;
} constant;

void main() {
    gl_Position = vec4(2.0 * in_pos / constant.screen_size - 1.0, 0.0, 1.0);
    frag_uv = in_uv;
    frag_color = in_color;
}
";

const FRAGMENT_SHADER: &str = r"#version 450
layout(location = 0) in vec2 frag_uv;
layout(location = 1) in vec4 frag_color;

layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform texture2D tex;
layout(set = 0, binding = 1) uniform sampler tex_sampler;

void main() {
    out_color = frag_color * texture(sampler2D(tex, tex_sampler), frag_uv);
}
";

/// The size of an `epaint::Vertex`: position and texture coordinates as f32
/// pairs, then the color as four bytes.
const VERTEX_STRIDE: u32 = 20;
/// The descriptor sets of a pool, one for each of egui's textures.
const POOL_SETS: u32 = 16;
/// The bindings of a texture's descriptor set.
const TEXTURE_BINDING: u32 = 0;
const SAMPLER_BINDING: u32 = 1;

/// One of egui's textures, with the descriptor set that binds it.
struct Texture {
    texture: Rc<RefCell<Texture2D>>,
    set: vk::DescriptorSet,
}

struct Painter {
    backend: Rc<BackendImpl>,
    context: Weak<Context>,
    set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    /// The render pass `pipeline` is for.
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    pools: Vec<vk::DescriptorPool>,
    /// The sets the newest pool has left.
    pool_sets_left: u32,
    /// The sets of freed textures that no frame uses any more.
    free_sets: Rc<RefCell<Vec<vk::DescriptorSet>>>,
    textures: HashMap<egui::TextureId, Texture>,
}

/// The overlay that paints the controls on Vulkan, made with the first
/// frame.
pub(crate) fn vulkan_overlay(paint: Rc<RefCell<Paint>>) -> Overlay {
    let mut painter: Option<Painter> = None;
    let mut failed = false;
    Box::new(move |target: &OverlayTarget<'_>| {
        if failed {
            return;
        }
        if painter.is_none() {
            match Painter::new(target) {
                Ok(new) => painter = Some(new),
                Err(e) => {
                    Log::error(Event::Render, format!("The controls can't be drawn: {e}"));
                    failed = true;
                    return;
                }
            }
        }
        let Some(painter) = painter.as_mut() else { return };
        let mut paint = paint.borrow_mut();
        let mut textures = std::mem::take(&mut paint.textures);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                painter.update_texture(target, *id, delta);
            }
        }
        if let Err(e) = painter.paint(target, &paint.primitives, paint.pixels_per_point) {
            Log::error(Event::Render, format!("The controls can't be drawn: {e}"));
            failed = true;
        }
        for id in &textures.free {
            if let Some(texture) = painter.textures.remove(id) {
                painter.free_texture(texture);
            }
        }
        textures.clear();
    })
}

/// Runs `destroy` once the frames that use what it destroys are done, or
/// now if the context is gone, and its frames with it.
fn destroy_later(context: &Weak<Context>, destroy: impl FnOnce() + 'static) {
    match context.upgrade() {
        Some(context) => context.enqueue_deletion(Box::new(move |_| destroy())),
        None => destroy(),
    }
}

impl Painter {
    fn new(target: &OverlayTarget<'_>) -> Result<Painter, String> {
        let device = target.device;
        let bindings =
            [(TEXTURE_BINDING, vk::DescriptorType::SAMPLED_IMAGE), (SAMPLER_BINDING, vk::DescriptorType::SAMPLER)].map(
                |(binding, descriptor_type)| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(binding)
                        .descriptor_type(descriptor_type)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                },
            );
        let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: the create info is valid for the device.
        let set_layout = unsafe { device.create_descriptor_set_layout(&create_info, None) }
            .map_err(|e| format!("descriptor set layout: {e}"))?;

        let push_constants = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .size(std::mem::size_of::<[f32; 2]>() as u32)];
        let set_layouts = [set_layout];
        let create_info =
            vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts).push_constant_ranges(&push_constants);
        // SAFETY: the set layout is live. Nothing uses it if the pipeline
        // layout can't be made.
        let pipeline_layout = unsafe {
            device.create_pipeline_layout(&create_info, None).map_err(|e| {
                device.destroy_descriptor_set_layout(set_layout, None);
                format!("pipeline layout: {e}")
            })?
        };

        // Dropping the painter destroys what a failed step leaves.
        let mut painter = Painter {
            backend: target.context.get_backend().clone(),
            context: target.context.weak(),
            set_layout,
            pipeline_layout,
            render_pass: vk::RenderPass::null(),
            pipeline: vk::Pipeline::null(),
            pools: Vec::new(),
            pool_sets_left: 0,
            free_sets: Rc::default(),
            textures: HashMap::new(),
        };
        painter.set_render_pass(device, target.render_pass)?;
        Ok(painter)
    }

    /// Makes the pipeline for `render_pass`, unless it is the one it has.
    fn set_render_pass(&mut self, device: &ash::Device, render_pass: vk::RenderPass) -> Result<(), String> {
        if self.pipeline != vk::Pipeline::null() && self.render_pass == render_pass {
            return Ok(());
        }
        let module = |stage: Stage, source: &str| {
            let spirv = compile(stage, source)?;
            let create_info = vk::ShaderModuleCreateInfo::default().code(&spirv);
            // SAFETY: the code is the SPIR-V module naga wrote.
            unsafe { device.create_shader_module(&create_info, None) }.map_err(|e| format!("shader module: {e}"))
        };
        let vertex = module(Stage::Vertex, VERTEX_SHADER)?;
        let pipeline = module(Stage::Fragment, FRAGMENT_SHADER).and_then(|fragment| {
            let pipeline = self.create_pipeline(device, render_pass, vertex, fragment);
            // SAFETY: a pipeline keeps nothing of the modules it was made
            // from.
            unsafe { device.destroy_shader_module(fragment, None) };
            pipeline
        });
        // SAFETY: as above.
        unsafe { device.destroy_shader_module(vertex, None) };
        let old = std::mem::replace(&mut self.pipeline, pipeline?);
        self.render_pass = render_pass;
        if old != vk::Pipeline::null() {
            let backend = self.backend.clone();
            // SAFETY: the frames that drew with the pipeline are done.
            destroy_later(&self.context, move || unsafe { backend.get_device().destroy_pipeline(old, None) });
        }
        Ok(())
    }

    fn create_pipeline(
        &self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        vertex: vk::ShaderModule,
        fragment: vk::ShaderModule,
    ) -> Result<vk::Pipeline, String> {
        let stages = [(vk::ShaderStageFlags::VERTEX, vertex), (vk::ShaderStageFlags::FRAGMENT, fragment)].map(
            |(stage, module)| vk::PipelineShaderStageCreateInfo::default().stage(stage).module(module).name(c"main"),
        );
        let vertex_bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(VERTEX_STRIDE)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let vertex_attributes =
            [(0, vk::Format::R32G32_SFLOAT, 0), (1, vk::Format::R32G32_SFLOAT, 8), (2, vk::Format::R8G8B8A8_UNORM, 16)]
                .map(|(location, format, offset)| {
                    vk::VertexInputAttributeDescription::default()
                        .binding(0)
                        .location(location)
                        .format(format)
                        .offset(offset)
                });
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&vertex_bindings)
            .vertex_attribute_descriptions(&vertex_attributes);
        let input_assembly =
            vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        // The viewport and the scissor rectangle are set with each frame.
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample =
            vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // egui-wgpu's blending, of premultiplied colors.
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .color_blend_op(vk::BlendOp::ADD)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_DST_ALPHA)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let create_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .dynamic_state(&dynamic)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .layout(self.pipeline_layout)
            .render_pass(render_pass)
            .subpass(0);
        // SAFETY: the create info points at live state, the modules have
        // the stages' `main`, and the render pass has one color attachment
        // and no depth or stencil.
        let pipelines = unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[create_info], None) };
        match pipelines {
            Ok(pipelines) => pipelines.first().copied().ok_or_else(|| "no pipeline".to_owned()),
            Err((_, e)) => Err(format!("pipeline: {e}")),
        }
    }

    /// A descriptor set for a texture: a freed texture's, or one of the
    /// newest pool, or of a new pool when that one has none left.
    fn allocate_set(&mut self, device: &ash::Device) -> Result<vk::DescriptorSet, String> {
        if let Some(set) = self.free_sets.borrow_mut().pop() {
            return Ok(set);
        }
        if self.pool_sets_left == 0 {
            let sizes = [vk::DescriptorType::SAMPLED_IMAGE, vk::DescriptorType::SAMPLER]
                .map(|ty| vk::DescriptorPoolSize::default().ty(ty).descriptor_count(POOL_SETS));
            let create_info = vk::DescriptorPoolCreateInfo::default().max_sets(POOL_SETS).pool_sizes(&sizes);
            // SAFETY: the create info is valid for the device.
            let pool = unsafe { device.create_descriptor_pool(&create_info, None) }
                .map_err(|e| format!("descriptor pool: {e}"))?;
            self.pools.push(pool);
            self.pool_sets_left = POOL_SETS;
        }
        let pool = self.pools.last().copied().ok_or("descriptor pool")?;
        let set_layouts = [self.set_layout];
        let allocate_info = vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&set_layouts);
        // SAFETY: the pool and the layout are live, only this thread uses
        // the pool, and it has a set left.
        let sets =
            unsafe { device.allocate_descriptor_sets(&allocate_info) }.map_err(|e| format!("descriptor set: {e}"))?;
        self.pool_sets_left -= 1;
        sets.first().copied().ok_or_else(|| "descriptor set".to_owned())
    }

    /// Frees a texture: its descriptor set is another texture's to take
    /// once the frames that draw with it are done, when the texture
    /// destroys itself too.
    fn free_texture(&self, texture: Texture) {
        let Texture { texture, set } = texture;
        let free_sets = self.free_sets.clone();
        destroy_later(&self.context, move || free_sets.borrow_mut().push(set));
        drop(texture);
    }

    /// Applies one of egui's texture updates: a whole new texture, or a
    /// region of an existing one.
    fn update_texture(&mut self, target: &OverlayTarget<'_>, id: egui::TextureId, delta: &egui::epaint::ImageDelta) {
        let egui::ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        if width == 0 || height == 0 {
            return;
        }
        let pixels: Vec<u8> = image.pixels.iter().flat_map(egui::Color32::to_array).collect();
        let size = Size::new(width as u32, height as u32);
        if let Some([x, y]) = delta.pos {
            if let Some(texture) = self.textures.get(&id) {
                texture.texture.borrow_mut().upload_sub_region(Some(&pixels), size, x as u16, y as u16);
            }
        } else {
            if let Some(old) = self.textures.remove(&id) {
                self.free_texture(old);
            }
            let texture = target.context.create_vulkan_texture_2d();
            let (image_view, sampler, layout) = {
                let mut texture = texture.borrow_mut();
                texture.set_format(TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte);
                texture.set_sampler_configuration(sampler_state(delta.options));
                texture.upload_data(Some(&pixels), size);
                (texture.get_vulkan_image_view(), texture.get_vulkan_sampler(), texture.get_vulkan_image_layout())
            };
            if image_view == vk::ImageView::null() || sampler == vk::Sampler::null() {
                return;
            }
            let Ok(set) = self.allocate_set(target.device) else {
                return;
            };
            let image_info = [vk::DescriptorImageInfo::default().image_view(image_view).image_layout(layout)];
            let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(TEXTURE_BINDING)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(SAMPLER_BINDING)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&sampler_info),
            ];
            // SAFETY: no frame uses the set, and the view and the
            // sampler are the texture's, which is freed with the set.
            unsafe { target.device.update_descriptor_sets(&writes, &[]) };
            self.textures.insert(id, Texture { texture, set });
        }
    }

    /// Records the controls' meshes into the target's render pass, clipped
    /// to their rectangles.
    fn paint(
        &mut self,
        target: &OverlayTarget<'_>,
        primitives: &[egui::ClippedPrimitive],
        pixels_per_point: f32,
    ) -> Result<(), String> {
        let meshes: Vec<(&egui::Rect, &egui::Mesh)> = primitives
            .iter()
            .filter_map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(mesh) if !mesh.indices.is_empty() => Some((&p.clip_rect, mesh)),
                _ => None,
            })
            .collect();
        if meshes.is_empty() {
            return Ok(());
        }
        self.set_render_pass(target.device, target.render_pass)?;

        // All the meshes' vertices and indexes, each mesh's after the last.
        let mut vertices: Vec<u8> = Vec::new();
        let mut indexes: Vec<u8> = Vec::new();
        let mut draws = Vec::with_capacity(meshes.len());
        for (clip, mesh) in &meshes {
            let first_vertex = vertices.len() / VERTEX_STRIDE as usize;
            let first_index = indexes.len() / std::mem::size_of::<u32>();
            draws.push((*clip, mesh.texture_id, first_vertex, first_index, mesh.indices.len()));
            for vertex in &mesh.vertices {
                for value in [vertex.pos.x, vertex.pos.y, vertex.uv.x, vertex.uv.y] {
                    vertices.extend_from_slice(&value.to_ne_bytes());
                }
                vertices.extend_from_slice(&vertex.color.to_array());
            }
            for index in &mesh.indices {
                indexes.extend_from_slice(&index.to_ne_bytes());
            }
        }
        // Dropped after the draws are recorded, and destroyed when the
        // frame is done.
        let vertex_buffer =
            target.context.create_buffer(Some(&vertices), vertices.len(), vk::BufferUsageFlags::VERTEX_BUFFER, false);
        let index_buffer =
            target.context.create_buffer(Some(&indexes), indexes.len(), vk::BufferUsageFlags::INDEX_BUFFER, false);
        if !vertex_buffer.is_valid() || !index_buffer.is_valid() {
            return Ok(());
        }

        let (device, command_buffer) = (target.device, target.command_buffer);
        let (width, height) = (target.size.width as f32, target.size.height as f32);
        let screen_size = [width / pixels_per_point, height / pixels_per_point];
        let viewport = vk::Viewport { x: 0.0, y: 0.0, width, height, min_depth: 0.0, max_depth: 1.0 };
        // SAFETY: the command buffer is in the render pass the pipeline was
        // made for, the buffers hold the meshes' vertices and indexes, and
        // the screen size fills the push constants of the layout.
        unsafe {
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_set_viewport(command_buffer, 0, &[viewport]);
            device.cmd_push_constants(
                command_buffer,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                &screen_size.map(f32::to_ne_bytes).concat(),
            );
            device.cmd_bind_vertex_buffers(command_buffer, 0, &[vertex_buffer.get_vulkan_buffer()], &[0]);
            device.cmd_bind_index_buffer(command_buffer, index_buffer.get_vulkan_buffer(), 0, vk::IndexType::UINT32);
        }

        for (clip, texture_id, first_vertex, first_index, index_count) in draws {
            // The clip rectangle in pixels, inside the target.
            let x0 = (clip.min.x * pixels_per_point).round().clamp(0.0, width) as u32;
            let y0 = (clip.min.y * pixels_per_point).round().clamp(0.0, height) as u32;
            let x1 = (clip.max.x * pixels_per_point).round().clamp(0.0, width) as u32;
            let y1 = (clip.max.y * pixels_per_point).round().clamp(0.0, height) as u32;
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let Some(texture) = self.textures.get(&texture_id) else {
                continue;
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: x0 as i32, y: y0 as i32 },
                extent: vk::Extent2D { width: x1 - x0, height: y1 - y0 },
            };
            // SAFETY: the set binds a live texture and sampler, which stay
            // until the frame is done, the scissor rectangle is inside the
            // target, and the indexes and the vertices they name are inside
            // the buffers.
            unsafe {
                device.cmd_set_scissor(command_buffer, 0, &[scissor]);
                device.cmd_bind_descriptor_sets(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[texture.set],
                    &[],
                );
                device.cmd_draw_indexed(
                    command_buffer,
                    index_count as u32,
                    1,
                    first_index as u32,
                    first_vertex as i32,
                    0,
                );
            }
        }
        Ok(())
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        let backend = self.backend.clone();
        let (set_layout, pipeline_layout, pipeline) = (self.set_layout, self.pipeline_layout, self.pipeline);
        let pools = std::mem::take(&mut self.pools);
        // The textures go first: each waits for the frames that use it.
        self.textures.clear();
        destroy_later(&self.context, move || {
            let device = backend.get_device();
            // SAFETY: the frames that drew with these are done, and nothing
            // else refers to them: destroying a pool frees its sets.
            unsafe {
                device.destroy_pipeline(pipeline, None);
                device.destroy_pipeline_layout(pipeline_layout, None);
                device.destroy_descriptor_set_layout(set_layout, None);
                for pool in pools {
                    device.destroy_descriptor_pool(pool, None);
                }
            }
        });
    }
}

/// The backend's sampler configuration for egui's texture options. The
/// backend's textures have one filter, for which egui's magnification
/// filter is taken, and repeat without mirroring.
fn sampler_state(options: egui::TextureOptions) -> SamplerState {
    let wrap = match options.wrap_mode {
        egui::TextureWrapMode::ClampToEdge => TextureWrapType::Clamp,
        egui::TextureWrapMode::Repeat | egui::TextureWrapMode::MirroredRepeat => TextureWrapType::Repeat,
    };
    SamplerState {
        filter: match options.magnification {
            egui::TextureFilter::Nearest => TextureFilterType::Nearest,
            egui::TextureFilter::Linear => TextureFilterType::Linear,
        },
        wrap_u: wrap,
        wrap_v: wrap,
        ..SamplerState::default()
    }
}
