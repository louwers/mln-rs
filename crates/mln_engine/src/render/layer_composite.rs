//! Layers drawn as a whole: a layer with `fill-layer-opacity` or
//! `line-layer-opacity` below 1, or with a `*-layer-blend` other than
//! `normal`, draws into a texture of the frame's size, which is then
//! composited onto the frame with that opacity and blend. Its features
//! overlap within the texture as they would on the frame, so the opacity and
//! blend apply once to the layer rather than to each feature.
//!
//! A translucent fill extrusion layer draws the same way, opaque, where the
//! texture's pass can test and write the frame's depth: the nearest of its
//! surfaces then covers each pixel once, behind and in front of the other
//! layers' 3D geometry as on the frame.
//!
//! `screen` and `plus-lighter` are blend functions. The other blends are
//! computed in the shader against the backdrop, which a pass can't sample
//! while it draws into it: a frame with such a layer draws into a texture of
//! its own, copied into a second texture before each such layer and onto the
//! frame's target at the end.

use mln_gfx::color_mode::{BlendFunction, LinearBlend};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferUsage, ColorAttachment, DepthState, DepthStencilAttachment, Draw,
    DynamicState, FixedState, GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PipelineKey, SamplerId, Scissor,
    ShaderId, ShaderKey, StencilState, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureUsage,
    VertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::{BUFFER_IDS, PROJECTION_ID};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::ColorBlendFactorType;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMode, DrawModeType, Rgba, Size, StencilMode};
use mln_style::types::LayerBlendType;

use crate::draw::buffer::Buffer;
use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::draw::objects::Entries;
use crate::draw::owned::Owned;
use crate::draw::resources::{GpuTexture, attachment_bytes};
use crate::draw::vertex_attribute::Uniforms;
use crate::render::paint_parameters::{DrawTarget, OpenPass};
use crate::render::static_data::RenderStaticData;
use crate::shaders::programs;
use crate::shaders::ubo::{LayerCompositeBlendMode, LayerCompositeUBO};

/// The ids of the buffers that the draws bind.
const PROPS_ID: usize = programs::LAYER_COMPOSITE.buffer_id("props");

/// The format of the texture a composited layer draws into: its colors are
/// premultiplied, as the frame's are.
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
/// The format of its depth and stencil, which the layer's tile clipping and
/// depth tests read as they would the frame's.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The slots of the layer's texture and of the frame's behind it.
const IMAGE_SLOT: usize = programs::LAYER_COMPOSITE.texture_slot("image");
const BACKDROP_SLOT: usize = programs::LAYER_COMPOSITE.texture_slot("backdrop");

/// The vertex layout of the quad that covers the frame: a corner of the unit
/// square in two shorts.
fn layout() -> VertexBufferLayout {
    VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute {
            location: const { programs::LAYER_COMPOSITE.location("pos") as u32 },
            format: AttributeDataType::Short2,
            offset: 0,
        }],
    }
}

/// How a draw group's draws composite onto the frame, after they draw
/// into a texture of their own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Composite {
    pub opacity: f32,
    pub blend: LayerBlendType,
    /// Whether the draws test and write the frame's depth and stencil rather
    /// than the texture's own, as a 3D layer's do: they and the other
    /// layers' 3D draws then hide each other as they would on the frame.
    pub frame_depth: bool,
}

impl Composite {
    /// How a layer with the layer opacity `opacity` and the layer blend
    /// `blend` composites, with a depth and stencil of its own: `None` for
    /// an opaque layer of the `normal` blend, which draws straight onto the
    /// frame.
    #[must_use]
    pub fn of(opacity: f32, blend: LayerBlendType) -> Option<Composite> {
        let composites = opacity < 1.0 || blend != LayerBlendType::Normal;
        composites.then_some(Composite { opacity, blend, frame_depth: false })
    }

    /// The blend the shader computes: none for the blends a blend function
    /// expresses.
    fn shader_blend(self) -> LayerCompositeBlendMode {
        match self.blend {
            LayerBlendType::Normal | LayerBlendType::Screen | LayerBlendType::PlusLighter => {
                LayerCompositeBlendMode::None
            }
            LayerBlendType::Multiply => LayerCompositeBlendMode::Multiply,
            LayerBlendType::Overlay => LayerCompositeBlendMode::Overlay,
            LayerBlendType::Color => LayerCompositeBlendMode::Color,
            LayerBlendType::Luminosity => LayerCompositeBlendMode::Luminosity,
        }
    }

