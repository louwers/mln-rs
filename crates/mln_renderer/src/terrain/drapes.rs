//! The drapes: a texture for each terrain tile, which the layers that drape
//! over terrain draw into, flat, as a small frame of their own, and which
//! the tile's draw samples.
//!
//! A drape draws the frame's layers with the map's camera, so zoom, bearing
//! and line widths are the map's, through a projection of the tile's square
//! of the world onto its texture ([`TransformParameters::for_drape`]). Each
//! draped layer keeps a drawing of its own in each drape, made by the
//! layer's own draw update ([`LayerParts::update_drape_draws`]), with the
//! drape's uniforms, clip masks and stencil.
//!
//! A drape draws again only when what its layers would draw it from
//! differs from what they drew it from ([`DrapeKey`]); while the camera
//! moves, a drape drawn at another zoom stays, as in GL JS.

use std::sync::Arc;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_map_types::View;
use mln_style::layer_type::ErasedLayerStyle;
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use super::TerrainCover;
use crate::clip_masks::ClipMasks;
use crate::gfx::context::Context;
use crate::gfx::resources::GpuTexture;
use crate::gfx::store::SharedTexture;
use crate::layer_composite::LayerComposite;
use crate::layer_drawing::{ChangedBuckets, DrapeDrawing, DrawGroups, DrawOrder};
use crate::layer_type::ErasedEvaluatedLayer;
use crate::paint_parameters::{OpenPass, PaintParameters, TransformParameters};
use crate::render_layer::{Atlases, LayerSource, RenderLayer};
use crate::render_pass::RenderPass;
use crate::render_tile::TileStamp;
use crate::tile_matrix::to_projection_ubo;

/// The format of a drape's texture.
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
/// The format of the depth and stencil the drapes draw with.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;
/// The most samples the terrain takes of a drape along a pixel's footprint
/// (GL JS's).
const MAX_ANISOTROPY: u8 = 16;

/// The side in texels of the drapes of terrain tiles of `tile_size` pixels:
/// twice as wide, whatever the pixel ratio (GL JS's quality factor).
pub(crate) fn drape_size(tile_size: u16) -> u32 {
    u32::from(tile_size) * 2
}

/// A drawing of a layer that a drape keeps, with the style it was made for.
struct DrapeLayer {
    style: Arc<dyn ErasedLayerStyle>,
    drawing: DrapeDrawing,
}

/// What a draped layer draws a drape with.
struct LayerKey {
    style: Arc<dyn ErasedLayerStyle>,
    /// The layer's [`paint_revision`](crate::render_layer::RenderLayerBase::paint_revision).
    paint_revision: u64,
    evaluated: Arc<dyn ErasedEvaluatedLayer>,
    /// The number of the layer's tiles among the key's.
    tiles: usize,
}

/// What a drape's layers draw it from: the layers, their values, what they
/// draw of their tiles, and the map's zoom. A drape drawn from the same
/// would get the same texels.
///
/// The images of the style, and the feature state of the tiles' buckets,
/// aren't in it: a change of them makes the drapes they show in draw again
/// ([`Drapes::redraw`], [`Drapes::mark_changed`]).
#[derive(Default)]
struct DrapeKey {
    /// The draped layers, in drawing order.
    layers: Vec<LayerKey>,
    /// The layers' tiles, each layer's after the one before's.
    tiles: Vec<TileStamp>,
    zoom: f64,
}

impl DrapeKey {
    /// Whether a drape drawn from `drawn` has the texels that this key would
    /// draw: of the same layers, with the same styles and paint revisions,
    /// from the same tiles, and `at_rest`, with the same values at the same
    /// zoom. A camera that moves keeps drapes drawn at another zoom.
    fn matches(&self, drawn: &DrapeKey, at_rest: bool) -> bool {
        self.tiles == drawn.tiles
            && self.layers.len() == drawn.layers.len()
            && self.layers.iter().zip(&drawn.layers).all(|(layer, drawn)| {
                Arc::ptr_eq(&layer.style, &drawn.style)
                    && layer.paint_revision == drawn.paint_revision
                    && layer.tiles == drawn.tiles
                    && (!at_rest || Arc::ptr_eq(&layer.evaluated, &drawn.evaluated))
            })
            && (!at_rest || self.zoom == drawn.zoom)
    }

