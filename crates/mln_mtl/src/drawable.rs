// Differences from C++:
// - The bindings' buffer indexes come from the shader (`upload`).
// - An attribute without data gets the context's empty buffer bound, which
//   C++ fetches (`noBindingBuffer`) but doesn't bind.
// - The pipeline and depth-stencil states are dropped when the pass's
//   attachment formats change (C++: when its descriptor changes, and the
//   depth-stencil state not at all).

use bytemuck::bytes_of;
use mln_gfx::attribute::AttributeBindingArray;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::context::Context as _;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::drawable::{self, DrawSegment, DrawableBase, UniqueDrawSegment};
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::GpuError;
use mln_gfx::index_vector::IndexVectorBasePtr;
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBasePtr, same_shader};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_gfx::upload;
use mln_gfx::util_hash::{hash, hash_combine};
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;
use objc2::Message;
use objc2_metal::{
    MTLCullMode, MTLIndexType, MTLPrimitiveType, MTLRenderCommandEncoder, MTLScissorRect, MTLVertexDescriptor,
    MTLVertexFormat, MTLVertexStepFunction,
};

use crate::context::Context;
use crate::gpu::{map_cull_mode, map_winding_mode, primitive_type, vertex_format};
use crate::mtl_fwd::{MTLBufferPtr, MTLDepthStencilStatePtr, MTLRenderPipelineStatePtr, MTLVertexDescriptorPtr};
use crate::render_pass::{AttachmentFormats, RenderPass};
use crate::shaders::shader_program::{ShaderProgram, as_mtl_shader};

struct DrawableImpl {
    segments: Vec<UniqueDrawSegment>,
    vertex_desc: Option<MTLVertexDescriptorPtr>,
    vertex_desc_hash: u64,
    indexes: Option<IndexVectorBasePtr>,
    vertex_count: usize,
    vertex_type: AttributeDataType,
    attribute_bindings: AttributeBindingArray,
    instance_bindings: AttributeBindingArray,
    uniform_buffers: UniformBufferArray,
    vertex_attr_id: usize,
    pipeline_state: Option<MTLRenderPipelineStatePtr>,
    /// The attachments of the pass the states were made for.
    attachment_formats: Option<AttachmentFormats>,
    depth_stencil_state: Option<MTLDepthStencilStatePtr>,
    /// The stencil mode `depth_stencil_state` was made with, less its
    /// reference value.
    stencil_state_mode: StencilMode,
}

pub(crate) struct Drawable {
    base: DrawableBase,
    impl_: DrawableImpl,
}

/// Sets each binding's buffer index to the one the shader reads it from.
fn apply_buffer_indexes(bindings: &mut AttributeBindingArray, buffer_index_of: impl Fn(usize) -> Option<usize>) {
    for (index, binding) in bindings.iter_mut().enumerate() {
        if let Some(binding) = binding
            && let Some(buffer_index) = buffer_index_of(index)
        {
            binding.buffer_index = buffer_index as u32;
        }
    }
}

impl Drawable {
    #[must_use]
    pub(crate) fn new(name: String) -> Self {
        Drawable {
            base: DrawableBase::new(name),
            impl_: DrawableImpl {
                segments: Vec::new(),
                vertex_desc: None,
                vertex_desc_hash: 0,
                indexes: None,
                vertex_count: 0,
                vertex_type: AttributeDataType::Invalid,
                attribute_bindings: Vec::new(),
                instance_bindings: Vec::new(),
                uniform_buffers: UniformBufferArray::new(),
                vertex_attr_id: 0,
                pipeline_state: None,
                attachment_formats: None,
                depth_stencil_state: None,
                stencil_state_mode: StencilMode::default(),
            },
        }
    }

    pub(crate) fn set_vertex_attr_id(&mut self, id: usize) {
        self.impl_.vertex_attr_id = id;
    }

    fn upload_in(&mut self, context: &Context) -> Result<(), GpuError> {
        let Some(shader_ptr) = self.base.shader.clone() else {
            Log::warning(
                Event::General,
                format!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name),
            );
            debug_assert!(false);
            return Ok(());
        };
        let Some(shader) = as_mtl_shader(&shader_ptr) else {
            return Ok(());
        };
        // We need either raw index data or a buffer already created from
        // them. We can have a buffer and no indexes, but only if it's not
        // marked dirty.
        let Some(indexes) = self.impl_.indexes.clone() else {
            debug_assert!(false, "Missing index data");
            return Ok(());
        };
        {
            let mut indexes = indexes.borrow_mut();
            if indexes.empty() && (indexes.get_buffer().is_none() || indexes.get_dirty()) {
                debug_assert!(false, "Missing index data");
                return Ok(());
            }
            upload::upload_indexes(context, &mut indexes)?;
        }
        let force = self.base.attribute_update_time.is_none();

