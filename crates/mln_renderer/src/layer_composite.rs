//! Layers drawn as a whole: a layer with `fill-layer-opacity` or
//! `line-layer-opacity` below 1, or with a `*-layer-blend` other than
//! `normal`, draws into a texture of the frame's size, which is then
//! composited onto the frame with that opacity and blend. Its features
//! overlap within the texture as they would on the frame, so the opacity and
//! blend apply once to the layer rather than to each feature.
//!
//! `screen` and `plus-lighter` are blend functions. The other blends are
//! computed in the shader against the backdrop, which a pass can't sample
//! while it draws into it: a frame with such a layer draws into a texture of
//! its own, copied into a second texture before each such layer and onto the
//! frame's target at the end.

use mln_gfx::buffer::BufferPtr;
use mln_gfx::color_mode::{BlendFunction, LinearBlend};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Draw, DynamicState, FixedState, GeometryDesc, GeometryId, Gpu, GpuError, LoadOp,
    PipelineKey, SamplerId, Scissor, ShaderId, StencilState, TargetFormats, TextureFormat, TextureId, VertexAttribute,
    VertexBufferLayout, VertexStep,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_composite_ubo::{
    LAYER_COMPOSITE_BLEND_COLOR, LAYER_COMPOSITE_BLEND_LUMINOSITY, LAYER_COMPOSITE_BLEND_MULTIPLY,
    LAYER_COMPOSITE_BLEND_NONE, LAYER_COMPOSITE_BLEND_OVERLAY, LayerCompositeUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, ID_LAYER_COMPOSITE_IMAGE_TEXTURE, ID_LAYER_COMPOSITE_POS_VERTEX_ATTRIBUTE,
    ID_LAYER_COMPOSITE_UBO, ID_PROJECTION_UBO, MAX_UBO_COUNT_PER_SHADER,
};
use mln_gfx::types::ColorBlendFactorType;
use mln_gfx::{
    AttributeDataType, ColorMode, CullFaceMode, DepthMode, DrawModeType, ShaderProgramPtr, ShaderRegistry, StencilMode,
};
use mln_style_types::LayerBlendType;
use mln_util::Color;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gfx::attachment::Attachment;
use crate::gfx::context::Context;
use crate::gfx::owned::Owned;
use crate::gfx::store::SharedTexture;
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::paint_parameters::{DrawTarget, OpenPass};
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

/// How a draw group's draws composite onto the frame, after they draw
/// into a texture of their own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Composite {
    pub(crate) opacity: f32,
    pub(crate) blend: LayerBlendType,
}

impl Composite {
    /// How a layer with the layer opacity `opacity` and the layer blend
    /// `blend` composites: `None` for an opaque layer of the `normal` blend,
    /// which draws straight onto the frame.
    #[must_use]
    pub(crate) fn of(opacity: f32, blend: LayerBlendType) -> Option<Composite> {
        (opacity < 1.0 || blend != LayerBlendType::Normal).then_some(Composite { opacity, blend })
    }

    /// The blend the shader computes: none for the blends a blend function
    /// expresses.
    fn shader_blend(self) -> i32 {
        match self.blend {
            LayerBlendType::Normal | LayerBlendType::Screen | LayerBlendType::PlusLighter => LAYER_COMPOSITE_BLEND_NONE,
            LayerBlendType::Multiply => LAYER_COMPOSITE_BLEND_MULTIPLY,
            LayerBlendType::Overlay => LAYER_COMPOSITE_BLEND_OVERLAY,
            LayerBlendType::Color => LAYER_COMPOSITE_BLEND_COLOR,
            LayerBlendType::Luminosity => LAYER_COMPOSITE_BLEND_LUMINOSITY,
        }
    }

    /// Whether the shader blends with the backdrop, a copy of the frame.
    fn reads_backdrop(self) -> bool {
        self.shader_blend() != LAYER_COMPOSITE_BLEND_NONE
    }

    /// The blend function that composites the shader's color: on premultiplied
    /// colors, and a replacement where the shader blends with the backdrop.
    fn blend_function(self) -> BlendFunction {
        let add = |dst_factor| BlendFunction::Add(LinearBlend { src_factor: ColorBlendFactorType::One, dst_factor });
        match self.blend {
            LayerBlendType::Normal => add(ColorBlendFactorType::OneMinusSrcAlpha),
            LayerBlendType::Screen => add(ColorBlendFactorType::OneMinusSrcColor),
            LayerBlendType::PlusLighter => add(ColorBlendFactorType::One),
            LayerBlendType::Multiply | LayerBlendType::Overlay | LayerBlendType::Color | LayerBlendType::Luminosity => {
                BlendFunction::Replace
            }
        }
    }
}

