//! Layers drawn as a whole: a layer with a `*-layer-opacity` below 1, or
//! with a `*-layer-blend` other than `normal`, draws into a texture of the
//! frame's size, which is then composited onto the frame with that opacity
//! and blend. Its features overlap within the texture as they would on the
//! frame, so the opacity and blend apply once to the layer rather than to
//! each feature.
//!
//! A translucent layer of 3D geometry draws the same way, opaque, where the
//! texture's pass can test and write the frame's depth: the nearest of its
//! surfaces then covers each pixel once, behind and in front of the other
//! layers' 3D geometry as on the frame.
//!
//! `screen` and `plus-lighter` are blend functions. The other blends are
//! computed in the shader against the backdrop, which a pass can't sample
//! while it draws into it: a frame with such a layer draws into a texture of
//! its own, copied into a second texture before each such layer and onto the
//! frame's target at the end.

use mln_gfx::ColorMode;
use mln_gfx::color_mode::{BlendFunction, LinearBlend};
use mln_gfx::gpu::{
    ColorAttachment, DepthStencilAttachment, Gpu, GpuError, LoadOp, TargetFormats, TextureFormat, TextureId,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::ColorBlendFactorType;
use mln_style_types::LayerBlendType;
use mln_util::Color;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::draw_objects::Mesh;
use crate::encode::{Draw, Group};
use crate::gfx::context::Resources;
use crate::gfx::resources::GpuTexture;
use crate::handles::{Buffer, IndexBuffer, Texture};
use crate::mesh_desc::MeshDesc;
use crate::paint_parameters::{DepthTest, OpenPass, PaintParameters, Stencil};
use crate::render_static_data::RenderStaticData;
use crate::shaders::programs;
use crate::shaders::ubo::{
    LAYER_COMPOSITE_BLEND_COLOR, LAYER_COMPOSITE_BLEND_LUMINOSITY, LAYER_COMPOSITE_BLEND_MULTIPLY,
    LAYER_COMPOSITE_BLEND_NONE, LAYER_COMPOSITE_BLEND_OVERLAY, LayerCompositeUBO,
};

/// The format of the texture a composited layer draws into: its colors are
/// premultiplied, as the frame's are.
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
/// The format of its depth and stencil, which the layer's tile clipping and
/// depth tests read as they would the frame's.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// How a draw group's draws composite onto the frame, after they draw
/// into a texture of their own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Composite {
    /// The opacity of the drawn texture.
    pub opacity: f32,
    /// How the drawn texture blends with the frame.
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

/// A texture that passes draw into and meshes sample.
struct Attachment {
    texture: Texture,
    id: TextureId,
}

impl Attachment {
    fn new(gpu: &mut dyn Gpu, context: &mut Resources, size: Size, format: TextureFormat) -> Result<Self, GpuError> {
        let texture = context.texture(|texture| {
            texture.set_size(size);
            texture.set_format(format);
            texture.set_render_target();
        });
        let id = context.create_texture(gpu, &texture.0)?;
        Ok(Attachment { texture, id })
    }
}

/// The textures a composited layer draws into, of one size.
struct Target {
    size: Size,
    color: Attachment,
    /// The depth and stencil of the layers that don't draw with the frame's,
    /// from the first frame that composites one.
    depth_stencil: Option<GpuTexture>,
}

/// The textures of a frame with a layer that blends in the shader: the
/// frame draws into `frame`, which is copied into `backdrop` for each such
/// layer and onto the frame's target at the end.
struct FrameCopy {
    format: TextureFormat,
    frame: Attachment,
    backdrop: Attachment,
}

/// The quad that covers the frame.
struct Quad {
    vertices: Buffer<[i16; 2]>,
    indexes: IndexBuffer,
}

/// The texture that the frame's composited layers draw into, one after
/// another, and the draws that composite one onto the frame.
#[derive(Default)]
pub(crate) struct LayerComposite {
    target: Option<Target>,
    frame: Option<FrameCopy>,
    quad: Option<Quad>,
    /// The draw groups composited in the frame, by id, in the order of their
    /// draws.
    groups: Vec<(SimpleIdentity, Composite)>,
    /// The meshes that draw the quad, by how they blend and whether they
    /// read the frame's texture rather than the layer's: made of the
    /// textures they sample, and dropped with them.
    meshes: Vec<((BlendFunction, bool), Mesh)>,
    /// The draw of each group, in its order, and then the copy's: each the
    /// mesh it draws, and its draws' buffers, which hold its block.
    draws: Vec<(usize, Group)>,
}

impl LayerComposite {
    /// Makes what the frame composites `groups` with, each a draw group and
    /// how it composites, onto a frame of `size` whose color is of `format`,
    /// and drops it where there are none. With `overdraw`, the overdraw
    /// inspector's, every group blends as `normal`, and without `format` so
    /// does every group that blends in the shader.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        size: Size,
        format: Option<TextureFormat>,
        overdraw: bool,
        groups: &[(SimpleIdentity, Composite)],
    ) {
        self.groups.clear();
        if groups.is_empty() || size.is_empty() {
            self.drop_textures();
            self.quad = None;
            return;
        }
        for &(group, mut composite) in groups {
            if overdraw || (format.is_none() && composite.reads_backdrop()) {
                composite.blend = LayerBlendType::Normal;
            }
            self.groups.push((group, composite));
        }
        let frame_format = format.filter(|_| self.groups.iter().any(|(_, c)| c.reads_backdrop()));
        if let Err(error) = self.make(gpu, context, size, frame_format) {
            Log::warning(Event::Render, format!("Making the layer composite's textures failed: {error}"));
            self.groups.clear();
            self.drop_textures();
            return;
        }

        let copy = Composite { opacity: 1.0, blend: LayerBlendType::Normal, frame_depth: false };
        let groups: Vec<_> =
            self.groups.iter().map(|&(_, composite)| (composite, composite.blend_function(), false)).collect();
        let slots: Vec<_> = groups
            .into_iter()
            .chain([(copy, BlendFunction::Replace, true)])
            .map(|(composite, blend, of_frame)| (composite, self.mesh(context, blend, of_frame)))
            .collect();
        self.draws.truncate(slots.len());
        self.draws.resize_with(slots.len(), || (0, Group::new()));
        for ((composite, index), (mesh, draws)) in slots.into_iter().zip(&mut self.draws) {
            *mesh = index;
            let ubo =
                LayerCompositeUBO { opacity: composite.opacity, blend: composite.shader_blend(), ..Default::default() };
            draws.begin(0.0, false, false);
            draws.push(&self.meshes[*mesh].1, Draw::DEFAULT, ()).block(&ubo);
            draws.finish(gpu, context, &(), []);
            draws.upload(gpu, context);
        }
    }

    /// The index of the mesh that blends with `blend`, of the frame's
    /// texture where `of_frame` and otherwise of the layer's, made if there
    /// is none. A mesh that blends in the shader reads the frame's copy as
    /// its backdrop, and otherwise none.
    fn mesh(&mut self, context: &mut Resources, blend: BlendFunction, of_frame: bool) -> usize {
        if let Some(index) = self.meshes.iter().position(|(key, _)| *key == (blend, of_frame)) {
            return index;
        }
        let (Some(target), Some(quad)) = (&self.target, &self.quad) else { return 0 };
        let (image, backdrop) = match &self.frame {
            Some(frame) if of_frame => (&frame.frame.texture, &frame.frame.texture),
            Some(frame) => (&target.color.texture, &frame.backdrop.texture),
            None => (&target.color.texture, &target.color.texture),
        };
        let mut mesh = MeshDesc::new(&programs::LAYER_COMPOSITE, &quad.indexes);
        mesh.vertices(&quad.vertices)
            .texture(image, SamplerState::default())
            .texture(backdrop, SamplerState::default())
            .blend(ColorMode { blend_function: blend, ..ColorMode::alpha_blended() })
            .projection(ProjectionVariant::Mercator);
        self.meshes.push(((blend, of_frame), context.mesh(&mesh)));
        self.meshes.len() - 1
    }

    /// Drops the textures and the meshes that read them.
    fn drop_textures(&mut self) {
        self.target = None;
        self.frame = None;
        self.meshes.clear();
    }

    /// Makes the target of `size`, unless it is made, with a depth and
    /// stencil of its own where a group draws with them, the frame copy of
    /// `frame_format` where there is one, and the quad.
    fn make(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        size: Size,
        frame_format: Option<TextureFormat>,
    ) -> Result<(), GpuError> {
        if self.target.as_ref().is_none_or(|target| target.size != size) {
            self.drop_textures();
            let color = Attachment::new(gpu, context, size, COLOR)?;
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
                    self.meshes.clear();
                }
            }
            Some(format) if self.frame.as_ref().is_none_or(|frame| frame.format != format) => {
                self.frame = None;
                self.meshes.clear();
                let frame = Attachment::new(gpu, context, size, format)?;
                let backdrop = Attachment::new(gpu, context, size, format)?;
                self.frame = Some(FrameCopy { format, frame, backdrop });
            }
            Some(_) => {}
        }
        if self.quad.is_none() {
            let vertices: [[i16; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];
            let indexes = context.index_buffer(&RenderStaticData::quad_triangle_indices());
            self.quad = Some(Quad { vertices: context.buffer(&vertices), indexes });
        }
        Ok(())
    }

    /// The texture the frame draws into rather than its target's, where a
    /// layer of the frame blends in the shader.
    #[must_use]
    pub(crate) fn frame_texture(&self) -> Option<TextureId> {
        self.frame.as_ref().map(|frame| frame.frame.id)
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
                texture: frame.backdrop.id,
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
                texture: target.color.id,
                load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0)),
            }),
            depth_stencil,
            size: target.size,
            targets: TargetFormats { color: Some(COLOR), depth_stencil: format },
        })
    }

    /// Composites the texture onto `parameters`' open pass, as the draw
    /// group `group` does, testing `depth`.
    pub(crate) fn draw(
        &self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        depth: DepthTest,
        group: SimpleIdentity,
    ) {
        if let Some(slot) = self.groups.iter().position(|(id, _)| *id == group) {
            self.draw_slot(gpu, parameters, slot, depth);
        }
    }

    /// Copies the frame's texture onto `parameters`' open pass, over what it
    /// holds.
    pub(crate) fn copy_frame(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.frame.is_some() {
            self.draw_slot(gpu, parameters, self.groups.len(), DepthTest::DISABLED);
        }
    }

    /// Draws the quad as the draw of `slot` does: a group's, which reads the
    /// layer's texture, or the copy's after them, which reads the frame's.
    fn draw_slot(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, slot: usize, depth: DepthTest) {
        let Some((mesh, draws)) = self.draws.get(slot) else { return };
        let Some((_, mesh)) = self.meshes.get(*mesh) else { return };
        draws.draw_mesh(gpu, parameters, mesh, 0, depth, Stencil::DISABLED);
    }
}
