use std::rc::Rc;

use geometry::Point;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_tile_id::OverscaledTileID;
use mln_util::identity::SimpleIdentity;
use mln_util::time::{Instant, Instant as Time};

use crate::draw::DrawObjects;
use crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::layers::collision_layer_tweaker::CollisionDrawableData;
use crate::layers::color_relief_layer_tweaker::ColorReliefDrawableData;
use crate::layers::hillshade_prepare_layer_tweaker::HillshadePrepareDrawableData;
use crate::layers::line_layer_tweaker::LineDrawableData;
use crate::layers::render_symbol_layer::SymbolDrawableData;
use crate::paint_parameters::PaintParameters;
use crate::render_pass::RenderPass;

pub(crate) type DrawPriority = i64;

/// What a layer keeps with each of its drawables.
#[derive(Clone, Debug)]
pub(crate) enum DrawableData {
    Collision(CollisionDrawableData),
    ColorRelief(ColorReliefDrawableData),
    HillshadePrepare(HillshadePrepareDrawableData),
    /// When a hillshade drawable's geometry was made.
    HillshadeGeometryTime(Time),
    Line(LineDrawableData),
    Symbol(SymbolDrawableData),
    /// The text lines of a debug overlay drawable.
    DebugText(Vec<String>),
}

/// Geometry drawn with one program, with the GPU objects that draw it, and
/// the state a layer group draws it with: its pass, tile and order, and its
/// depth and stencil modes. A layer group draws it into the open pass of the
/// context's `Gpu`.
pub(crate) struct Drawable {
    pub(crate) enabled: bool,
    pub(crate) enable_stencil: bool,
    pub(crate) enable_depth: bool,
    pub(crate) is_3d: bool,
    pub(crate) unique_id: SimpleIdentity,
    pub(crate) render_pass: RenderPass,
    pub(crate) tile_id: Option<OverscaledTileID>,
    pub(crate) draw_priority: DrawPriority,
    pub(crate) sub_layer_index: i32,
    pub(crate) depth_type: DepthMaskType,
    /// The stencil mode a 2D drawable draws with, in place of the mode that
    /// clips it to its tile.
    pub(crate) stencil_mode: Option<StencilMode>,
    /// What the drawable's layer keeps with it.
    pub(crate) data: Option<DrawableData>,
    /// Sets the symbol atlases' textures on the drawable before it draws.
    pub(crate) atlases: Option<Rc<DrawableAtlasesTweaker>>,
    /// The key of the layer tweaker the drawable was built for, which writes
    /// its buffers (`LayerTweakerBase::key`).
    pub(crate) tweaker: Option<SimpleIdentity>,
    pub(crate) type_: usize,
    pub(crate) origin: Option<Point<f64>>,
    /// The index of the drawable's entries in the storage buffers of its
    /// layer group, which shaders read as `globalIndex`.
    pub(crate) ubo_index: u32,
    /// When the drawable was made, to tell whether its geometry is older
    /// than its layer's.
    pub(crate) create_time: Instant,
    /// The depth and stencil modes of a 3D drawable, which its layer group
    /// sets before it draws.
    depth_mode_3d: DepthMode,
    stencil_mode_3d: StencilMode,
    /// The geometry and the GPU objects that draw it.
    pub(crate) objects: DrawObjects,
}

impl Drawable {
    /// Whether the tweaker of key `key` sets the drawable's uniforms: the
    /// drawable was built for it, or for no tweaker.
    #[must_use]
    pub(crate) fn tweaked_by(&self, key: SimpleIdentity) -> bool {
        self.tweaker.is_none_or(|tweaker| tweaker == key)
    }

    #[must_use]
    pub(crate) fn new(name: String) -> Self {
        Drawable {
            enabled: true,
            enable_stencil: false,
            enable_depth: true,
            is_3d: false,
            unique_id: SimpleIdentity::new(),
            render_pass: RenderPass::Opaque,
            tile_id: None,
            draw_priority: 0,
            sub_layer_index: 0,
            depth_type: DepthMaskType::ReadOnly,
            stencil_mode: None,
            data: None,
            atlases: None,
            tweaker: None,
            type_: 0,
            origin: None,
            ubo_index: 0,
            create_time: Instant::now(),
            depth_mode_3d: DepthMode::disabled(),
            stencil_mode_3d: StencilMode::default(),
            objects: DrawObjects::new(name),
        }
    }

    /// Draws the drawable into `parameters`' open pass on `gpu`, the
    /// context's, binding the layer group's buffers in `layer`.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &PaintParameters<'_>,
        layer: Option<&UniformBufferArray>,
    ) {
        if !self.enabled {
            return;
        }
        let (depth_mode, stencil_mode) = if self.is_3d {
            (self.depth_mode_3d, self.stencil_mode_3d)
        } else {
            let depth = if self.enable_depth {
                parameters.depth_mode_for_sublayer(self.sub_layer_index as u8, self.depth_type)
            } else {
                DepthMode::disabled()
            };
            let stencil = match (self.stencil_mode, self.tile_id) {
                (Some(mode), _) => mode,
                (None, Some(tile_id)) if self.enable_stencil => {
                    parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped())
                }
                _ => StencilMode::disabled(),
            };
            (depth, stencil)
        };
        self.objects.draw(gpu, parameters, layer, depth_mode, stencil_mode, self.ubo_index);
    }

    /// Sets the depth and stencil modes a 3D drawable draws with.
    pub(crate) fn set_3d_modes(&mut self, depth: DepthMode, stencil: StencilMode) {
        self.depth_mode_3d = depth;
        self.stencil_mode_3d = stencil;
    }

    /// Whether the drawable draws in pass `value`.
    #[must_use]
    pub(crate) fn has_render_pass(&self, value: RenderPass) -> bool {
        (self.render_pass.underlying_type() & value.underlying_type()) != 0
    }
}

pub(crate) type UniqueDrawable = Box<Drawable>;
