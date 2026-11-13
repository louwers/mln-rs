// Differences from C++:
// - The drawable's uniform buffer set is bound only when it has a buffer.
//   C++ binds one for every drawable, of placeholders for most: the layer
//   shaders read no drawable buffer.
// - A new shader has the vertex input rebuilt at the next upload; its
//   vertex and instance attributes can differ from the last one's.
// - `setSharedBuffers`, which binds the instance data of symbols and fill
//   extrusions as storage buffers, is not ported: instance attributes are
//   vertex inputs that step per instance.
// - A vertex input binding is a buffer read at a stride, a rate and a vertex
//   offset, and is bound at that offset. C++ has a binding for each buffer,
//   bound at its start, and reads a fill extrusion wall's next outline vertex
//   from the storage buffer.

use std::rc::Rc;

use ash::vk;
use mln_gfx::attribute::AttributeBindingArray;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::context::Context as _;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::drawable::{self, DrawSegment, DrawableBase, Textures, UniqueDrawSegment};
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::index_vector::{IndexBufferBase, IndexVectorBasePtr};
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr, same_shader};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::Texture2DPtr;
use mln_gfx::types::{BufferUsageType, DepthMaskType};
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_gfx::upload_pass::UploadPass as _;
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
use mln_gfx::vertex_buffer::IndexBuffer as GfxIndexBuffer;
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use crate::context::Context;
use crate::descriptor_set::{ImageDescriptorSet, UniformDescriptorSet};
use crate::pipeline::PipelineInfo;
use crate::render_pass::RenderPass;
use crate::shaders::layout::{DescriptorSetType, UBO_INDEX_PUSH_CONSTANT_SIZE};
use crate::shaders::shader_program::as_vulkan_shader;
use crate::upload_pass::UploadPass;
use crate::vertex_buffer_resource::{IndexBufferResource, VertexBufferResource};

/// The uploaded buffer of an index vector.
struct IndexBuffer {
    buffer: GfxIndexBuffer,
}

impl IndexBufferBase for IndexBuffer {}

struct DrawableImpl {
    segments: Vec<UniqueDrawSegment>,

    indexes: Option<IndexVectorBasePtr>,
    vertex_count: usize,
    vertex_type: AttributeDataType,

    attribute_bindings: AttributeBindingArray,
    instance_bindings: AttributeBindingArray,

    uniform_buffers: UniformBufferArray,
    uniform_descriptor_set: Option<UniformDescriptorSet>,

    vertex_attr_id: usize,

    depth_for_3d: Option<DepthMode>,
    stencil_for_3d: Option<StencilMode>,

    pipeline_info: PipelineInfo,

    vulkan_vertex_buffers: Vec<vk::Buffer>,
    vulkan_vertex_offsets: Vec<vk::DeviceSize>,

    image_descriptor_set: Option<ImageDescriptorSet>,
}

pub(crate) struct Drawable {
    base: DrawableBase,
    impl_: DrawableImpl,
}

/// What a vertex input binding reads: attributes that agree on it share a
/// binding.
#[derive(Clone, Copy, PartialEq)]
struct InputSource {
    /// The address of the buffer's resource.
    buffer: *const (),
    stride: u32,
    input_rate: vk::VertexInputRate,
    /// Where in the buffer the binding starts, in bytes.
    offset: vk::DeviceSize,
}

fn same_textures(a: &Textures, b: &Textures) -> bool {
    a.iter().zip(b).all(|(a, b)| match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    })
}

impl Drawable {
    #[must_use]
    pub(crate) fn new(name: String) -> Self {
        Drawable {
            base: DrawableBase::new(name),
            impl_: DrawableImpl {
                segments: Vec::new(),
                indexes: None,
                vertex_count: 0,
                vertex_type: AttributeDataType::Invalid,
                attribute_bindings: Vec::new(),
                instance_bindings: Vec::new(),
                uniform_buffers: UniformBufferArray::new(),
                uniform_descriptor_set: None,
                vertex_attr_id: 0,
                depth_for_3d: None,
                stencil_for_3d: None,
                pipeline_info: PipelineInfo::default(),
                vulkan_vertex_buffers: Vec::new(),
                vulkan_vertex_offsets: Vec::new(),
                image_descriptor_set: None,
            },
        }
    }

    pub(crate) fn set_vertex_attr_id(&mut self, id: usize) {
        self.impl_.vertex_attr_id = id;
    }

    pub(crate) fn set_depth_mode_for_3d(&mut self, value: &DepthMode) {
        self.impl_.depth_for_3d = Some(*value);
    }