    /// Whether a bucket in `changed` is among the key's.
    fn holds(&self, changed: &ChangedBuckets) -> bool {
        self.tiles.iter().any(|tile| tile.bucket.is_some_and(|bucket| changed.contains(bucket)))
    }
}

/// The drape of a terrain tile.
struct Drape {
    tile: OverscaledTileID,
    texture: SharedTexture,
    transform: TransformParameters,
    clip_masks: ClipMasks,
    /// Composites nothing: the layers that would composite draw straight.
    layer_composite: LayerComposite,
    /// The frame's draped layers' drawings, in drawing order.
    layers: Vec<DrapeLayer>,
    /// The drawings of the last frame that this frame's layers haven't
    /// taken yet, which go once it has updated them.
    previous: Vec<DrapeLayer>,
    /// The number of draw groups of the layers' drawings, which sets the
    /// pass's depths.
    groups: DrawGroups,
    /// What the layers last drew the texture from; `None` before they
    /// first draw it, and once it is stale.
    drawn: Option<DrapeKey>,
    /// What the frame's layers would draw it from.
    key: DrapeKey,
    /// Whether the frame draws the drape again.
    redraws: bool,
}

impl Drape {
    /// The drape of `tile` in `view`, with a texture of `size` texels a side.
    fn new(context: &mut Context, view: &View, tile: OverscaledTileID, size: u32) -> Drape {
        let texture = context.add_texture_with(|texture| {
            texture.set_size(Size::new(size, size));
            texture.set_format(COLOR);
            texture.set_render_target();
            texture.set_mipmapped(true);
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                max_anisotropy: MAX_ANISOTROPY,
                mipmapped: true,
            });
        });
        Drape {
            tile,
            texture,
            transform: TransformParameters::for_drape(view, &tile.to_unwrapped()),
            clip_masks: ClipMasks::default(),
            layer_composite: LayerComposite::default(),
            layers: Vec::new(),
            previous: Vec::new(),
            groups: DrawGroups::default(),
            drawn: None,
            key: DrapeKey::default(),
            redraws: false,
        }
    }
}

/// The drapes of the terrain tiles of a frame, in the cover's order.
#[derive(Default)]
pub(crate) struct Drapes {
    drapes: Vec<Drape>,
    /// The side of the drapes' textures, in texels.
    size: u32,
    /// The depth and stencil every drape draws with in turn, of `size`.
    depth_stencil: Option<GpuTexture>,
    /// The tiles of the drapes drawn since the last [`take_passes`](Self::take_passes).
    #[cfg(test)]
    passes: Vec<OverscaledTileID>,
}

impl Drapes {
    /// Starts a frame's update of the drapes, those of `cover`'s tiles in
    /// `view`: keeps the drapes of the tiles it had, and drops the others.
    /// The frame's draped layers then give their keys
    /// ([`key_layer`](Self::key_layer)), which choose the drapes that draw
    /// again ([`choose`](Self::choose)), whose drawings they then update
    /// ([`update_layer`](Self::update_layer)).
    pub(crate) fn begin(&mut self, context: &mut Context, view: &View, cover: &TerrainCover) {
        let size = drape_size(cover.tile_size);
        if size != self.size {
            *self = Drapes { size, ..Drapes::default() };
        }
        let mut kept = std::mem::take(&mut self.drapes);
        self.drapes.reserve(cover.tiles.len());
        for tile in &cover.tiles {
            let mut drape = match kept.iter().position(|drape| drape.tile == tile.id) {
                Some(index) => {
                    let mut drape = kept.swap_remove(index);
                    drape.transform = TransformParameters::for_drape(view, &tile.id.to_unwrapped());
                    drape
                }
                None => Drape::new(context, view, tile.id, size),
            };
            drape.key.layers.clear();
            drape.key.tiles.clear();
            drape.key.zoom = view.zoom();
            self.drapes.push(drape);
        }
    }

