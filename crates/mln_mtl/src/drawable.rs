// Differences from C++:
// - The bindings' buffer indexes come from the shader (`upload`; see
//   upload_pass.rs).
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
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::index_vector::{IndexBufferBase, IndexVectorBasePtr};
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBasePtr, same_shader};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::{CullFaceSideType, CullFaceWindingType, DepthMaskType};
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_gfx::util_hash::{hash, hash_combine};
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
use mln_gfx::vertex_buffer::IndexBuffer as GfxIndexBuffer;
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;
use objc2_metal::{
    MTLCullMode, MTLIndexType, MTLPrimitiveType, MTLRenderCommandEncoder, MTLScissorRect, MTLVertexDescriptor,
    MTLVertexFormat, MTLVertexStepFunction, MTLWinding,
};

use crate::context::Context;
use crate::mtl_fwd::{MTLBufferPtr, MTLDepthStencilStatePtr, MTLRenderPipelineStatePtr, MTLVertexDescriptorPtr};
use crate::render_pass::{AttachmentFormats, RenderPass};
use crate::shaders::shader_program::{ShaderProgram, as_mtl_shader};
use crate::texture2d::Texture2D;
use crate::uniform_buffer::bind_uniform_buffer_array;
use crate::upload_pass::UploadPass;
use crate::vertex_buffer_resource::{IndexBufferResource, VertexBufferResource};

/// The uploaded buffer of an index vector.
struct IndexBuffer {
    buffer: GfxIndexBuffer,
}

impl IndexBufferBase for IndexBuffer {}

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
    previous_stencil_mode: StencilMode,
}

pub(crate) struct Drawable {
    base: DrawableBase,
    impl_: DrawableImpl,
}

fn get_primitive_type(type_: DrawModeType) -> MTLPrimitiveType {
    match type_ {
        DrawModeType::Points => MTLPrimitiveType::Point,
        DrawModeType::Lines => MTLPrimitiveType::Line,
        DrawModeType::LineStrip => MTLPrimitiveType::LineStrip,
        DrawModeType::Triangles => MTLPrimitiveType::Triangle,
        _ => {
            debug_assert!(false, "unsupported draw mode");
            MTLPrimitiveType::Point
        }
    }
}

fn map_cull_mode(side: CullFaceSideType) -> MTLCullMode {
    match side {
        CullFaceSideType::Front => MTLCullMode::Front,
        CullFaceSideType::Back => MTLCullMode::Back,
        CullFaceSideType::FrontAndBack => MTLCullMode::None,
    }
}

fn map_winding_mode(winding: CullFaceWindingType) -> MTLWinding {
    match winding {
        CullFaceWindingType::Clockwise => MTLWinding::Clockwise,
        CullFaceWindingType::CounterClockwise => MTLWinding::CounterClockwise,
    }
}