    pub(crate) fn set_stencil_mode_for_3d(&mut self, value: &StencilMode) {
        self.impl_.stencil_for_3d = Some(*value);
    }

    pub(crate) fn upload(&mut self, upload_pass: &UploadPass) {
        let Some(shader_ptr) = self.base.shader.clone() else {
            Log::warning(
                Event::General,
                format!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name),
            );
            debug_assert!(false);
            return;
        };
        let Some(shader) = as_vulkan_shader(&shader_ptr) else {
            return;
        };
        let usage = BufferUsageType::StaticDraw;

        // We need either raw index data or a buffer already created from
        // them. We can have a buffer and no indexes, but only if it's not
        // marked dirty.
        let Some(indexes) = self.impl_.indexes.clone() else {
            debug_assert!(false, "Missing index data");
            return;
        };
        {
            let mut indexes = indexes.borrow_mut();
            if indexes.empty() && (indexes.get_buffer().is_none() || indexes.get_dirty()) {
                debug_assert!(false, "Missing index data");
                return;
            }
            if indexes.get_buffer().is_none() || indexes.get_dirty() {
                // Create a buffer for the index data. We don't update any
                // existing buffer because it may still be in use by the
                // previous frame.
                let resource = upload_pass.create_index_buffer_of(&indexes);
                let buffer = GfxIndexBuffer::new(resource);
                indexes.set_buffer(Some(Box::new(IndexBuffer { buffer })));
                indexes.set_dirty(false);
            }
        }

        // No vertex input yet, or a new shader's to build.
        let no_vertex_input = self.impl_.pipeline_info.input_attributes.is_empty();
        let build_attribs = no_vertex_input
            || match (&self.base.vertex_attributes, self.base.attribute_update_time) {
                (Some(attrs), Some(t)) => attrs.borrow().is_modified_after(t),
                _ => true,
            };

