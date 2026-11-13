// Ports include/mln/gl/drawable_gl.hpp, src/mln/gl/drawable_gl_impl.hpp and
// src/mln/gl/drawable_gl.cpp
//
// Differences from C++:
// - Raw vertices become an attribute with a buffer of its own, as on the
//   other backends (see upload_pass.rs).
// - The vertex arrays are kept beside the segments (C++: in a segment
//   subclass), and are made again when the attribute bindings or the index
//   buffer change. An index buffer is told apart by a serial number, since
//   GL can give a new buffer the name of a deleted one.
// - The uniform blocks are bound by `Context::bind_uniform_blocks`, and the
//   textures to the units of their ids; neither is unbound after the draw.
// - No `stencilWriteRef`: the context draws the clip masks.
// - Instance attributes, which C++'s OpenGL drawables don't have: the
//   renderer draws fill extrusion walls instanced on every backend.

use std::sync::atomic::{AtomicU64, Ordering};

use glow::HasContext;
use mln_gfx::attribute::AttributeBindingArray;
use mln_gfx::context::Context as _;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::drawable::{self, DrawSegment, DrawableBase, UniqueDrawSegment};
use mln_gfx::gfx_types::{AttributeDataType, PrimitiveType};
use mln_gfx::index_vector::{IndexBufferBase, IndexVectorBasePtr};
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBase, ShaderProgramBasePtr, same_shader};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::BufferUsageType;
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
use mln_gfx::vertex_buffer::IndexBuffer as GfxIndexBuffer;
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use crate::context::Context;
use crate::enums;
use crate::shaders::shader_program::as_gl_shader;
use crate::texture2d::Texture2D;
use crate::upload_pass::UploadPass;
use crate::vertex_array::VertexArray;
use crate::vertex_buffer_resource::IndexBufferResource;

/// `struct IndexBufferGL : gfx::IndexBufferBase`: adapts `gfx::IndexBuffer`.
struct IndexBuffer {
    buffer: GfxIndexBuffer,
    /// Tells this buffer apart from one it replaced, whose GL name it may
    /// have.
    serial: u64,
}

impl IndexBuffer {
    fn new(buffer: GfxIndexBuffer) -> Self {
        static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);
        IndexBuffer { buffer, serial: NEXT_SERIAL.fetch_add(1, Ordering::Relaxed) }
    }
}

impl IndexBufferBase for IndexBuffer {}

/// `gl::DrawableGL::Impl`
struct DrawableImpl {
    segments: Vec<UniqueDrawSegment>,
    /// The vertex array of each segment (`DrawSegmentGL::vertexArray`).
    vertex_arrays: Vec<Option<VertexArray>>,
    /// The serial number of the index buffer the vertex arrays have.
    vertex_array_index_buffer: Option<u64>,
    indexes: Option<IndexVectorBasePtr>,
    vertex_count: usize,
    vertex_type: AttributeDataType,
    attribute_bindings: AttributeBindingArray,
    instance_bindings: AttributeBindingArray,
    /// Whether the bindings were built for the shader and attributes set.
    attribute_bindings_built: bool,
    uniform_buffers: UniformBufferArray,
    vertex_attr_id: usize,
}

pub struct Drawable {
    base: DrawableBase,
    impl_: DrawableImpl,
}

impl Drawable {
    #[must_use]
    pub fn new(name: String) -> Self {
        Drawable {
            base: DrawableBase::new(name),
            impl_: DrawableImpl {
                segments: Vec::new(),
                vertex_arrays: Vec::new(),
                vertex_array_index_buffer: None,
                indexes: None,
                vertex_count: 0,
                vertex_type: AttributeDataType::Invalid,
                attribute_bindings: Vec::new(),
                instance_bindings: Vec::new(),
                attribute_bindings_built: false,
                uniform_buffers: UniformBufferArray::new(),
                vertex_attr_id: 0,
            },
        }
    }

    /// `setVertexAttrId`
    pub fn set_vertex_attr_id(&mut self, id: usize) {
        self.impl_.vertex_attr_id = id;
    }