    /// Adds `layer`, a layer of the frame that drapes, which draws from
    /// `source`, to every drape's key. The layers come in drawing order.
    pub(crate) fn key_layer(&mut self, layer: &dyn RenderLayer, source: Option<LayerSource<'_>>) {
        let base = layer.base();
        for drape in &mut self.drapes {
            let key = &mut drape.key;
            let start = key.tiles.len();
            layer.drape_stamps(source, drape.tile, &mut key.tiles);
            key.layers.push(LayerKey {
                style: Arc::clone(&base.style),
                paint_revision: base.paint_revision,
                evaluated: Arc::clone(&base.evaluated_properties),
                tiles: key.tiles.len() - start,
            });
        }
    }

    /// Chooses the drapes the frame draws again, once every draped layer
    /// gave its key: those not drawn yet or stale, and those whose key
    /// doesn't [match](DrapeKey::matches) what they were drawn from, with
    /// the camera `at_rest` or not. Their layers' drawings wait for the
    /// layers to take them.
    pub(crate) fn choose(&mut self, at_rest: bool) {
        for drape in &mut self.drapes {
            drape.redraws = drape.drawn.as_ref().is_none_or(|drawn| !drape.key.matches(drawn, at_rest));
            if drape.redraws {
                drape.previous = std::mem::take(&mut drape.layers);
            }
        }
    }

    /// Makes the draws of `layer`, a layer of the frame that drapes, in
    /// every drape that draws again, with the drape's `parameters`, from its
    /// `source`. The layers update in drawing order.
    pub(crate) fn update_layer(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        atlases: &Atlases,
        layer: &mut dyn RenderLayer,
        source: Option<LayerSource<'_>>,
    ) {
        let size = Size::new(self.size, self.size);
        let style = &layer.base().style;
        let style = Arc::clone(style);
        let pattern_atlas_size = atlases.pattern.pixel_size();
        for drape in self.drapes.iter_mut().filter(|drape| drape.redraws) {
            let Drape { tile, transform, clip_masks, layer_composite, layers, previous, .. } = drape;
            // A drawing made for another style of the layer goes, as the
            // layer's own draws do.
            let mut kept = match previous.iter().position(|kept| Arc::ptr_eq(&kept.style, &style)) {
                Some(index) => previous.swap_remove(index),
                None => DrapeLayer { style: Arc::clone(&style), drawing: layer.new_drape_drawing() },
            };
            let mut parameters = parameters.drape(transform, clip_masks, layer_composite, size, tile.to_unwrapped());
            layer.update_drape_draws(gpu, &mut parameters, atlases, source, *tile, &mut kept.drawing);
            let global = parameters.global_paint_params(pattern_atlas_size);
            kept.drawing.write_uniform(gpu, ID_GLOBAL_PAINT_PARAMS_UBO, &global, parameters.context);
            layers.push(kept);
        }
    }

    /// Ends the frame's update of the drapes: the drawings of layers that no
    /// longer drape go.
    pub(crate) fn finish_update(&mut self) {
        for drape in &mut self.drapes {
            drape.previous.clear();
        }
    }

    /// The texture of the drape of the tile at `index` in the cover, `tile`.
    pub(crate) fn texture(&self, index: usize, tile: &OverscaledTileID) -> Option<&SharedTexture> {
        self.drapes.get(index).filter(|drape| drape.tile == *tile).map(|drape| &drape.texture)
    }