        if build_attribs {
            let attrs = self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr).clone();
            // Apply drawable values to shader defaults.
            let attribute_bindings = upload_pass.build_attribute_bindings(
                self.impl_.vertex_count,
                self.impl_.vertex_type,
                /*vertex_attribute_index=*/ usize::MAX,
                /*vertex_data=*/ &[],
                shader.get_vertex_attributes(),
                &mut attrs.borrow_mut(),
                usage,
                self.base.attribute_update_time,
            );
            attrs.borrow().visit_attributes(|attrib| attrib.set_dirty(false));
            if self.impl_.attribute_bindings != attribute_bindings {
                self.impl_.attribute_bindings = attribute_bindings;
            }
        }

        // build instance buffer
        let build_instance_buffer = match (&self.base.instance_attributes, self.base.attribute_update_time) {
            (Some(attrs), Some(t)) => no_vertex_input || attrs.borrow().is_modified_after(t),
            (Some(_), None) => true,
            (None, _) => false,
        };

        if build_instance_buffer && let Some(attrs) = self.base.instance_attributes.clone() {
            // Build instance attribute buffers.
            let count = attrs.borrow().get_min_count();
            let instance_bindings = upload_pass.build_attribute_bindings(
                count,
                /*vertex_type=*/ AttributeDataType::Byte,
                /*vertex_attribute_index=*/ usize::MAX,
                /*vertex_data=*/ &[],
                shader.get_instance_attributes(),
                &mut attrs.borrow_mut(),
                usage,
                self.base.attribute_update_time,
            );
            // clear dirty flag
            attrs.borrow().visit_attributes(|attrib| attrib.set_dirty(false));
            if self.impl_.instance_bindings != instance_bindings {
                self.impl_.instance_bindings = instance_bindings;
            }
        }

        if build_attribs || build_instance_buffer {
            self.build_vulkan_input_bindings(upload_pass.vulkan_context());
        }

        let textures_need_upload = self.base.textures.iter().flatten().any(|t| t.borrow().needs_upload());
        if textures_need_upload {
            self.upload_textures();
        }

        self.base.attribute_update_time = Some(Instant::now());
    }

    /// Builds the vertex input of the pipelines, a binding for each way the
    /// attributes read a buffer, and the buffers and offsets to bind at them.
    fn build_vulkan_input_bindings(&mut self, context: &Context) {
        let impl_ = &mut self.impl_;
        impl_.vulkan_vertex_buffers.clear();
        impl_.vulkan_vertex_offsets.clear();
        impl_.pipeline_info.input_attributes.clear();
        impl_.pipeline_info.input_bindings.clear();

        // What each binding reads, `None` for the placeholder's. Attributes
        // of a buffer at different vertex offsets (a fill extrusion wall's
        // outline vertex and the next one) read it through bindings of their
        // own, bound that far into it.
        let mut unique_sources: Vec<Option<InputSource>> = Vec::new();
        let mut dummy_binding_index: Option<u32> = None;

        for (bindings, input_rate) in [
            (&impl_.attribute_bindings, vk::VertexInputRate::VERTEX),
            (&impl_.instance_bindings, vk::VertexInputRate::INSTANCE),
        ] {
            for (i, binding) in bindings.iter().enumerate() {
                let Some(binding) = binding else {
                    continue;
                };
                let vertex_buffer = binding
                    .vertex_buffer_resource
                    .as_ref()
                    .and_then(|resource| Some((resource, resource.downcast_ref::<VertexBufferResource>()?)))
                    .filter(|(_, buffer)| buffer.get().is_valid());
                let Some((resource, vertex_buffer)) = vertex_buffer else {
                    // Back missing attributes with a zero-stride dummy
                    // buffer that reads a constant value, per vertex or per
                    // instance alike.
                    let binding_index = *dummy_binding_index.get_or_insert_with(|| {
                        let index = impl_.pipeline_info.input_bindings.len() as u32;
                        // sync `unique_sources` with `input_bindings`
                        unique_sources.push(None);
                        impl_.pipeline_info.input_bindings.push(
                            vk::VertexInputBindingDescription::default()
                                .binding(index)
                                .stride(0)
                                .input_rate(input_rate),
                        );
                        impl_.vulkan_vertex_buffers.push(context.get_dummy_buffer());
                        impl_.vulkan_vertex_offsets.push(0);
                        index
                    });
                    impl_.pipeline_info.input_attributes.push(
                        vk::VertexInputAttributeDescription::default()
                            .binding(binding_index)
                            .location(i as u32)
                            .format(PipelineInfo::vulkan_format(binding.attribute.data_type))
                            .offset(0),
                    );
                    continue;
                };

                let source = InputSource {
                    buffer: Rc::as_ptr(resource).cast(),
                    stride: binding.vertex_stride,
                    input_rate,
                    offset: vk::DeviceSize::from(binding.vertex_offset) * vk::DeviceSize::from(binding.vertex_stride),
                };
                let binding_index = if let Some(index) = unique_sources.iter().position(|s| *s == Some(source)) {
                    index
                } else {
                    let index = impl_.pipeline_info.input_bindings.len();
                    unique_sources.push(Some(source));
                    // add new buffer binding
                    impl_.pipeline_info.input_bindings.push(
                        vk::VertexInputBindingDescription::default()
                            .binding(index as u32)
                            .stride(source.stride)
                            .input_rate(input_rate),
                    );
                    impl_.vulkan_vertex_buffers.push(vertex_buffer.get().get_vulkan_buffer());
                    impl_.vulkan_vertex_offsets.push(source.offset);
                    index
                };
                impl_.pipeline_info.input_attributes.push(
                    vk::VertexInputAttributeDescription::default()
                        .binding(binding_index as u32)
                        .location(i as u32)
                        .format(PipelineInfo::vulkan_format(binding.attribute.data_type))
                        .offset(binding.attribute.offset),
                );
            }
        }

        impl_.pipeline_info.update_vertex_input_hash();
    }

    fn bind_attributes(&self, context: &Context, command_buffer: vk::CommandBuffer) -> bool {
        if self.impl_.vulkan_vertex_buffers.is_empty() {
            return false;
        }
        let device = context.get_backend().get_device();
        // SAFETY: the buffers are kept by the attribute bindings and the
        // index vector, and the command buffer is recording.
        unsafe {
            device.cmd_bind_vertex_buffers(
                command_buffer,
                0,
                &self.impl_.vulkan_vertex_buffers,
                &self.impl_.vulkan_vertex_offsets,
            );
            if let Some(indexes) = &self.impl_.indexes {
                let indexes = indexes.borrow();
                let index_buffer = indexes
                    .get_buffer()
                    .and_then(|b| b.downcast_ref::<IndexBuffer>())
                    .and_then(|b| b.buffer.get_resource::<IndexBufferResource>())
                    .filter(|resource| resource.get().is_valid());
                if let Some(index_buffer) = index_buffer {
                    device.cmd_bind_index_buffer(
                        command_buffer,
                        index_buffer.get().get_vulkan_buffer(),
                        0,
                        vk::IndexType::UINT16,
                    );
                }
            }
        }
        true
    }

    fn bind_descriptors(&mut self, context: &Context, command_buffer: vk::CommandBuffer) -> bool {
        let Some(shader_ptr) = self.base.shader.clone() else {
            return false;
        };
        let Some(shader) = as_vulkan_shader(&shader_ptr) else {
            return false;
        };

        // bind uniforms
        let uniform_buffers = &self.impl_.uniform_buffers;
        if (0..uniform_buffers.allocated_size()).any(|id| uniform_buffers.get(id).is_some()) {
            self.impl_
                .uniform_descriptor_set
                .get_or_insert_with(|| UniformDescriptorSet::new(context, DescriptorSetType::DrawableUniform))
                .bind(context, command_buffer, uniform_buffers);
        }

        if shader.has_textures() {
            // update image set
            self.impl_.image_descriptor_set.get_or_insert_with(|| ImageDescriptorSet::new(context)).bind(
                context,
                command_buffer,
                &self.base.textures,
                shader,
            );
        }
        true
    }

    fn upload_textures(&self) {
        for texture in self.base.textures.iter().flatten() {
            texture.borrow_mut().upload();
        }
    }
}