    /// `upload(gfx::UploadPass&)`
    pub fn upload(&mut self, upload_pass: &UploadPass) {
        const USAGE: BufferUsageType = BufferUsageType::StaticDraw;

        let Some(shader_ptr) = self.base.shader.clone() else {
            Log::warning(
                Event::General,
                format!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name),
            );
            debug_assert!(false);
            return;
        };
        let Some(shader) = as_gl_shader(&shader_ptr) else {
            return;
        };
        let context = upload_pass.gl_context();

        // Create an index buffer if necessary
        let Some(indexes) = self.impl_.indexes.clone() else {
            debug_assert!(false, "Missing index data");
            return;
        };
        let index_buffer = {
            let mut indexes = indexes.borrow_mut();
            if indexes.get_buffer().is_none() || indexes.get_dirty() {
                if indexes.empty() {
                    debug_assert!(false, "Missing index data");
                    return;
                }
                let resource = upload_pass.create_index_buffer_of(&indexes);
                let buffer = GfxIndexBuffer::new(indexes.elements(), resource);
                indexes.set_buffer(Some(Box::new(IndexBuffer::new(buffer))));
                indexes.set_dirty(false);
            }
            indexes.get_buffer().and_then(|b| b.downcast_ref::<IndexBuffer>()).and_then(|b| {
                let buffer = b.buffer.get_resource::<IndexBufferResource>()?.get_buffer()?;
                Some((buffer, b.serial))
            })
        };
        let Some((index_buffer, index_buffer_serial)) = index_buffer else {
            return;
        };

