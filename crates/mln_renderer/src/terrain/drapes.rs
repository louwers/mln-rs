//! The drapes: a texture for each terrain tile and stack of draped layers,
//! which the stack's layers draw into, flat, as a small frame of their own,
//! and which the tile's draw of the stack samples.
//!
//! A drape draws the frame's layers with the map's camera, so zoom, bearing
//! and line widths are the map's, through a projection of the tile's square
//! of the world onto its texture ([`TransformParameters::for_drape`]). Each
//! draped layer keeps a drawing of its own in each drape of its stack, made
//! by the layer's own draw update ([`LayerParts::update_drape_draws`]), with
//! the drape's uniforms, clip masks and stencil.
//!
//! A drape draws again only when what its layers would draw it from
//! differs from what they drew it from ([`DrapeKey`]); while the camera
//! moves, a drape drawn at another zoom stays, as in GL JS. A drape whose
//! layers make no draw has no texture, so a stack that draws nothing in a
//! tile holds no memory there.

use std::sync::Arc;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_map_types::{MapDebugOptions, View};
use mln_style::layer_type::ErasedLayerStyle;
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use super::TerrainCover;
use crate::clip_masks::ClipMasks;
use crate::gfx::context::Context;
use crate::gfx::resources::{GpuTexture, attachment_bytes};
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
/// draw of their tiles, the map's zoom, and its bearing if a layer draws by
/// it. A drape drawn from the same would get the same texels.
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
    /// The map's bearing, which a layer draws by, or `None`.
    bearing: Option<f64>,
}

impl DrapeKey {
    /// Whether a drape drawn from `drawn` has the texels that this key would
    /// draw: of the same layers, with the same styles and paint revisions,
    /// from the same tiles, and `at_rest`, with the same values at the same
    /// zoom and bearing. A camera that moves keeps drapes drawn at another
    /// zoom or bearing.
    fn matches(&self, drawn: &DrapeKey, at_rest: bool) -> bool {
        self.tiles == drawn.tiles
            && self.layers.len() == drawn.layers.len()
            && self.layers.iter().zip(&drawn.layers).all(|(layer, drawn)| {
                Arc::ptr_eq(&layer.style, &drawn.style)
                    && layer.paint_revision == drawn.paint_revision
                    && layer.tiles == drawn.tiles
                    && (!at_rest || Arc::ptr_eq(&layer.evaluated, &drawn.evaluated))
            })
            && (!at_rest || (self.zoom == drawn.zoom && self.bearing == drawn.bearing))
    }

    /// Whether a bucket in `changed` is among the key's.
    fn holds(&self, changed: &ChangedBuckets) -> bool {
        self.tiles.iter().any(|tile| tile.bucket.is_some_and(|bucket| changed.contains(bucket)))
    }
}