impl drawable::Drawable for Drawable {
    fn base(&self) -> &DrawableBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut DrawableBase {
        &mut self.base
    }

    fn draw(&mut self, parameters: &mut dyn PaintParameters) {
        let parameters: &dyn PaintParameters = parameters;
        let Some(context) = parameters.context().downcast_ref::<Context>() else {
            return;
        };
        let Some(render_pass) = parameters.render_pass().and_then(|p| p.downcast_ref::<RenderPass>()) else {
            debug_assert!(false);
            return;
        };
        if !render_pass.is_encoding() {
            return;
        }
        let Some(shader_ptr) = self.base.shader.clone() else {
            Log::warning(
                Event::General,
                format!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name),
            );
            debug_assert!(false);
            return;
        };
        let Some(shader) = as_vulkan_shader(&shader_ptr) else {
            return;
        };
        let backend = context.get_backend();
        let device = backend.get_device();
        let command_buffer = render_pass.get_command_buffer();

        if !self.bind_attributes(context, command_buffer) {
            return;
        }
        if !self.bind_descriptors(context, command_buffer) {
            return;
        }

        debug_assert_eq!(size_of_val(&self.base.ubo_index), UBO_INDEX_PUSH_CONSTANT_SIZE as usize);
        // SAFETY: the command buffer is recording, and the index is the
        // push constant of the general pipeline layout.
        unsafe {
            device.cmd_push_constants(
                command_buffer,
                context.get_general_pipeline_layout(),
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::bytes_of(&self.base.ubo_index),
            );
        }

        let pipeline_info = &mut self.impl_.pipeline_info;
        if self.base.enable_depth {
            if let Some(depth_mode) = &self.impl_.depth_for_3d {
                pipeline_info.set_depth_mode(depth_mode);
            } else if self.base.is_3d {
                pipeline_info.set_depth_mode(&parameters.depth_mode_for_3d(DepthMaskType::ReadWrite));
            } else {
                let depth_mode =
                    parameters.depth_mode_for_sublayer(self.base.sub_layer_index as u8, self.base.depth_type);
                pipeline_info.set_depth_mode(&depth_mode);
            }
        } else {
            pipeline_info.set_depth_mode(&DepthMode::disabled());
        }

