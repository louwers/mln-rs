//! Layers drawn as a whole: a layer with `fill-layer-opacity` or
//! `line-layer-opacity` below 1 draws into a texture of the frame's size,
//! which is then composited onto the frame with that opacity. Its features
//! overlap within the texture as they would on the frame, so the opacity
//! applies once to the layer rather than to each feature.

use std::rc::Rc;

use mln_gfx::buffer::BufferPtr;
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Draw, DynamicState, FixedState, GeometryDesc, GeometryId, Gpu, GpuError, LoadOp,
    PipelineKey, SamplerId, Scissor, ShaderId, StencilState, TargetFormats, TextureFormat, TextureId, VertexAttribute,
    VertexBufferLayout, VertexStep,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_composite_ubo::LayerCompositeUBO;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE, ID_LAYER_COMPOSITE_UBO,
    ID_PROJECTION_UBO, MAX_UBO_COUNT_PER_SHADER,
};
use mln_gfx::texture2d::texture_format;
use mln_gfx::types::{TextureChannelDataType, TexturePixelType};
use mln_gfx::{
    AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawModeType, ShaderProgramPtr, ShaderRegistry,
    StencilMode,
};
use mln_util::Color;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::drawable::clamped_scissor;
use crate::gfx::attachment::Attachment;
use crate::gfx::context::Context;
use crate::gfx::owned::Owned;
use crate::gfx::texture2d::Texture2D;
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::paint_parameters::{OpenPass, PaintParameters};
use crate::render_static_data::RenderStaticData;

/// The format of the texture a composited layer draws into: its colors are
/// premultiplied, as the frame's are.
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
/// The format of its depth and stencil, which the layer's tile clipping and
/// depth tests read as they would the frame's.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The vertex layout of the quad that covers the frame: a corner of the unit
/// square in two shorts.
fn layout() -> VertexBufferLayout {
    VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute {
            id: ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE as u32,
            format: AttributeDataType::Short2,
            offset: 0,
        }],
    }
}

/// The textures a composited layer draws into, of one size.
struct Target {
    size: Size,
    /// The color texture, which `texture` and `sampler` bind.
    _color: Texture2D,
    texture: TextureId,
    sampler: SamplerId,
    depth_stencil: Attachment,
}

/// The quad that covers the frame.
struct Quad {
    vertices: BufferPtr,
    indices: BufferPtr,
    geometry: Option<Owned<GeometryId>>,
}

/// The texture that the frame's composited layers draw into, one after
/// another, and the draw that composites one onto the frame.
#[derive(Default)]
pub(crate) struct LayerComposite {
    shader: Option<ShaderProgramPtr>,
    shader_id: Option<ShaderId>,
    target: Option<Target>,
    quad: Option<Quad>,
    /// The layer groups composited in the frame, by id, in the order of their
    /// slots of `uniforms`.
    groups: Vec<SimpleIdentity>,
    /// Each group's `LayerCompositeUBO`, at an offset the GPU can bind: one
    /// write a frame.
    uniforms: Option<UniformBufferPtr>,
    /// The bytes of `uniforms`, kept between frames.
    bytes: Vec<u8>,
    /// A projection the shader doesn't read, for the block that OpenGL's
    /// prelude declares, which WebGL2 rejects a draw without a buffer for.
    projection: Option<UniformBufferPtr>,
    /// The bindings of each slot of `uniforms`, and what they were made of:
    /// the epoch of the frame's buffers, the uniforms' buffer and the
    /// target's texture.
    bindings: Vec<Owned<BindingsId>>,
    bindings_made: Option<(u64, BufferId, TextureId)>,
}