    /// Whether the shader blends with the backdrop, a copy of the frame.
    fn reads_backdrop(self) -> bool {
        reads_backdrop(self.blend)
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

/// Whether a layer that composites with `blend` blends with the backdrop,
/// a copy of the frame beneath it, in the shader.
pub(crate) fn reads_backdrop(blend: LayerBlendType) -> bool {
    Composite { opacity: 1.0, blend, frame_depth: false }.shader_blend() != LayerCompositeBlendMode::None
}

/// A texture that passes draw into and draws sample, with the sampler
/// they sample it with.
struct Texture {
    texture: GpuTexture,
    sampler: SamplerId,
}

impl Texture {
    fn new(gpu: &mut dyn Gpu, context: &mut Context, size: Size, format: TextureFormat) -> Result<Texture, GpuError> {
        let usage = TextureUsage { sampled: true, render_target: true, copy_src: true };
        let desc = TextureDesc { label: "render target", size, format, usage, mipmapped: false };
        let bytes = size.width as usize * size.height as usize * format.bytes_per_texel().unwrap_or(4) as usize;
        let texture = context.device().texture(gpu, &desc, Some(bytes))?;
        Ok(Texture { texture, sampler: context.sampler(gpu, SamplerState::default())? })
    }

    fn id(&self) -> TextureId {
        self.texture.id()
    }
}

/// The textures a composited layer draws into, of one size.
struct Target {
    size: Size,
    color: Texture,
    /// The depth and stencil of the layers that don't draw with the frame's,
    /// from the first frame that composites one.
    depth_stencil: Option<GpuTexture>,
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
    vertices: Buffer,
    indices: Buffer,
    geometry: Option<Owned<GeometryId>>,
}

/// The texture that the frame's composited layers draw into, one after
/// another, and the draw that composites one onto the frame.
#[derive(Default)]
pub(crate) struct LayerComposite {
    shader: Option<ShaderKey>,
    shader_id: Option<ShaderId>,
    target: Option<Target>,
    frame: Option<FrameCopy>,
    quad: Option<Quad>,
    /// The draw groups composited in the frame, by id, in the order of their
    /// slots of `uniforms`.
    groups: Vec<(SimpleIdentity, Composite)>,
    /// Each group's `LayerCompositeUBO`, at an offset the GPU can bind, and
    /// then the copy's: one write a frame.
    uniforms: Option<Buffer>,
    /// The bytes of `uniforms`, kept between frames.
    bytes: Vec<u8>,
    /// A projection the shader doesn't read, for the block that OpenGL's
    /// prelude declares, which WebGL2 rejects a draw without a buffer for.
    projection: Option<Buffer>,
    /// The bindings of each slot of `uniforms`, and what they were made of:
    /// the epoch of the frame's buffers, the textures and the number of
    /// slots, the last of which is the copy's. The composite drops them
    /// when it replaces a buffer or texture they bind.
    bindings: Vec<Owned<BindingsId>>,
    bindings_made: Option<(u64, TextureId, Option<TextureId>, usize)>,
}

impl LayerComposite {
    /// Makes what the frame composites `groups` with, each a draw group and
    /// how it composites, onto a frame of `size` whose color is of `format`,
    /// and drops it where there are none. Without `format`, every group
    /// that blends in the shader blends as `normal`.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        size: Size,
        format: Option<TextureFormat>,
        groups: &[(SimpleIdentity, Composite)],
    ) {
        self.groups.clear();
        if groups.is_empty() || size.is_empty() {
            self.drop_textures();
            self.quad = None;
            return;
        }
        self.shader.get_or_insert_with(|| {
            context.shader_key(&programs::LAYER_COMPOSITE, Uniforms::default(), ProjectionVariant::Mercator)
        });
        for &(group, mut composite) in groups {
            if format.is_none() && composite.reads_backdrop() {
                composite.blend = LayerBlendType::Normal;
            }
            self.groups.push((group, composite));
        }
        let frame_format = format.filter(|_| self.groups.iter().any(|(_, c)| c.reads_backdrop()));
        if let Err(error) = self.make(gpu, context, size, frame_format) {
            context.fail(format!("Making the layer composite's textures failed: {error}"));
            self.groups.clear();
            self.drop_textures();
            return;
        }

        let size = size_of::<LayerCompositeUBO>();
        let stride = size.next_multiple_of(context.caps().buffer_offset_alignment as usize);
        let copy = Composite { opacity: 1.0, blend: LayerBlendType::Normal, frame_depth: false };
        self.bytes.clear();
        self.bytes.resize((self.groups.len() + 1) * stride, 0);
        for (slot, composite) in self.groups.iter().map(|&(_, composite)| composite).chain([copy]).enumerate() {
            let ubo = LayerCompositeUBO {
                opacity: composite.opacity,
                blend: composite.shader_blend() as i32,
                ..LayerCompositeUBO::default()
            };
            self.bytes[slot * stride..][..size].copy_from_slice(bytemuck::bytes_of(&ubo));
        }
        if context.device().write_uniforms(gpu, &mut self.uniforms, &self.bytes, false) {
            self.drop_bindings();
        }
        if self.projection.is_none() {
            let projection = ProjectionUBO::default();
            context.device().write_uniforms(gpu, &mut self.projection, bytemuck::bytes_of(&projection), false);
        }
    }

    /// Drops the textures and the bindings that read them.
    fn drop_textures(&mut self) {
        self.target = None;
        self.frame = None;
        self.drop_bindings();
    }

    fn drop_bindings(&mut self) {
        self.bindings.clear();
        self.bindings_made = None;
    }

    /// Makes the target of `size`, unless it is made, with a depth and
    /// stencil of its own where a group draws with them, the frame copy of
    /// `frame_format` where there is one, and the quad.
    fn make(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        size: Size,
        frame_format: Option<TextureFormat>,
    ) -> Result<(), GpuError> {
        if self.target.as_ref().is_none_or(|target| target.size != size) {
            self.drop_textures();
            let color = Texture::new(gpu, context, size, COLOR)?;
            self.target = Some(Target { size, color, depth_stencil: None });
        }
        if let Some(target) = self.target.as_mut().filter(|target| target.depth_stencil.is_none())
            && self.groups.iter().any(|(_, composite)| !composite.frame_depth)
        {
            target.depth_stencil =
                Some(context.device().attachment(gpu, "layer composite depth and stencil", size, DEPTH_STENCIL)?);
        }
        match frame_format {
            None => {
                if self.frame.take().is_some() {
                    self.drop_bindings();
                }
            }
            Some(format) if self.frame.as_ref().is_none_or(|frame| frame.format != format) => {
                self.frame = None;
                self.drop_bindings();
                let frame = Texture::new(gpu, context, size, format)?;
                let backdrop = Texture::new(gpu, context, size, format)?;
                self.frame = Some(FrameCopy { format, frame, backdrop });
            }
            Some(_) => {}
        }
        if self.quad.is_none() {
            let vertices: [[i16; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];
            let indices = RenderStaticData::quad_triangle_indices();
            self.quad = Some(Quad {
                vertices: context.device().buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&vertices))?,
                indices: context.device().buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&indices))?,
                geometry: None,
            });
        }
        Ok(())
    }

    /// The bytes of its textures: its target's color, depth and stencil,
    /// and the frame and backdrop of a frame with a layer that blends in
    /// the shader.
    pub(crate) fn bytes(&self) -> u64 {
        let Some(target) = &self.target else { return 0 };
        let depth_stencil = target.depth_stencil.as_ref().map_or(0, |_| attachment_bytes(target.size, DEPTH_STENCIL));
        let frame = self.frame.as_ref().map_or(0, |frame| 2 * attachment_bytes(target.size, frame.format));
        attachment_bytes(target.size, COLOR) + depth_stencil + frame
    }

    /// The texture the frame draws into rather than its target's, where a
    /// layer of the frame blends in the shader.
    #[must_use]
    pub(crate) fn frame_texture(&self) -> Option<TextureId> {
        self.frame.as_ref().map(|frame| frame.frame.id())
    }

    /// The texture a pass whose draw groups are `groups` draws into rather
    /// than its own target: the frame copy's, where one of them blends in the
    /// shader in this frame; `None` otherwise.
    #[must_use]
    pub(crate) fn frame_texture_for(&self, groups: &[(SimpleIdentity, Composite)]) -> Option<TextureId> {
        let reads_backdrop = |group: &SimpleIdentity| {
            self.groups.iter().any(|(id, composite)| id == group && composite.reads_backdrop())
        };
        self.frame_texture().filter(|_| groups.iter().any(|(group, _)| reads_backdrop(group)))
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
                texture: frame.backdrop.id(),
                load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 0.0)),
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

    /// Whether the draw group `group` draws into the texture with the
    /// frame's depth and stencil rather than the texture's own.
    #[must_use]
    pub(crate) fn draws_with_frame_depth(&self, group: SimpleIdentity) -> bool {
        self.groups.iter().any(|(id, composite)| *id == group && composite.frame_depth)
    }

    /// The pass that draws the draw group `group` into the texture, cleared
    /// to transparent: with the texture's depth cleared to 1 and its stencil
    /// to 0, or with the depth and stencil of `frame`, the frame's pass, as
    /// they are, where the group draws with those. `None` without a target.
    pub(crate) fn pass(&self, group: SimpleIdentity, frame: &OpenPass) -> Option<OpenPass> {
        let target = self.target.as_ref()?;
        let (depth_stencil, format) = if self.draws_with_frame_depth(group) {
            let loaded =
                |attachment| DepthStencilAttachment { depth: LoadOp::Load, stencil: LoadOp::Load, ..attachment };
            (frame.depth_stencil.map(loaded), frame.targets.depth_stencil)
        } else {
            let own = DepthStencilAttachment {
                texture: target.depth_stencil.as_ref()?.id(),
                depth: LoadOp::Clear(1.0),
                stencil: LoadOp::Clear(0),
            };
            (Some(own), Some(DEPTH_STENCIL))
        };
        Some(OpenPass {
            label: "layer composite",
            color: Some(ColorAttachment {
                texture: target.color.id(),
                load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 0.0)),
            }),
            depth_stencil,
            size: target.size,
            targets: TargetFormats { color: Some(COLOR), depth_stencil: format },
        })
    }

    /// Composites the texture onto the open pass, as the draw group `group`
    /// does, testing `depth`.
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
            context.fail(format!("Compositing a layer failed: {error}"));
        }
    }

    /// Copies the frame's texture onto the open pass, over what it holds.
    pub(crate) fn copy_frame(
        &mut self,
        gpu: &mut dyn Gpu,
        target: DrawTarget,
        context: &mut Context,
    ) -> Result<(), GpuError> {
        if self.frame.is_none() {
            return Ok(());
        }
        let slot = self.groups.len();
        self.draw_quad(gpu, target, context, slot, BlendFunction::Replace, DepthMode::disabled())
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
            None => *self.shader_id.insert(context.shader(gpu, shader)?),
        };

        let layer = &target.color;
        let frame = self.frame.as_ref();
        let slots = self.groups.len() + 1;
        let made = (context.global_uniform_buffers().epoch(), layer.id(), frame.map(|frame| frame.frame.id()), slots);
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
                let mut entries = Entries::new();
                for id in 0..BUFFER_IDS {
                    let (buffer, offset, size) = match id {
                        PROPS_ID => (uniforms, (slot * stride) as u64, size as u64),
                        PROJECTION_ID => (projection, 0, projection.size() as u64),
                        _ => match global.get(id) {
                            Some(buffer) => (buffer, 0, buffer.size() as u64),
                            None => continue,
                        },
                    };
                    entries.push(BindGroupEntry::Buffer { id: id as u32, buffer: buffer.id(), offset, size });
                }
                for (slot, texture) in [(IMAGE_SLOT, image), (BACKDROP_SLOT, backdrop)] {
                    entries.push(BindGroupEntry::Texture {
                        slot: slot as u32,
                        texture: texture.id(),
                        sampler: texture.sampler,
                    });
                }
                let bindings = gpu.create_bindings(&BindingsDesc {
                    label: "layer composite",
                    shader: shader_id,
                    entries: entries.as_slice(),
                })?;
                self.bindings.push(context.own(bindings, Vec::new()));
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
            quad.geometry = Some(context.own(geometry, Vec::new()));
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
                depth_range: depth.range,
                scissor: scissor.unwrap_or(full),
                stencil_reference: 0,
                blend_constant: Rgba::default(),
                line_width: 1.0,
            },
        })?;
        let stats = context.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        Ok(())
    }
}