        let mut stencil_mode = StencilMode::disabled();
        if self.base.enable_stencil {
            if let Some(mode) = self.impl_.stencil_for_3d {
                stencil_mode = mode;
            } else if !self.base.is_3d
                && let Some(tile_id) = self.base.tile_id
            {
                stencil_mode = parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped());
            }
        }
        pipeline_info.set_stencil_mode(&stencil_mode);

        pipeline_info.set_render_target(render_pass.get_vulkan_render_pass(), render_pass.get_extent());

        let mut draw_calls = 0;
        for seg in &self.impl_.segments {
            let segment = seg.get_segment();

            // update pipeline info with per segment modifiers
            pipeline_info.set_draw_mode(*seg.get_mode());
            pipeline_info.set_scissor_rect(&parameters.scissor_rect());
            pipeline_info.set_dynamic_values(backend, command_buffer);

            let Some(pipeline) = shader.get_pipeline(pipeline_info) else {
                continue;
            };
            // SAFETY: the pipeline was made for this render pass and the
            // vertex input of the bound buffers; the segment's indexes and
            // vertices are within them.
            unsafe {
                device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, pipeline);
                if segment.index_length != 0 {
                    device.cmd_draw_indexed(
                        command_buffer,
                        segment.index_length as u32,
                        segment.instance_count as u32,
                        segment.index_offset as u32,
                        segment.vertex_offset as i32,
                        segment.base_instance as u32,
                    );
                } else {
                    device.cmd_draw(
                        command_buffer,
                        segment.vertex_length as u32,
                        segment.instance_count as u32,
                        segment.vertex_offset as u32,
                        segment.base_instance as u32,
                    );
                }
            }
            draw_calls += 1;
        }
        if draw_calls > 0 {
            let mut stats = context.rendering_stats();
            stats.num_draw_calls += draw_calls;
            stats.total_draw_calls += draw_calls;
        }
    }

    fn update_vertex_attributes(
        &mut self,
        vertices: Option<VertexAttributeArrayPtr>,
        vertex_count: usize,
        mode: DrawMode,
        indexes: Option<IndexVectorBasePtr>,
        segments: &[SegmentBase],
    ) {
        drawable::Drawable::set_vertex_attributes(self, vertices);
        self.impl_.vertex_count = vertex_count;
        self.impl_.segments = segments.iter().map(|seg| Box::new(DrawSegment::new(mode, seg.copy()))).collect();
        self.impl_.indexes = indexes;
    }

    fn set_vertices(&mut self, data: Vec<u8>, count: usize, data_type: AttributeDataType) {
        self.impl_.vertex_count = count;
        self.impl_.vertex_type = data_type;
        if count != 0 && data_type != AttributeDataType::Invalid && !data.is_empty() {
            let attrs = self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr).clone();
            let mut attrs = attrs.borrow_mut();
            if let Some(attrib) = attrs.set(self.impl_.vertex_attr_id, /*index=*/ -1, data_type, 0) {
                attrib.set_raw_data(data);
                attrib.set_stride(VertexAttribute::get_stride_of(data_type));
            } else {
                Log::warning(
                    Event::General,
                    format!("Vertex attribute type mismatch: {} / {}", self.base.name, self.impl_.vertex_attr_id),
                );
                debug_assert!(false);
            }
        }
    }

    fn set_index_data(&mut self, indexes: Option<IndexVectorBasePtr>, segments: Vec<UniqueDrawSegment>) {
        debug_assert!(indexes.as_ref().is_some_and(|i| i.borrow().elements() != 0));
        self.impl_.indexes = indexes;
        self.impl_.segments = segments;
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.impl_.uniform_buffers
    }

    fn set_shader(&mut self, value: Option<ShaderProgramBasePtr>) {
        if same_shader(&self.base.shader, &value) {
            return;
        }
        self.impl_.pipeline_info.input_attributes.clear();
        self.base.shader = value;
    }

    fn set_enable_color(&mut self, value: bool) {
        self.base.enable_color = value;
        self.impl_.pipeline_info.color_mask =
            if value { vk::ColorComponentFlags::RGBA } else { vk::ColorComponentFlags::empty() };
    }

    fn set_color_mode(&mut self, value: &ColorMode) {
        self.base.color_mode = *value;
        self.impl_.pipeline_info.set_color_blend(value);
    }

    fn set_enable_depth(&mut self, value: bool) {
        self.base.enable_depth = value;
        self.impl_.pipeline_info.depth_test = value;
    }

    fn set_depth_type(&mut self, value: DepthMaskType) {
        self.base.depth_type = value;
        self.impl_.pipeline_info.set_depth_write(value);
    }

    fn set_line_width(&mut self, value: i32) {
        self.base.line_width = value;
        self.impl_.pipeline_info.wide_lines = value != 1;
        self.impl_.pipeline_info.dynamic_values.line_width = value as f32;
    }

    fn set_cull_face_mode(&mut self, value: &CullFaceMode) {
        self.base.cull_face_mode = *value;
        self.impl_.pipeline_info.set_cull_mode(*value);
    }

    fn set_textures(&mut self, textures: Textures) {
        if same_textures(&textures, &self.base.textures) {
            return;
        }
        self.base.textures = textures;
    }

    fn set_texture(&mut self, texture: Option<Texture2DPtr>, id: usize) {
        debug_assert!(id < self.base.textures.len());
        if id >= self.base.textures.len() {
            return;
        }
        self.base.textures[id] = texture;
    }
}