    /// Records that the buckets in `changed` changed, for the drapes' draws
    /// built from them, and makes the drapes that show them draw again.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for drape in &mut self.drapes {
            for layer in &mut drape.layers {
                layer.drawing.mark_changed(changed);
            }
            if drape.drawn.as_ref().is_some_and(|drawn| drawn.holds(changed)) {
                drape.drawn = None;
            }
        }
    }

    /// Makes every drape draw again: what they show changed in a way that
    /// their keys don't tell, such as the style's images.
    pub(crate) fn redraw(&mut self) {
        for drape in &mut self.drapes {
            drape.drawn = None;
        }
    }

    /// The tiles of the drapes drawn since the last call.
    #[cfg(test)]
    pub(crate) fn take_passes(&mut self) -> Vec<OverscaledTileID> {
        std::mem::take(&mut self.passes)
    }

    /// Makes the textures of the drapes that draw again and what they draw
    /// with, and uploads the GPU objects of their draws and the masks of the
    /// tiles they clip to, through `gpu`, the context of `parameters`, the
    /// frame's.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.drapes.is_empty() {
            self.depth_stencil = None;
            return;
        }
        let size = Size::new(self.size, self.size);
        if self.depth_stencil.is_none() {
            match parameters.context.device().attachment(gpu, "drape depth and stencil", size, DEPTH_STENCIL) {
                Ok(depth_stencil) => self.depth_stencil = Some(depth_stencil),
                Err(error) => {
                    Log::error(Event::Render, format!("Making the drapes' depth and stencil failed: {error}"));
                }
            }
        }
        let shaders = &parameters.static_data.shaders;
        let stencil_clear_tile = parameters.stencil_clear_tile();
        for drape in self.drapes.iter_mut().filter(|drape| drape.redraws) {
            let context = &mut *parameters.context;
            if let Err(error) = context.create_texture(gpu, &drape.texture) {
                Log::error(Event::Render, format!("Making a drape's texture failed: {error}"));
            }
            for layer in &mut drape.layers {
                layer.drawing.upload(gpu, context);
            }
            let layers = &drape.layers;
            drape.groups = DrawGroups::of(
                |visit| {
                    for layer in layers {
                        layer.drawing.visit_groups(visit);
                    }
                },
                stencil_clear_tile.map(|_| drape.tile.to_unwrapped()),
            );
            let transform = &drape.transform;
            drape.clip_masks.update(gpu, context, shaders, false, &drape.groups.clip_tiles, |tile| {
                to_projection_ubo(&transform.projection_data_for_tile(tile, false))
            });
        }
    }

    /// Draws each drape that draws again into its texture, through `gpu`,
    /// the context of `parameters`, the frame's, outside any pass, and then
    /// its texture's mips: its layers' opaque draws front to back, then
    /// their translucent ones back to front, over transparent texels.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(depth_stencil) = self.depth_stencil.as_ref().map(GpuTexture::id) else {
            return;
        };
        let size = Size::new(self.size, self.size);
        for drape in self.drapes.iter_mut().filter(|drape| drape.redraws) {
            let Drape { tile, texture, transform, clip_masks, layer_composite, layers, groups, drawn, key, .. } = drape;
            // A drape that fails to draw draws again in the next frame.
            *drawn = None;
            let color = match parameters.context.create_texture(gpu, texture) {
                Ok(color) => color,
                Err(error) => {
                    Log::error(Event::Render, format!("Making a drape's texture failed: {error}"));
                    continue;
                }
            };
            let pass = OpenPass {
                label: "drape",
                color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0)) }),
                depth_stencil: Some(DepthStencilAttachment {
                    texture: depth_stencil,
                    depth: LoadOp::Clear(1.0),
                    stencil: LoadOp::Clear(0),
                }),
                size,
                targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
            };
            let mut drape_parameters =
                parameters.drape(transform, clip_masks, layer_composite, size, tile.to_unwrapped());
            let drawn_pass = drape_parameters.begin_pass(gpu, pass).and_then(|()| {
                drape_parameters.depth_range_size = groups.depth_range_size();
                drape_parameters.pass = RenderPass::Opaque;
                drape_parameters.current_layer = 0;
                for layer in layers.iter_mut().rev() {
                    layer.drawing.render(gpu, &mut drape_parameters, DrawOrder::TopDown);
                }
                drape_parameters.pass = RenderPass::Translucent;
                drape_parameters.current_layer = (groups.count as u32).wrapping_sub(1);
                for layer in layers.iter_mut() {
                    layer.drawing.render(gpu, &mut drape_parameters, DrawOrder::BottomUp);
                }
                drape_parameters.end_pass(gpu)
            });
            match drawn_pass.and_then(|()| parameters.context.generate_mipmaps(gpu, texture)) {
                Ok(()) => {
                    *drawn = Some(std::mem::take(key));
                    #[cfg(test)]
                    self.passes.push(*tile);
                }
                Err(error) => Log::error(Event::Render, format!("Drawing a drape failed: {error}")),
            }
        }
    }
}