/// A texture that passes draw into and draws sample.
struct Texture {
    /// The texture, which `texture` and `sampler` bind.
    _texture: SharedTexture,
    texture: TextureId,
    sampler: SamplerId,
}

impl Texture {
    fn new(context: &mut Context, size: Size, format: TextureFormat) -> Result<Texture, GpuError> {
        let owned = context.add_texture_with(|texture| {
            texture.set_size(size);
            texture.set_format(format);
            texture.set_render_target();
        });
        let (texture, sampler) = context.bind_texture(&owned)?;
        Ok(Texture { _texture: owned, texture, sampler })
    }
}

/// The textures a composited layer draws into, of one size.
struct Target {
    size: Size,
    color: Texture,
    depth_stencil: Attachment,
}

/// The textures of a frame with a layer that blends in the shader: the
/// frame draws into `frame`, which is copied into `backdrop` for each such
/// layer and onto the frame's target at the end.
struct FrameCopy {
    format: TextureFormat,
    frame: Texture,
    backdrop: Texture,
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
    frame: Option<FrameCopy>,
    quad: Option<Quad>,
    /// The draw groups composited in the frame, by id, in the order of their
    /// slots of `uniforms`.
    groups: Vec<(SimpleIdentity, Composite)>,
    /// Each group's `LayerCompositeUBO`, at an offset the GPU can bind, and
    /// then the copy's: one write a frame.
    uniforms: Option<UniformBufferPtr>,
    /// The bytes of `uniforms`, kept between frames.
    bytes: Vec<u8>,
    /// A projection the shader doesn't read, for the block that OpenGL's
    /// prelude declares, which WebGL2 rejects a draw without a buffer for.
    projection: Option<UniformBufferPtr>,
    /// The bindings of each slot of `uniforms`, and what they were made of:
    /// the epoch of the frame's buffers, the uniforms' buffer, the textures
    /// and the number of slots, the last of which is the copy's.
    bindings: Vec<Owned<BindingsId>>,
    bindings_made: Option<(u64, BufferId, TextureId, Option<TextureId>, usize)>,
}

impl LayerComposite {
    /// Makes what the frame composites `groups` with, each a draw group and
    /// how it composites, onto a frame of `size` whose color is of `format`,
    /// and drops it where there are none. With `overdraw`, the overdraw
    /// inspector's, every group blends as `normal`, and without `format` so
    /// does every group that blends in the shader.
    pub(crate) fn update(
        &mut self,
        context: &mut Context,
        shaders: &ShaderRegistry,
        size: Size,
        format: Option<TextureFormat>,
        overdraw: bool,
        groups: &[(SimpleIdentity, Composite)],
    ) {
        self.groups.clear();
        if groups.is_empty() || size.is_empty() {
            self.drop_textures();
            return;
        }
        if self.shader.is_none() {
            self.shader = shaders.shader("LayerCompositeShader", ProjectionVariant::Mercator);
        }
        if self.shader.is_none() {
            return;
        }
        for &(group, mut composite) in groups {
            if overdraw || (format.is_none() && composite.reads_backdrop()) {
                composite.blend = LayerBlendType::Normal;
            }
            self.groups.push((group, composite));
        }
        let frame_format = format.filter(|_| self.groups.iter().any(|(_, c)| c.reads_backdrop()));
        if let Err(error) = self.make(context, size, frame_format) {
            Log::warning(Event::Render, format!("Making the layer composite's textures failed: {error}"));
            self.groups.clear();
            self.drop_textures();
            return;
        }

        let size = size_of::<LayerCompositeUBO>();
        let stride = size.next_multiple_of(context.caps().buffer_offset_alignment as usize);
        let copy = Composite { opacity: 1.0, blend: LayerBlendType::Normal };
        self.bytes.clear();
        self.bytes.resize((self.groups.len() + 1) * stride, 0);
        for (slot, composite) in self.groups.iter().map(|&(_, composite)| composite).chain([copy]).enumerate() {
            let ubo = LayerCompositeUBO {
                opacity: composite.opacity,
                blend: composite.shader_blend(),
                ..LayerCompositeUBO::default()
            };
            self.bytes[slot * stride..][..size].copy_from_slice(bytemuck::bytes_of(&ubo));
        }
        context.device().write_uniform_buffer(&mut self.uniforms, &self.bytes);
        if self.projection.is_none() {
            context.device().write_uniform_buffer_pod(&mut self.projection, &ProjectionUBO::default());
        }
    }