/// The drape of a terrain tile for one stack of draped layers.
struct StackDrape {
    /// The texture, while the stack's layers draw something in the tile.
    texture: Option<SharedTexture>,
    clip_masks: ClipMasks,
    /// The stack's layers' drawings, in drawing order.
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

/// A drape's texture, of `size` texels a side, with its mips.
fn drape_texture(context: &mut Context, size: u32) -> SharedTexture {
    context.add_texture_with(|texture| {
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
    })
}

impl StackDrape {
    /// A drape not drawn yet, without a texture.
    fn new() -> StackDrape {
        StackDrape {
            texture: None,
            clip_masks: ClipMasks::default(),
            layers: Vec::new(),
            previous: Vec::new(),
            groups: DrawGroups::default(),
            drawn: None,
            key: DrapeKey::default(),
            redraws: false,
        }
    }
}

/// The drapes of a terrain tile, one for each stack.
struct Drape {
    tile: OverscaledTileID,
    transform: TransformParameters,
    stacks: Vec<StackDrape>,
}

/// The drapes of the terrain tiles of a frame, in the cover's order, each
/// with a drape for each of the frame's stacks of draped layers, which has
/// a texture where the stack draws something.
#[derive(Default)]
pub(crate) struct Drapes {
    drapes: Vec<Drape>,
    /// The number of stacks of the frame.
    stacks: usize,
    /// The side of the drapes' textures, in texels.
    size: u32,
    /// The depth and stencil every drape draws with in turn, of `size`.
    depth_stencil: Option<GpuTexture>,
    /// What the layers drawn as a whole in the drapes that draw again draw
    /// into and composite with, of `size`, which every drape's pass uses in
    /// turn.
    layer_composite: LayerComposite,
    /// The tiles and stacks of the drapes drawn since the last
    /// [`take_passes`](Self::take_passes).
    #[cfg(test)]
    passes: Vec<(OverscaledTileID, usize)>,
}

impl Drapes {
    /// Starts a frame's update of the drapes, those of `cover`'s tiles in
    /// `view` and of `stacks` stacks: keeps the drapes of the tiles and
    /// stacks it had, and drops the others. The frame's draped layers then
    /// give their keys ([`key_layer`](Self::key_layer)), which choose the
    /// drapes that draw again ([`choose`](Self::choose)), whose drawings
    /// they then update ([`update_layer`](Self::update_layer)).
    pub(crate) fn begin(&mut self, view: &View, cover: &TerrainCover, stacks: usize) {
        let size = drape_size(cover.tile_size);
        if size != self.size {
            *self = Drapes { size, ..Drapes::default() };
        }
        self.stacks = stacks;
        let mut kept = std::mem::take(&mut self.drapes);
        self.drapes.reserve(cover.tiles.len());
        for tile in &cover.tiles {
            let transform = TransformParameters::for_drape(view, &tile.id.to_unwrapped());
            let mut drape = match kept.iter().position(|drape| drape.tile == tile.id) {
                Some(index) => Drape { transform, ..kept.swap_remove(index) },
                None => Drape { tile: tile.id, transform, stacks: Vec::new() },
            };
            drape.stacks.resize_with(stacks, StackDrape::new);
            for drape in &mut drape.stacks {
                drape.key.layers.clear();
                drape.key.tiles.clear();
                drape.key.zoom = view.zoom();
                drape.key.bearing = None;
            }
            self.drapes.push(drape);
        }
    }

    /// The number of stacks of the frame, each of which has a drape of every
    /// tile.
    pub(crate) fn stacks(&self) -> usize {
        self.stacks
    }

    /// Adds `layer`, a layer of `stack` that drapes, which draws from
    /// `source`, to the key of every drape of the stack. The stack's layers
    /// come in drawing order.
    pub(crate) fn key_layer(&mut self, stack: usize, layer: &dyn RenderLayer, source: Option<LayerSource<'_>>) {
        let base = layer.base();
        for drape in &mut self.drapes {
            let Some(StackDrape { key, .. }) = drape.stacks.get_mut(stack) else {
                continue;
            };
            let start = key.tiles.len();
            layer.drape_stamps(source, drape.tile, &mut key.tiles);
            if layer.drapes_by_bearing() {
                key.bearing = Some(drape.transform.view.angle());
            }
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
        for drape in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
            drape.redraws = drape.drawn.as_ref().is_none_or(|drawn| !drape.key.matches(drawn, at_rest));
            if drape.redraws {
                drape.previous = std::mem::take(&mut drape.layers);
            }
        }
    }