        let build_vertex_attribs = self.impl_.vertex_desc.is_none()
            || match (&self.base.vertex_attributes, self.base.attribute_update_time) {
                (Some(attrs), Some(t)) => attrs.borrow().is_modified_after(t),
                _ => true,
            };

        if build_vertex_attribs {
            let attrs =
                std::rc::Rc::clone(self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr));
            // Apply drawable values to shader defaults.
            let mut attribute_bindings = upload::build_attribute_bindings(
                context,
                mln_gfx::shaders::shader_program_base::ShaderProgramBase::get_vertex_attributes(shader),
                &mut attrs.borrow_mut(),
                force,
            )?;
            apply_buffer_indexes(&mut attribute_bindings, |index| shader.get_vertex_buffer_index(index));
            attrs.borrow().visit_attributes(|attrib| attrib.set_dirty(false));
            if self.impl_.attribute_bindings != attribute_bindings {
                self.impl_.attribute_bindings = attribute_bindings;
            }
        }

        let build_instance_attribs = match (&self.base.instance_attributes, self.base.attribute_update_time) {
            (Some(attrs), Some(t)) => attrs.borrow().is_modified_after(t),
            (Some(_), None) => true,
            (None, _) => false,
        };

        if build_instance_attribs && let Some(attrs) = self.base.instance_attributes.clone() {
            // Build instance attribute buffers.
            let mut instance_bindings = upload::build_attribute_bindings(
                context,
                mln_gfx::shaders::shader_program_base::ShaderProgramBase::get_instance_attributes(shader),
                &mut attrs.borrow_mut(),
                force,
            )?;
            apply_buffer_indexes(&mut instance_bindings, |index| shader.get_instance_buffer_index(index));
            attrs.borrow().visit_attributes(|attrib| attrib.set_dirty(false));
            if self.impl_.instance_bindings != instance_bindings {
                self.impl_.instance_bindings = instance_bindings;
            }
        }

        if build_vertex_attribs || build_instance_attribs {
            // Create a layout descriptor for each attribute.
            let vertex_desc = MTLVertexDescriptor::vertexDescriptor();
            let mut desc_hash = 0u64;
            let mut apply_bindings = |bindings: &AttributeBindingArray, step_function: MTLVertexStepFunction| {
                for (index, binding) in bindings.iter().enumerate() {
                    let Some(binding) = binding else {
                        continue;
                    };
                    let has_buffer = binding.buffer.is_some();
                    let buffer_index = binding.buffer_index as usize;
                    // SAFETY: the attribute and buffer indexes are below
                    // Metal's 31.
                    unsafe {
                        let attrib_desc = vertex_desc.attributes().objectAtIndexedSubscript(index);
                        attrib_desc.setBufferIndex(buffer_index);
                        attrib_desc.setOffset(binding.attribute.offset as usize);
                        attrib_desc
                            .setFormat(vertex_format(binding.attribute.data_type).unwrap_or(MTLVertexFormat::Invalid));

                        let layout_desc = vertex_desc.layouts().objectAtIndexedSubscript(buffer_index);
                        if layout_desc.stride() == 0 {
                            debug_assert!(binding.vertex_stride > 0);
                            layout_desc.setStride(binding.vertex_stride as usize);
                            layout_desc.setStepFunction(if has_buffer {
                                step_function
                            } else {
                                MTLVertexStepFunction::Constant
                            });
                            layout_desc.setStepRate(usize::from(has_buffer));
                        } else {
                            debug_assert_eq!(layout_desc.stride(), binding.vertex_stride as usize);
                        }
                    }
                    hash_combine(
                        &mut desc_hash,
                        hash(&[
                            index as u64,
                            u64::from(binding.attribute.offset),
                            binding.attribute.data_type as u64,
                            u64::from(binding.vertex_stride),
                            u64::from(has_buffer),
                        ]),
                    );
                }
            };
            apply_bindings(&self.impl_.attribute_bindings, MTLVertexStepFunction::PerVertex);
            apply_bindings(&self.impl_.instance_bindings, MTLVertexStepFunction::PerInstance);

            self.impl_.vertex_desc = Some(vertex_desc);
            self.impl_.vertex_desc_hash = desc_hash;
            self.impl_.pipeline_state = None;
        }

        for texture in self.base.textures.iter().flatten() {
            if let Err(error) = texture.borrow_mut().upload(context) {
                Log::error(Event::Render, format!("Uploading a texture failed: {error}"));
            }
        }

        self.base.attribute_update_time = Some(Instant::now());
        Ok(())
    }

    fn bind_attribute_bindings(
        bindings: &AttributeBindingArray,
        render_pass: &RenderPass,
        empty: Option<&MTLBufferPtr>,
    ) {
        for binding in bindings.iter().flatten() {
            match &binding.buffer {
                Some(buffer) => {
                    let offset = binding.vertex_offset as usize * binding.vertex_stride as usize;
                    render_pass.bind_vertex_buffer(buffer.id(), offset, binding.buffer_index as usize);
                }
                None => {
                    if let Some(empty) = empty {
                        render_pass.set_vertex_buffer(empty, 0, binding.buffer_index as usize);
                    }
                }
            }
        }
    }

    fn bind_textures(&self, context: &Context, render_pass: &RenderPass, shader: &ShaderProgram) {
        for (id, texture) in self.base.textures.iter().enumerate() {
            let Some(texture) = texture else { continue };
            let Some(location) =
                mln_gfx::shaders::shader_program_base::ShaderProgramBase::get_sampler_location(shader, id)
            else {
                continue;
            };
            let binding = texture.borrow_mut().binding(context);
            match binding {
                Ok((texture, sampler)) => render_pass.bind_texture(texture, sampler, location),
                Err(error) => Log::error(Event::Render, format!("Binding a texture failed: {error}")),
            }
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

    fn upload(&mut self, context: &dyn mln_gfx::context::Context) {
        let Some(context) = context.downcast_ref::<Context>() else {
            return;
        };
        if let Err(error) = self.upload_in(context) {
            Log::error(Event::Render, format!("Uploading drawable {} failed: {error}", self.base.name));
        }
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
        let Some(encoder) = render_pass.get_metal_encoder() else {
            debug_assert!(false);
            return;
        };
        let Some(shader_ptr) = self.base.shader.clone() else {
            Log::warning(
                Event::General,
                format!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name),
            );
            debug_assert!(false);
            return;
        };
        let Some(shader) = as_mtl_shader(&shader_ptr) else {
            return;
        };

        let formats = render_pass.get_attachment_formats();
        if self.impl_.attachment_formats != Some(formats) {
            self.impl_.pipeline_state = None;
            self.impl_.depth_stencil_state = None;
            self.impl_.attachment_formats = Some(formats);
        }

        render_pass.set_bytes(bytes_of(&self.base.ubo_index), ID_GLOBAL_UBO_INDEX);

        let empty = context.get_empty_vertex_buffer();
        Self::bind_attribute_bindings(&self.impl_.attribute_bindings, render_pass, empty.as_ref());
        Self::bind_attribute_bindings(&self.impl_.instance_bindings, render_pass, empty.as_ref());
        self.bind_textures(context, render_pass, shader);
        render_pass.bind_uniform_buffers(&self.impl_.uniform_buffers);

        let Some(indexes) = self.impl_.indexes.clone() else {
            debug_assert!(false, "Index buffer not uploaded");
            return;
        };
        let indexes = indexes.borrow();
        let index_buffer: Option<MTLBufferPtr> = indexes
            .get_buffer()
            .and_then(|buffer| context.get_backend().gpu().borrow().native_buffer(buffer.id()).map(Message::retain));
        let Some(index_buffer) = index_buffer.filter(|_| !indexes.get_dirty()) else {
            debug_assert!(false, "Index buffer not uploaded");
            return;
        };
        let Some(vertex_desc) = self.impl_.vertex_desc.clone() else {
            debug_assert!(false, "Vertex descriptor missing");
            return;
        };

        let cull_mode = self.base.cull_face_mode;
        render_pass.set_cull_mode(if cull_mode.enabled { map_cull_mode(cull_mode.side) } else { MTLCullMode::None });
        render_pass.set_front_facing_winding(map_winding_mode(cull_mode.winding));

        let scissor = parameters.scissor_rect();
        render_pass.set_scissor_rect(MTLScissorRect {
            x: scissor.x.max(0) as usize,
            y: scissor.y.max(0) as usize,
            width: scissor.width as usize,
            height: scissor.height as usize,
        });

        if self.impl_.pipeline_state.is_none() {
            let color_mode = self.base.color_mode;
            self.impl_.pipeline_state = shader.get_render_pipeline_state(
                formats,
                &vertex_desc,
                &color_mode,
                Some(hash(&[color_mode.hash(), self.impl_.vertex_desc_hash])),
            );
        }
        let Some(pipeline_state) = &self.impl_.pipeline_state else {
            debug_assert!(false, "Failed to create render pipeline state");
            return;
        };
        render_pass.set_render_pipeline_state(pipeline_state);

        // For 3D mode, stenciling is handled by the layer group.
        if self.base.is_3d {
            render_pass.set_depth_range(parameters.depth_mode_for_3d(DepthMaskType::ReadWrite).range);
        } else {
            let stencil_mode = match (self.base.stencil_mode, self.base.tile_id) {
                (Some(mode), _) => mode,
                (None, Some(tile_id)) if self.base.enable_stencil => {
                    parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped())
                }
                _ => StencilMode::disabled(),
            };
            let depth_mode = if self.base.enable_depth {
                parameters.depth_mode_for_sublayer(self.base.sub_layer_index as u8, self.base.depth_type)
            } else {
                DepthMode::disabled()
            };
            // The reference value is the encoder's, so a state serves every
            // mode that differs from its own in that value only.
            let state_mode = StencilMode { ref_: 0, ..stencil_mode };
            if self.impl_.depth_stencil_state.is_none() || self.impl_.stencil_state_mode != state_mode {
                self.impl_.depth_stencil_state =
                    context.make_depth_stencil_state(&depth_mode, &stencil_mode, render_pass);
                self.impl_.stencil_state_mode = state_mode;
            }
            render_pass.set_depth_stencil_state(self.impl_.depth_stencil_state.as_ref());
            render_pass.set_stencil_reference(stencil_mode.ref_);
            render_pass.set_depth_range(depth_mode.range);
        }

        let base_vertex_supported = context.get_backend().is_base_vertex_instance_drawing_supported();
        let mut draw_calls = 0;
        for segment in &self.impl_.segments {
            let ml_segment = segment.get_segment();
            if ml_segment.index_length == 0 {
                continue;
            }
            let primitive_type = primitive_type(segment.get_mode().type_).unwrap_or(MTLPrimitiveType::Point);
            // In bytes, not indexes.
            let index_offset = size_of::<u16>() * ml_segment.index_offset;
            debug_assert!(ml_segment.index_offset + ml_segment.index_length <= indexes.elements());
            // SAFETY: the segment's indexes are in the index buffer and
            // refer to vertices of the bound buffers.
            unsafe {
                if base_vertex_supported {
                    encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount_baseVertex_baseInstance(
                        primitive_type,
                        ml_segment.index_length,
                        MTLIndexType::UInt16,
                        &index_buffer,
                        index_offset,
                        ml_segment.instance_count,
                        ml_segment.vertex_offset as isize,
                        ml_segment.base_instance,
                    );
                } else {
                    encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount(
                        primitive_type,
                        ml_segment.index_length,
                        MTLIndexType::UInt16,
                        &index_buffer,
                        index_offset,
                        ml_segment.instance_count,
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
            let attrs =
                std::rc::Rc::clone(self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr));
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

    fn set_color_mode(&mut self, value: &ColorMode) {
        self.impl_.pipeline_state = None;
        self.base.color_mode = *value;
    }

    fn set_enable_stencil(&mut self, value: bool) {
        self.base.enable_stencil = value;
        self.impl_.depth_stencil_state = None;
    }

    fn set_enable_depth(&mut self, value: bool) {
        self.base.enable_depth = value;
        self.impl_.depth_stencil_state = None;
    }

    fn set_sub_layer_index(&mut self, value: i32) {
        self.base.sub_layer_index = value;
        self.impl_.depth_stencil_state = None;
    }

    fn set_depth_type(&mut self, value: DepthMaskType) {
        self.base.depth_type = value;
        self.impl_.depth_stencil_state = None;
    }

    fn set_shader(&mut self, value: Option<ShaderProgramBasePtr>) {
        if same_shader(&self.base.shader, &value) {
            return;
        }
        self.impl_.pipeline_state = None;
        self.base.shader = value;
    }

    fn set_cull_face_mode(&mut self, value: &CullFaceMode) {
        self.base.cull_face_mode = *value;
    }
}