fn mtl_vertex_type_of(type_: AttributeDataType) -> MTLVertexFormat {
    use AttributeDataType as A;
    use MTLVertexFormat as F;
    match type_ {
        A::Byte => F::Char,
        A::Byte2 => F::Char2,
        A::Byte3 => F::Char3,
        A::Byte4 => F::Char4,
        A::UByte => F::UChar,
        A::UByte2 => F::UChar2,
        A::UByte3 => F::UChar3,
        A::UByte4 => F::UChar4,
        A::UByte4Normalized => F::UChar4Normalized,
        A::Short => F::Short,
        A::Short2 => F::Short2,
        A::Short3 => F::Short3,
        A::Short4 => F::Short4,
        A::UShort => F::UShort,
        A::UShort2 => F::UShort2,
        A::UShort3 => F::UShort3,
        A::UShort4 => F::UShort4,
        A::Int => F::Int,
        A::Int2 => F::Int2,
        A::Int3 => F::Int3,
        A::Int4 => F::Int4,
        A::UInt => F::UInt,
        A::UInt2 => F::UInt2,
        A::UInt3 => F::UInt3,
        A::UInt4 => F::UInt4,
        A::Float => F::Float,
        A::Float2 => F::Float2,
        A::Float3 => F::Float3,
        A::Float4 => F::Float4,
        _ => {
            debug_assert!(false, "Unsupported vertex attribute format");
            F::Invalid
        }
    }
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
                previous_stencil_mode: StencilMode::default(),
            },
        }
    }

    pub(crate) fn set_vertex_attr_id(&mut self, id: usize) {
        self.impl_.vertex_attr_id = id;
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
        let Some(shader) = as_mtl_shader(&shader_ptr) else {
            return;
        };
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
                // Create or update a buffer for the index data. We don't
                // update any existing buffer because it may still be in use
                // by the previous frame.
                let resource = upload_pass.create_index_buffer_of(&indexes);
                let buffer = GfxIndexBuffer::new(resource);
                indexes.set_buffer(Some(Box::new(IndexBuffer { buffer })));
                indexes.set_dirty(false);
            }
        }

        let build_vertex_attribs = self.impl_.vertex_desc.is_none()
            || match (&self.base.vertex_attributes, self.base.attribute_update_time) {
                (Some(attrs), Some(t)) => attrs.borrow().is_modified_after(t),
                _ => true,
            };

        if build_vertex_attribs {
            let attrs =
                std::rc::Rc::clone(self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr));
            // Apply drawable values to shader defaults.
            let mut attribute_bindings = mln_gfx::upload_pass::UploadPass::build_attribute_bindings(
                upload_pass,
                self.impl_.vertex_count,
                self.impl_.vertex_type,
                /*vertex_attribute_index=*/ usize::MAX,
                /*vertex_data=*/ &[],
                mln_gfx::shaders::shader_program_base::ShaderProgramBase::get_vertex_attributes(shader),
                &mut attrs.borrow_mut(),
                mln_gfx::types::BufferUsageType::StaticDraw,
                self.base.attribute_update_time,
            );
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
            let count = attrs.borrow().get_min_count();
            let mut instance_bindings = mln_gfx::upload_pass::UploadPass::build_attribute_bindings(
                upload_pass,
                count,
                AttributeDataType::Byte,
                usize::MAX,
                &[],
                mln_gfx::shaders::shader_program_base::ShaderProgramBase::get_instance_attributes(shader),
                &mut attrs.borrow_mut(),
                mln_gfx::types::BufferUsageType::StaticDraw,
                self.base.attribute_update_time,
            );
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
                    let has_buffer = binding.vertex_buffer_resource.is_some();
                    let buffer_index = binding.buffer_index as usize;
                    // SAFETY: the attribute and buffer indexes are below
                    // Metal's 31.
                    unsafe {
                        let attrib_desc = vertex_desc.attributes().objectAtIndexedSubscript(index);
                        attrib_desc.setBufferIndex(buffer_index);
                        attrib_desc.setOffset(binding.attribute.offset as usize);
                        attrib_desc.setFormat(mtl_vertex_type_of(binding.attribute.data_type));

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

        let textures_need_upload = self.base.textures.iter().flatten().any(|t| t.borrow().needs_upload());
        if textures_need_upload {
            for texture in self.base.textures.iter().flatten() {
                texture.borrow_mut().upload();
            }
        }

        self.base.attribute_update_time = Some(Instant::now());
    }

    fn bind_attribute_bindings(
        bindings: &AttributeBindingArray,
        render_pass: &RenderPass,
        empty: Option<&MTLBufferPtr>,
    ) {
        for binding in bindings.iter().flatten() {
            match binding.vertex_buffer_resource.as_ref().and_then(|r| r.downcast_ref::<VertexBufferResource>()) {
                Some(buffer) => render_pass.bind_vertex(buffer.get(), 0, binding.buffer_index as usize, 0),
                None => {
                    if let Some(empty) = empty {
                        render_pass.set_vertex_buffer(empty, 0, binding.buffer_index as usize);
                    }
                }
            }
        }
    }

    fn bind_textures(&self, render_pass: &RenderPass, shader: &ShaderProgram) {
        for (id, texture) in self.base.textures.iter().enumerate() {
            let Some(texture) = texture else { continue };
            let Some(location) =
                mln_gfx::shaders::shader_program_base::ShaderProgramBase::get_sampler_location(shader, id)
            else {
                continue;
            };
            let mut texture = texture.borrow_mut();
            if let Some(texture) = texture.downcast_mut::<Texture2D>() {
                texture.bind(render_pass, location);
            }
        }
    }

    fn unbind_textures(&self, render_pass: &RenderPass, shader: &ShaderProgram) {
        for (id, texture) in self.base.textures.iter().enumerate() {
            let Some(texture) = texture else { continue };
            let Some(location) =
                mln_gfx::shaders::shader_program_base::ShaderProgramBase::get_sampler_location(shader, id)
            else {
                continue;
            };
            let texture = texture.borrow();
            if let Some(texture) = texture.downcast_ref::<Texture2D>() {
                texture.unbind(render_pass, location);
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
        self.bind_textures(render_pass, shader);
        bind_uniform_buffer_array(&self.impl_.uniform_buffers, render_pass);

        let Some(indexes) = self.impl_.indexes.clone() else {
            debug_assert!(false, "Index buffer not uploaded");
            return;
        };
        let indexes = indexes.borrow();
        let index_buffer: Option<MTLBufferPtr> = indexes
            .get_buffer()
            .and_then(|b| b.downcast_ref::<IndexBuffer>())
            .and_then(|b| b.buffer.get_resource::<IndexBufferResource>())
            .and_then(|r| r.get().get_metal_buffer());
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
            let mut new_stencil_mode = None;
            let tile_id = self.base.tile_id;

            // If we have a stencil state, we may be able to reuse it, if the
            // tile masks haven't changed. We assume that only the reference
            // value needs to be compared.
            if self.impl_.depth_stencil_state.is_some()
                && self.base.enable_stencil
                && let Some(tile_id) = tile_id
            {
                let mode = parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped());
                if mode.ref_ != self.impl_.previous_stencil_mode.ref_ {
                    // No, need to rebuild it.
                    self.impl_.depth_stencil_state = None;
                }
                new_stencil_mode = Some(mode);
            }
            let depth_mode = if self.base.enable_depth {
                parameters.depth_mode_for_sublayer(self.base.sub_layer_index as u8, self.base.depth_type)
            } else {
                DepthMode::disabled()
            };
            if self.impl_.depth_stencil_state.is_none() {
                let stencil_mode = match tile_id {
                    Some(tile_id) if self.base.enable_stencil => {
                        let mode = parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped());
                        new_stencil_mode = Some(mode);
                        mode
                    }
                    _ => StencilMode::disabled(),
                };
                self.impl_.depth_stencil_state =
                    context.make_depth_stencil_state(&depth_mode, &stencil_mode, render_pass);
                if let Some(mode) = new_stencil_mode {
                    self.impl_.previous_stencil_mode = mode;
                }
            }
            render_pass.set_depth_stencil_state(self.impl_.depth_stencil_state.as_ref());
            render_pass.set_stencil_reference(self.impl_.previous_stencil_mode.ref_);
            render_pass.set_depth_range(depth_mode.range);
        }

        let base_vertex_supported = context.get_backend().is_base_vertex_instance_drawing_supported();
        let mut draw_calls = 0;
        for segment in &self.impl_.segments {
            let ml_segment = segment.get_segment();
            if ml_segment.index_length == 0 {
                continue;
            }
            let primitive_type = get_primitive_type(segment.get_mode().type_);
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

        self.unbind_textures(render_pass, shader);
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