    /// Makes the draws of `layer`, a layer of `stack` that drapes, in every
    /// drape of the stack that draws again, with the drape's `parameters`,
    /// from its `source`. The stack's layers update in drawing order.
    pub(crate) fn update_layer(
        &mut self,
        stack: usize,
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
        for Drape { tile, transform, stacks } in &mut self.drapes {
            let Some(StackDrape { clip_masks, layers, previous, .. }) =
                stacks.get_mut(stack).filter(|drape| drape.redraws)
            else {
                continue;
            };
            let layer_composite = &mut self.layer_composite;
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
    /// longer drape in their stack go. A drape that draws again and whose
    /// layers draw something has a texture, made in `context`; one whose
    /// layers draw nothing has none and draws no pass, drawn from its key.
    pub(crate) fn finish_update(&mut self, context: &mut Context) {
        for drape in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
            drape.previous.clear();
            if !drape.redraws {
                continue;
            }
            let mut draws = false;
            for layer in &drape.layers {
                layer.drawing.visit_groups(&mut |group| draws = draws || group.draws());
            }
            if draws {
                drape.texture.get_or_insert_with(|| drape_texture(context, self.size));
            } else {
                drape.texture = None;
                drape.drawn = Some(std::mem::take(&mut drape.key));
                drape.redraws = false;
            }
        }
    }

    /// The texture of the drape of `stack` of the tile at `index` in the
    /// cover, `tile`; `None` where the stack draws nothing in the tile.
    pub(crate) fn texture(&self, stack: usize, index: usize, tile: &OverscaledTileID) -> Option<&SharedTexture> {
        let drape = self.drapes.get(index).filter(|drape| drape.tile == *tile)?;
        drape.stacks.get(stack)?.texture.as_ref()
    }

    /// Records that the buckets in `changed` changed, for the drapes' draws
    /// built from them, and makes the drapes that show them draw again.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for drape in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
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
        for drape in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
            drape.drawn = None;
        }
    }

    /// Counts in the rendering stats of `context` the stacks, the drapes
    /// whose textures are made, and the bytes of their textures, of the
    /// depth and stencil they draw with, and of their composite.
    pub(crate) fn count(&self, context: &mut Context) {
        let mut drapes = 0;
        let mut bytes = 0;
        for texture in self.drapes.iter().flat_map(|drape| &drape.stacks).filter_map(|drape| drape.texture.as_ref()) {
            if let Some(texture) = context.texture(texture).filter(|texture| texture.gpu_texture().is_some()) {
                drapes += 1;
                bytes += texture.bytes();
            }
        }
        let size = Size::new(self.size, self.size);
        let stats = &mut context.rendering_stats().terrain;
        stats.stacks = self.stacks;
        stats.drapes = drapes;
        stats.mem_drapes = bytes;
        stats.mem_drape_depth_stencil =
            self.depth_stencil.as_ref().map_or(0, |_| attachment_bytes(size, DEPTH_STENCIL));
        stats.mem_drape_composite = self.layer_composite.bytes();
    }

    /// The tiles and stacks of the drapes drawn since the last call.
    #[cfg(test)]
    pub(crate) fn take_passes(&mut self) -> Vec<(OverscaledTileID, usize)> {
        std::mem::take(&mut self.passes)
    }