impl LayerComposite {
    /// Makes what the frame composites `groups` with, each a layer group and
    /// its opacity, onto a frame of `size`, and drops it where there are
    /// none.
    pub(crate) fn update(
        &mut self,
        context: &Context,
        shaders: &ShaderRegistry,
        size: Size,
        groups: &[(SimpleIdentity, f32)],
    ) {
        self.groups.clear();
        if groups.is_empty() || size.is_empty() {
            self.target = None;
            self.bindings.clear();
            self.bindings_made = None;
            return;
        }
        if self.shader.is_none() {
            self.shader = shaders.shader("LayerCompositeShader", ProjectionVariant::Mercator);
        }
        if self.shader.is_none() {
            return;
        }
        if let Err(error) = self.make(context, size) {
            Log::warning(Event::Render, format!("Making the layer composite's textures failed: {error}"));
            self.target = None;
            return;
        }

        let size = size_of::<LayerCompositeUBO>();
        let stride = size.next_multiple_of(context.caps().buffer_offset_alignment as usize);
        self.bytes.clear();
        self.bytes.resize(groups.len() * stride, 0);
        for (slot, &(group, opacity)) in groups.iter().enumerate() {
            let ubo = LayerCompositeUBO { opacity, ..LayerCompositeUBO::default() };
            self.bytes[slot * stride..][..size].copy_from_slice(bytemuck::bytes_of(&ubo));
            self.groups.push(group);
        }
        context.write_uniform_buffer(&mut self.uniforms, &self.bytes);
        if self.projection.is_none() {
            context.write_uniform_buffer_pod(&mut self.projection, &ProjectionUBO::default());
        }
    }