    /// Drops the textures and the bindings that read them.
    fn drop_textures(&mut self) {
        self.target = None;
        self.frame = None;
        self.bindings.clear();
        self.bindings_made = None;
    }

    /// Makes the target of `size`, unless it is made, the frame copy of
    /// `frame_format` where there is one, and the quad.
    fn make(&mut self, context: &mut Context, size: Size, frame_format: Option<TextureFormat>) -> Result<(), GpuError> {
        if self.target.as_ref().is_none_or(|target| target.size != size) {
            self.drop_textures();
            let color = Texture::new(context, size, COLOR)?;
            let depth_stencil = Attachment::new(
                &mut *context.gpu(),
                context.released(),
                "layer composite depth and stencil",
                size,
                DEPTH_STENCIL,
            )?;
            self.target = Some(Target { size, color, depth_stencil });
        }
        match frame_format {
            None => self.frame = None,
            Some(format) if self.frame.as_ref().is_none_or(|frame| frame.format != format) => {
                self.frame = None;
                let frame = Texture::new(context, size, format)?;
                let backdrop = Texture::new(context, size, format)?;
                self.frame = Some(FrameCopy { format, frame, backdrop });
            }
            Some(_) => {}
        }
        if self.quad.is_none() {
            let vertices: [[i16; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];
            let indices = RenderStaticData::quad_triangle_indices();
            self.quad = Some(Quad {
                vertices: context.device().buffer(BufferUsage::Vertex, bytemuck::cast_slice(&vertices))?,
                indices: context.device().buffer(BufferUsage::Index, bytemuck::cast_slice(&indices))?,
                geometry: None,
            });
        }
        Ok(())
    }

    /// The texture the frame draws into rather than its target's, where a
    /// layer of the frame blends in the shader.
    #[must_use]
    pub(crate) fn frame_texture(&self) -> Option<TextureId> {
        self.frame.as_ref().map(|frame| frame.frame.texture)
    }

    /// The pass that copies the frame's texture into the backdrop before the
    /// draw group `group` composites: `None` where it doesn't blend in the
    /// shader.
    #[must_use]
    pub(crate) fn backdrop_pass(&self, group: SimpleIdentity) -> Option<OpenPass> {
        let frame = self.frame.as_ref()?;
        let target = self.target.as_ref()?;
        let (_, composite) = self.groups.iter().find(|(id, _)| *id == group)?;
        composite.reads_backdrop().then_some(OpenPass {
            label: "layer composite backdrop",
            color: Some(ColorAttachment {
                texture: frame.backdrop.texture,
                load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0)),
            }),
            depth_stencil: None,
            size: target.size,
            targets: TargetFormats { color: Some(frame.format), depth_stencil: None },
        })
    }

    /// Whether the draw group `group` draws into the texture in this frame.
    #[must_use]
    pub(crate) fn composites(&self, group: SimpleIdentity) -> bool {
        self.target.is_some() && self.groups.iter().any(|(id, _)| *id == group)
    }

    /// The pass that draws a layer into the texture, cleared to transparent,
    /// with its depth cleared to 1 and its stencil to 0. `None` without a
    /// target.
    pub(crate) fn pass(&self) -> Option<OpenPass> {
        let target = self.target.as_ref()?;
        Some(OpenPass {
            label: "layer composite",
            color: Some(ColorAttachment {
                texture: target.color.texture,
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

    /// Composites the texture onto the open pass, as the draw group `group`
    /// does, at the depth of the current layer: layers above that drew into
    /// the depth buffer hide it.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        target: DrawTarget,
        depth: DepthMode,
        context: &mut Context,
        group: SimpleIdentity,
    ) {
        let Some(slot) = self.groups.iter().position(|(id, _)| *id == group) else {
            return;
        };
        let blend = self.groups[slot].1.blend_function();
        if let Err(error) = self.draw_quad(gpu, target, context, slot, blend, depth) {
            Log::warning(Event::Render, format!("Compositing a layer failed: {error}"));
        }
    }

    /// Copies the frame's texture onto the open pass, over what it holds.
    pub(crate) fn copy_frame(&mut self, gpu: &mut dyn Gpu, target: DrawTarget, context: &mut Context) {
        if self.frame.is_none() {
            return;
        }
        let slot = self.groups.len();
        let blend = BlendFunction::Replace;
        if let Err(error) = self.draw_quad(gpu, target, context, slot, blend, DepthMode::disabled()) {
            Log::warning(Event::Render, format!("Copying the frame failed: {error}"));
        }
    }

    /// Draws the quad with the uniforms of `slot`: a group's, which reads the
    /// layer's texture, or the copy's after them, which reads the frame's.
    fn draw_quad(
        &mut self,
        gpu: &mut dyn Gpu,
        draw_target: DrawTarget,
        context: &mut Context,
        slot: usize,
        blend: BlendFunction,
        depth: DepthMode,
    ) -> Result<(), GpuError> {
        let (Some(shader), Some(target), Some(quad), Some(uniforms), Some(projection)) =
            (&self.shader, &self.target, &mut self.quad, &self.uniforms, &self.projection)
        else {
            return Ok(());
        };
        let DrawTarget { size, targets, scissor } = draw_target;
        let shader_id = match self.shader_id {
            Some(id) => id,
            None => *self.shader_id.insert(context.shader(gpu, shader.key())?),
        };

        let layer = &target.color;
        let frame = self.frame.as_ref();
        let slots = self.groups.len() + 1;
        let made = (
            context.global_uniform_buffers().epoch(),
            uniforms.id(),
            layer.texture,
            frame.map(|frame| frame.frame.texture),
            slots,
        );
        if self.bindings_made != Some(made) {
            self.bindings.clear();
            self.bindings_made = None;
            let global = context.global_uniform_buffers();
            let size = size_of::<LayerCompositeUBO>();
            let stride = size.next_multiple_of(context.caps().buffer_offset_alignment as usize);
            for slot in 0..slots {
                // A group's image is the layer's texture and its backdrop the
                // frame's copy; the copy's image is the frame's texture.
                let (image, backdrop) = match frame {
                    Some(frame) if slot + 1 == slots => (&frame.frame, &frame.frame),
                    Some(frame) => (layer, &frame.backdrop),
                    None => (layer, layer),
                };
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
                for (id, texture) in
                    [(ID_LAYER_COMPOSITE_IMAGE_TEXTURE, image), (ID_LAYER_COMPOSITE_BACKDROP_TEXTURE, backdrop)]
                {
                    entries.push(BindGroupEntry::Texture {
                        id: id as u32,
                        texture: texture.texture,
                        sampler: texture.sampler,
                    });
                }
                let bindings = gpu.create_bindings(&BindingsDesc {
                    label: "layer composite",
                    shader: shader_id,
                    entries: &entries,
                })?;
                self.bindings.push(Owned::new(bindings, context.released()));
            }
            self.bindings_made = Some(made);
        }
        let Some(bindings) = self.bindings.get(slot) else {
            return Ok(());
        };

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

        let pipeline = context.pipeline(
            gpu,
            &PipelineKey {
                shader: shader_id,
                vertex_buffers: vec![layout()],
                state: FixedState {
                    primitive: DrawModeType::Triangles,
                    cull: CullFaceMode::disabled(),
                    blend,
                    color_mask: ColorMode::alpha_blended().mask,
                    depth: DepthState::from(&depth),
                    stencil: StencilState::from(&StencilMode::disabled()),
                },
                targets,
            },
        )?;
        let full = Scissor { x: 0, y: 0, width: size.width, height: size.height };
        gpu.draw(&Draw {
            pipeline,
            bindings: bindings.id(),
            geometry,
            first_index: 0,
            index_count: 6,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
            global_index: 0,
            dynamic: DynamicState {
                depth_range: (depth.range.min, depth.range.max),
                scissor: scissor.unwrap_or(full),
                stencil_reference: 0,
                blend_constant: Color::default(),
                line_width: 1.0,
            },
        })?;
        let stats = context.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        Ok(())
    }
}