    /// Makes the textures of the drapes that draw again and what they draw
    /// with, and uploads the GPU objects of their draws and the masks of the
    /// tiles they clip to, through `gpu`, the context of `parameters`, the
    /// frame's. The layer composite takes the composited draw groups of
    /// every drape that draws again, and drops its textures where there are
    /// none.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.drapes.is_empty() {
            self.depth_stencil = None;
            self.layer_composite = LayerComposite::default();
            return;
        }
        let size = Size::new(self.size, self.size);
        let redraws = self.drapes.iter().flat_map(|drape| &drape.stacks).any(|drape| drape.redraws);
        if redraws && self.depth_stencil.is_none() {
            match parameters.context.device().attachment(gpu, "drape depth and stencil", size, DEPTH_STENCIL) {
                Ok(depth_stencil) => self.depth_stencil = Some(depth_stencil),
                Err(error) => {
                    Log::error(Event::Render, format!("Making the drapes' depth and stencil failed: {error}"));
                }
            }
        }
        let stencil_clear_tile = parameters.stencil_clear_tile();
        for Drape { tile, transform, stacks } in &mut self.drapes {
            for drape in stacks.iter_mut().filter(|drape| drape.redraws) {
                let context = &mut *parameters.context;
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
                    stencil_clear_tile.map(|_| tile.to_unwrapped()),
                );
                if let Some(texture) = &drape.texture
                    && let Err(error) = context.create_texture(gpu, texture)
                {
                    Log::error(Event::Render, format!("Making a drape's texture failed: {error}"));
                }
                drape.clip_masks.update(gpu, context, false, &drape.groups.clip_tiles, |tile| {
                    to_projection_ubo(&transform.projection_data_for_tile(tile, false))
                });
            }
        }
        let redrawn = self.drapes.iter().flat_map(|drape| &drape.stacks).filter(|drape| drape.redraws);
        let composited: Vec<_> = redrawn.flat_map(|drape| drape.groups.composited.iter().copied()).collect();
        let overdraw = parameters.debug_options.contains(MapDebugOptions::OVERDRAW);
        self.layer_composite.update(gpu, parameters.context, size, Some(COLOR), overdraw, &composited);
    }

    /// Draws each drape that draws again into its texture, through `gpu`,
    /// the context of `parameters`, the frame's, outside any pass, and then
    /// its texture's mips: its layers' opaque draws front to back, then
    /// their translucent ones back to front, over transparent texels. A
    /// drape with a layer that blends in the shader draws into the layer
    /// composite's frame texture, which is then copied into its own.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(depth_stencil) = self.depth_stencil.as_ref().map(GpuTexture::id) else {
            return;
        };
        let size = Size::new(self.size, self.size);
        let targets = TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) };
        let layer_composite = &mut self.layer_composite;
        for Drape { tile, transform, stacks } in &mut self.drapes {
            #[cfg_attr(not(test), expect(unused_variables, reason = "only tests record the passes' stacks"))]
            for (stack, drape) in stacks.iter_mut().enumerate().filter(|(_, drape)| drape.redraws) {
                let StackDrape { texture, clip_masks, layers, groups, drawn, key, .. } = drape;
                // A drape that fails to draw draws again in the next frame.
                *drawn = None;
                let Some(texture) = texture.as_ref() else {
                    continue;
                };
                let color = match parameters.context.create_texture(gpu, texture) {
                    Ok(color) => color,
                    Err(error) => {
                        Log::error(Event::Render, format!("Making a drape's texture failed: {error}"));
                        continue;
                    }
                };
                let clear = LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0));
                let frame_texture = layer_composite.frame_texture_for(&groups.composited);
                let pass = OpenPass {
                    label: "drape",
                    color: Some(ColorAttachment { texture: frame_texture.unwrap_or(color), load: clear }),
                    depth_stencil: Some(DepthStencilAttachment {
                        texture: depth_stencil,
                        depth: LoadOp::Clear(1.0),
                        stencil: LoadOp::Clear(0),
                    }),
                    size,
                    targets,
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
                    // A drape whose frame doesn't reach it failed, as one
                    // whose pass did.
                    let presented = match frame_texture {
                        Some(_) => {
                            let present = OpenPass {
                                label: "drape",
                                color: Some(ColorAttachment { texture: color, load: clear }),
                                depth_stencil: None,
                                size,
                                targets: TargetFormats { color: Some(COLOR), depth_stencil: None },
                            };
                            drape_parameters.present_frame(gpu, present)
                        }
                        None => Ok(()),
                    };
                    drape_parameters.end_pass(gpu).and(presented)
                });
                match drawn_pass.and_then(|()| parameters.context.generate_mipmaps(gpu, texture)) {
                    Ok(()) => {
                        *drawn = Some(std::mem::take(key));
                        parameters.context.rendering_stats().terrain.drapes_drawn += 1;
                        #[cfg(test)]
                        self.passes.push((*tile, stack));
                    }
                    Err(error) => Log::error(Event::Render, format!("Drawing a drape failed: {error}")),
                }
            }
        }
    }
}