    /// Makes the target of `size`, unless it is made, and the quad.
    fn make(&mut self, context: &Context, size: Size) -> Result<(), GpuError> {
        if self.target.as_ref().is_none_or(|target| target.size != size) {
            self.target = None;
            self.bindings.clear();
            self.bindings_made = None;
            let mut color = Texture2D::new(Rc::clone(context.released()));
            color.set_size(size);
            color.set_format(texture_format(TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte));
            color.set_render_target();
            let (texture, sampler) = color.binding(&mut *context.gpu(), context)?;
            let depth_stencil = Attachment::new(context, "layer composite depth and stencil", size, DEPTH_STENCIL)?;
            self.target = Some(Target { size, _color: color, texture, sampler, depth_stencil });
        }
        if self.quad.is_none() {
            let vertices: [[i16; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];
            let indices = RenderStaticData::quad_triangle_indices();
            self.quad = Some(Quad {
                vertices: context.buffer(BufferUsage::Vertex, bytemuck::cast_slice(&vertices))?,
                indices: context.buffer(BufferUsage::Index, bytemuck::cast_slice(&indices))?,
                geometry: None,
            });
        }
        Ok(())
    }

    /// Whether the layer group `group` draws into the texture in this frame.
    #[must_use]
    pub(crate) fn composites(&self, group: SimpleIdentity) -> bool {
        self.target.is_some() && self.groups.contains(&group)
    }

    /// The pass that draws a layer into the texture, cleared to transparent,
    /// with its depth cleared to 1 and its stencil to 0. `None` without a
    /// target.
    pub(crate) fn pass(&self) -> Option<OpenPass> {
        let target = self.target.as_ref()?;
        Some(OpenPass {
            label: "layer composite",
            color: Some(ColorAttachment {
                texture: target.texture,
                load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0)),
            }),
            depth_stencil: Some(DepthStencilAttachment {
                texture: target.depth_stencil.id(),
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Clear(0),
            }),
            size: target.size,
            targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
        })
    }

    /// Composites the texture onto the open pass, with the opacity of the
    /// layer group `group`, at the depth of the current layer: layers above
    /// that drew into the depth buffer hide it.
    pub(crate) fn draw(&mut self, gpu: &mut dyn Gpu, parameters: &PaintParameters<'_>, group: SimpleIdentity) {
        if let Err(error) = self.draw_in(gpu, parameters, group) {
            Log::warning(Event::Render, format!("Compositing a layer failed: {error}"));
        }
    }

    fn draw_in(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &PaintParameters<'_>,
        group: SimpleIdentity,
    ) -> Result<(), GpuError> {
        let (Some(shader), Some(target), Some(quad), Some(uniforms), Some(projection)) =
            (&self.shader, &self.target, &mut self.quad, &self.uniforms, &self.projection)
        else {
            return Ok(());
        };
        let Some(slot) = self.groups.iter().position(|&id| id == group) else {
            return Ok(());
        };
        let Some((size, targets)) = parameters.pass_target() else {
            return Ok(());
        };
        let context = parameters.context;
        let shader_id = match self.shader_id {
            Some(id) => id,
            None => *self.shader_id.insert(context.shader(gpu, shader.key())?),
        };

        let (texture, sampler) = (target.texture, target.sampler);
        let made = (context.global_uniform_buffers().epoch(), uniforms.id(), texture);
        if self.bindings_made != Some(made) || self.bindings.len() < self.groups.len() {
            self.bindings.clear();
            self.bindings_made = None;
            let global = context.global_uniform_buffers();
            let size = size_of::<LayerCompositeUBO>();
            let stride = size.next_multiple_of(context.caps().buffer_offset_alignment as usize);
            for slot in 0..uniforms.size() / stride {
                let mut entries: Vec<BindGroupEntry> = (0..MAX_UBO_COUNT_PER_SHADER)
                    .filter_map(|id| {
                        let (buffer, offset, size) = match id {
                            ID_LAYER_COMPOSITE_UBO => (uniforms, (slot * stride) as u64, size as u64),
                            ID_PROJECTION_UBO => (projection, 0, projection.size() as u64),
                            _ => global.get(id).map(|buffer| (buffer, 0, buffer.size() as u64))?,
                        };
                        Some(BindGroupEntry::Buffer { id: id as u32, buffer: buffer.id(), offset, size })
                    })
                    .collect();
                entries.push(BindGroupEntry::Texture { id: ID_LAYER_COMPOSITE_IMAGE_TEXTURE as u32, texture, sampler });
                let bindings = gpu.create_bindings(&BindingsDesc {
                    label: "layer composite",
                    shader: shader_id,
                    entries: &entries,
                })?;
                self.bindings.push(Owned::new(bindings, context.released()));
            }
            self.bindings_made = Some(made);
        }

        let geometry = if let Some(geometry) = &quad.geometry {
            geometry.id()
        } else {
            let geometry = gpu.create_geometry(&GeometryDesc {
                shader: shader_id,
                layouts: &[layout()],
                vertex_buffers: &[(quad.vertices.id(), 0)],
                index_buffer: quad.indices.id(),
            })?;
            quad.geometry = Some(Owned::new(geometry, context.released()));
            geometry
        };

        let depth = parameters.depth_mode_for_sublayer(0, DepthMaskType::ReadOnly);
        let color = ColorMode::alpha_blended();
        let pipeline = context.pipeline(
            gpu,
            &PipelineKey {
                shader: shader_id,
                vertex_buffers: vec![layout()],
                state: FixedState {
                    primitive: DrawModeType::Triangles,
                    cull: CullFaceMode::disabled(),
                    blend: color.blend_function,
                    color_mask: color.mask,
                    depth: DepthState::from(&depth),
                    stencil: StencilState::from(&StencilMode::disabled()),
                },
                targets,
            },
        )?;
        let full = Scissor { x: 0, y: 0, width: size.width, height: size.height };
        gpu.draw(&Draw {
            pipeline,
            bindings: self.bindings[slot].id(),
            geometry,
            first_index: 0,
            index_count: 6,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
            global_index: 0,
            dynamic: DynamicState {
                depth_range: (depth.range.min, depth.range.max),
                scissor: clamped_scissor(parameters, size).unwrap_or(full),
                stencil_reference: 0,
                blend_constant: color.blend_color,
                line_width: 1.0,
            },
        })?;
        let mut stats = context.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        Ok(())
    }
}