        // Build the vertex attributes and bindings, if necessary
        let build_vertex_attribs = !self.impl_.attribute_bindings_built
            || match (&self.base.vertex_attributes, self.base.attribute_update_time) {
                (Some(attrs), Some(t)) => attrs.borrow().is_modified_after(t),
                _ => true,
            };
        if build_vertex_attribs {
            let attrs = self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr).clone();
            // Apply drawable values to shader defaults
            let attribute_bindings = mln_gfx::upload_pass::UploadPass::build_attribute_bindings(
                upload_pass,
                self.impl_.vertex_count,
                self.impl_.vertex_type,
                /*vertex_attribute_index=*/ usize::MAX,
                /*vertex_data=*/ &[],
                shader.get_vertex_attributes(),
                &mut attrs.borrow_mut(),
                USAGE,
                self.base.attribute_update_time,
            );
            attrs.borrow().visit_attributes(|attrib| attrib.set_dirty(false));
            if self.impl_.attribute_bindings != attribute_bindings {
                self.impl_.attribute_bindings = attribute_bindings;
                self.impl_.vertex_arrays.clear();
            }
            self.impl_.attribute_bindings_built = true;
        }

        // Build the instance attribute bindings, if any
        let build_instance_attribs = match (&self.base.instance_attributes, self.base.attribute_update_time) {
            (Some(attrs), Some(t)) => build_vertex_attribs || attrs.borrow().is_modified_after(t),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if build_instance_attribs && let Some(attrs) = self.base.instance_attributes.clone() {
            let count = attrs.borrow().get_min_count();
            let instance_bindings = mln_gfx::upload_pass::UploadPass::build_attribute_bindings(
                upload_pass,
                count,
                AttributeDataType::Byte,
                /*vertex_attribute_index=*/ usize::MAX,
                /*vertex_data=*/ &[],
                shader.get_instance_attributes(),
                &mut attrs.borrow_mut(),
                USAGE,
                self.base.attribute_update_time,
            );
            attrs.borrow().visit_attributes(|attrib| attrib.set_dirty(false));
            if self.impl_.instance_bindings != instance_bindings {
                self.impl_.instance_bindings = instance_bindings;
                self.impl_.vertex_arrays.clear();
            }
        }

        // Bind a VAO for each group of vertexes described by a segment
        if self.impl_.vertex_array_index_buffer != Some(index_buffer_serial) {
            self.impl_.vertex_arrays.clear();
            self.impl_.vertex_array_index_buffer = Some(index_buffer_serial);
        }
        self.impl_.vertex_arrays.resize_with(self.impl_.segments.len(), || None);
        for (segment, vertex_array) in self.impl_.segments.iter().zip(&mut self.impl_.vertex_arrays) {
            let segment = segment.get_segment();
            if segment.index_length == 0 || vertex_array.is_some() {
                continue;
            }
            *vertex_array = VertexArray::new(
                context,
                index_buffer,
                &self.impl_.attribute_bindings,
                segment.vertex_offset,
                &self.impl_.instance_bindings,
                segment.base_instance,
            );
            debug_assert!(vertex_array.is_some());
        }

        let textures_need_upload = self.base.textures.iter().flatten().any(|t| t.borrow().needs_upload());
        if textures_need_upload {
            for texture in self.base.textures.iter().flatten() {
                texture.borrow_mut().upload();
            }
        }

        self.base.attribute_update_time = Some(Instant::now());
    }

    /// `makeStencilMode(parameters)`
    fn make_stencil_mode(&self, parameters: &dyn PaintParameters) -> StencilMode {
        if self.base.enable_stencil
            && !self.base.is_3d
            && let Some(tile_id) = self.base.tile_id
        {
            return parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped());
        }
        StencilMode::disabled()
    }

    /// `bindTextures()`: each texture to the unit its sampler reads.
    fn bind_textures(&self, context: &Context, shader: &dyn ShaderProgramBase) {
        for (id, texture) in self.base.textures.iter().enumerate() {
            let Some(texture) = texture else { continue };
            let Some(unit) = shader.get_sampler_location(id) else {
                continue;
            };
            let mut texture = texture.borrow_mut();
            if let Some(texture) = texture.downcast_mut::<Texture2D>() {
                texture.bind(context, unit as u32);
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
        let Some(shader_ptr) = self.base.shader.clone() else {
            Log::warning(
                Event::General,
                format!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name),
            );
            debug_assert!(false);
            return;
        };
        let Some(shader) = as_gl_shader(&shader_ptr) else {
            return;
        };
        let gl = context.gl();
        context.state().use_program(gl, Some(shader.get_gl_program_id()));

        if self.base.enable_depth {
            context.set_depth_mode(&if self.base.is_3d {
                parameters.depth_mode_for_3d(self.base.depth_type)
            } else {
                parameters.depth_mode_for_sublayer(self.base.sub_layer_index as u8, self.base.depth_type)
            });
        } else {
            context.set_depth_mode(&DepthMode::disabled());
        }

        // For 3D mode, stenciling is handled by the layer group
        if !self.base.is_3d {
            context.set_stencil_mode(&self.make_stencil_mode(parameters));
        }

        context.set_color_mode(&self.base.color_mode);
        context.set_cull_face_mode(&self.base.cull_face_mode);
        context.set_scissor_test(&parameters.scissor_rect());

        context.bind_uniform_blocks(shader, &self.impl_.uniform_buffers, self.base.ubo_index);
        self.bind_textures(context, shader);

        let mut draw_calls = 0;
        for (segment, vertex_array) in self.impl_.segments.iter().zip(&self.impl_.vertex_arrays) {
            let ml_segment = segment.get_segment();
            let Some(vertex_array) = vertex_array else {
                continue;
            };
            if ml_segment.index_length == 0 {
                continue;
            }
            let mode = segment.get_mode();
            if mode.primitive_type() == PrimitiveType::Line {
                context.state().set_line_width(gl, mode.size);
            }
            context.state().bind_vertex_array(gl, Some(vertex_array.get_id()));
            let index_offset = (ml_segment.index_offset * std::mem::size_of::<u16>()) as i32;
            // SAFETY: the vertex array has the index buffer, which holds the
            // segment's indexes, and they refer to vertices of its buffers,
            // which have the segment's instances.
            unsafe {
                if self.impl_.instance_bindings.is_empty() {
                    gl.draw_elements(
                        enums::draw_mode(mode.type_),
                        ml_segment.index_length as i32,
                        glow::UNSIGNED_SHORT,
                        index_offset,
                    );
                } else {
                    gl.draw_elements_instanced(
                        enums::draw_mode(mode.type_),
                        ml_segment.index_length as i32,
                        glow::UNSIGNED_SHORT,
                        index_offset,
                        ml_segment.instance_count as i32,
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
        self.impl_.vertex_arrays.clear();
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
        self.impl_.vertex_arrays.clear();
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.impl_.uniform_buffers
    }

    fn set_shader(&mut self, value: Option<ShaderProgramBasePtr>) {
        if same_shader(&self.base.shader, &value) {
            return;
        }
        // Another program has its attributes at locations of its own.
        self.impl_.attribute_bindings_built = false;
        self.base.shader = value;
    }
}
