//! The drapes: a texture for each terrain tile and stack of draped layers,
//! which the stack's layers draw into, flat, as a small frame of their own,
//! and which the tile's draw of the stack samples.
//!
//! A drape draws the frame's layers with the map's camera, so zoom, bearing
//! and line widths are the map's, through a projection of the tile's square
//! of the world onto its texture ([`TransformParameters::for_drape`]). On the
//! globe the camera is the map's on Mercator ([`View::mercator`]), as GL JS
//! draws its drapes, so the layers draw as flat as on a Mercator map. A
//! drape draws the tiles over its own tile: on Mercator those of its world
//! copy; on the globe, which has no copies, those of any, in its own, so
//! that near the antimeridian it holds the tiles that the cover puts in the
//! next copy. Each draped layer keeps a drawing of its own in each drape of
//! its stack, made by the layer's own draw update
//! ([`LayerParts::update_drape_draws`]), with the drape's uniforms, clip
//! masks and stencil.
//!
//! A drape draws again only when what its layers would draw it from differs
//! from what they last drew it from ([`DrapeKey`]), or its side changes;
//! otherwise it keeps its texels. A drape whose layers make no draw has no
//! texture, so a stack that draws nothing in a tile holds no memory there.
//!
//! A drape takes GL JS's side, twice its tile's, or the device's largest
//! texture where that is smaller ([`drape_size`]). Where the map sets a
//! byte cap ([`TerrainOptions::drape_memory_limit`]), it lowers the sides
//! of the tiles farthest from the view's center, last in the cover, where
//! the drapes' textures and the depth and stencil they draw with would
//! hold more; it never drops a tile. The sides are settled before the
//! layers update their drawings, so a drape that changes takes its tile's
//! side at once. The cap counts as holding a texture every drape that
//! does, and every drape whose layers draw from something new, which may
//! draw something.
//!
//! A draped layer with a layer opacity below 1 or a layer blend other than
//! `normal` draws as a whole inside each drape, as in 2D
//! ([`LayerComposite`]): into a texture of the drape's side, composited onto
//! the drape. The drapes of a side share one such composite, which each
//! drape's pass uses in turn and which holds textures only in a frame where
//! such a drape draws. A drape with a layer that blends in the shader draws
//! into the composite's frame texture, which is copied into the drape before
//! its mips, so a blend's backdrop is the drape's own texels.
//!
//! [`LayerParts::update_drape_draws`]: crate::layer::render_layer::LayerParts::update_drape_draws
//! [`TerrainOptions::drape_memory_limit`]: crate::map::TerrainOptions::drape_memory_limit

use std::sync::Arc;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::shaders::bindings::PAINT_PARAMS_ID;
use mln_gfx::shaders::layer_ubo::GlobalPaintParamsUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{Rgba, Size};
use mln_style::layer::ErasedLayerStyle;
use smallvec::SmallVec;

use super::TerrainCover;
use crate::camera::View;
use crate::draw::context::Context;
use crate::draw::resources::{GpuTexture, attachment_bytes};
use crate::draw::store::SharedTexture;
use crate::layer::layer_drawing::{ChangedBuckets, DrapeDrawing, DrawGroups, DrawOrder};
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::layer::render_layer::{LayerSource, RenderLayer};
use crate::render::clip_masks::ClipMasks;
use crate::render::layer_composite::LayerComposite;
use crate::render::paint_parameters::{OpenPass, PaintParameters, TransformParameters};
use crate::render::pass::RenderPass;
use crate::render::tile::TileStamp;
use crate::render::tile_matrix::to_projection_ubo;
use crate::tile::OverscaledTileID;

/// The format of a drape's texture.
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
/// The format of the depth and stencil the drapes draw with.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;
/// The most samples the terrain takes of a drape along a pixel's footprint
/// (GL JS's).
const MAX_ANISOTROPY: u8 = 16;

/// The most texels a side of the drapes of terrain tiles of `tile_size`
/// pixels, on a device whose textures have at most `max_texture_size`
/// texels a side: twice as wide, whatever the pixel ratio (GL JS's quality
/// factor, the side of all of GL JS's drapes), or the device's largest.
pub(crate) fn drape_size(tile_size: u16, max_texture_size: u32) -> u32 {
    (u32::from(tile_size) * 2).min(max_texture_size).max(1)
}

/// The bytes of a drape's texture of `side` texels a side, with its mips,
/// as [`Texture2D::bytes`](crate::draw::texture2d::Texture2D::bytes)
/// counts them.
fn texture_bytes(side: u32) -> u64 {
    let bytes = attachment_bytes(Size::new(side, side), COLOR);
    bytes + bytes / 3
}

/// The bytes that the drapes of tiles whose drapes take `sizes` hold, where
/// `held` drapes of each tile have a texture, with a depth and stencil of
/// each side they have.
fn held_bytes(sizes: &[u32], held: &[usize]) -> u64 {
    let mut sides: SmallVec<[u32; 8]> = SmallVec::new();
    let mut bytes = 0;
    for (&size, &held) in sizes.iter().zip(held).filter(|&(_, &held)| held > 0) {
        bytes += held as u64 * texture_bytes(size);
        if !sides.contains(&size) {
            sides.push(size);
        }
    }
    bytes + sides.iter().map(|&side| attachment_bytes(Size::new(side, side), DEPTH_STENCIL)).sum::<u64>()
}

/// Lowers `sizes`, each the side of the drapes of a tile of which `held`
/// have a texture, nearest the view's center first, until they hold at
/// most `cap` bytes ([`held_bytes`]): the largest side allowed halves a
/// step at a time, for the tiles farthest from the center first, down to a
/// texel.
fn cap_sizes(sizes: &mut [u32], held: &[usize], cap: u64) {
    let mut limit = sizes.iter().copied().max().unwrap_or(0);
    while held_bytes(sizes, held) > cap && limit > 1 {
        let lower = limit / 2;
        for tile in (0..sizes.len()).rev() {
            if sizes[tile] > lower {
                sizes[tile] = lower;
                if held_bytes(sizes, held) <= cap {
                    return;
                }
            }
        }
        limit = lower;
    }
}

/// A drawing of a layer that a drape keeps, with the style it was made for.
struct DrapeLayer {
    style: Arc<dyn ErasedLayerStyle>,
    drawing: DrapeDrawing,
}

/// What a draped layer draws a drape with: its values, which it evaluates
/// again for a new style too.
struct LayerKey {
    evaluated: Arc<dyn ErasedEvaluatedLayer>,
    /// The number of the layer's tiles among the key's.
    tiles: usize,
}

/// What a drape's layers draw it from, but for its side: their values,
/// what they draw of their tiles, the global paint values their draws read,
/// which hold the map's zoom, and the map's bearing if a layer draws by it.
/// A drape drawn from the same at the same side would get the same texels.
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
    /// The global paint values of the drape's draws, without its side.
    paint: GlobalPaintParamsUBO,
    /// The map's bearing, which a layer draws by, or `None`.
    bearing: Option<f64>,
}

impl DrapeKey {
    /// Whether a drape drawn from `drawn` holds what this key would draw:
    /// the same layers' values, from the same tiles, with the same global
    /// paint values, and at the same bearing if a layer draws by it.
    fn same(&self, drawn: &DrapeKey) -> bool {
        self.tiles == drawn.tiles
            && self.bearing == drawn.bearing
            && bytemuck::bytes_of(&self.paint) == bytemuck::bytes_of(&drawn.paint)
            && self.layers.len() == drawn.layers.len()
            && self
                .layers
                .iter()
                .zip(&drawn.layers)
                .all(|(layer, drawn)| Arc::ptr_eq(&layer.evaluated, &drawn.evaluated) && layer.tiles == drawn.tiles)
    }

    /// Whether a bucket in `changed` is among the key's.
    fn holds(&self, changed: &ChangedBuckets) -> bool {
        self.tiles.iter().any(|tile| tile.bucket.is_some_and(|bucket| changed.contains(bucket)))
    }
}

/// The drape of a terrain tile for one stack of draped layers.
#[derive(Default)]
struct StackDrape {
    /// The texture, and its side in texels, while the stack's layers draw
    /// something in the tile.
    texture: Option<(SharedTexture, u32)>,
    /// The stack's layers' drawings, in drawing order.
    layers: Vec<DrapeLayer>,
    /// The drawings of the last frame that this frame's layers haven't
    /// taken yet, which go once it has updated them.
    previous: Vec<DrapeLayer>,
    /// The draw groups of the layers' drawings, which set the pass's depths
    /// and the masks it clips to.
    groups: DrawGroups,
    /// What the layers last drew it from, or found nothing to draw in it
    /// from; `None` before they first do, and once it must draw again.
    drawn: Option<DrapeKey>,
    /// What the frame's layers would draw it from.
    key: DrapeKey,
    /// Whether the frame's layers update its drawings, and it draws again
    /// if they draw something.
    updated: bool,
}

/// The drapes of a terrain tile, one for each stack, which share the clip
/// masks of the tile's projection.
struct Drape {
    tile: OverscaledTileID,
    transform: TransformParameters,
    /// The side in texels of its drapes in the frame.
    size: u32,
    clip_masks: ClipMasks,
    stacks: Vec<StackDrape>,
}

/// What the drapes of one side draw with, one after another: a depth and
/// stencil, and what their layers drawn as a whole draw into and composite
/// with.
struct Targets {
    side: u32,
    depth_stencil: Option<GpuTexture>,
    layer_composite: LayerComposite,
}

/// The drapes of the terrain tiles of a frame, in the cover's order.
#[derive(Default)]
pub(crate) struct Drapes {
    drapes: Vec<Drape>,
    /// Whether the frame's view is on the globe, where a drape draws the
    /// tiles over its own whatever their world copy.
    globe: bool,
    /// The most texels a side of a drape ([`drape_size`]).
    largest: u32,
    /// The most bytes the drapes' textures and depths and stencils hold
    /// before the tiles farthest from the view's center take fewer texels.
    byte_cap: u64,
    /// What the drapes draw with, for each side that a drape's texture has.
    targets: Vec<Targets>,
}

impl Drapes {
    /// Starts a frame's update of the drapes, those of `cover`'s tiles in
    /// `view`, and of `stacks` stacks: keeps the drapes of the tiles and
    /// stacks it had, with their drawings and what they were drawn from, and
    /// drops the others. Each tile's drapes take GL JS's side, at most
    /// `max_texture_size` ([`drape_size`]), lowered where they would hold
    /// more than `byte_cap` bytes ([`choose`](Self::choose)). The frame's
    /// draped layers then give their keys ([`key_layer`](Self::key_layer)),
    /// from which `choose` picks the drapes whose drawings they update
    /// ([`update_layer`](Self::update_layer)). The layers draw into the
    /// drapes as on Mercator, on the globe too ([`View::mercator`]).
    pub(crate) fn begin(
        &mut self,
        view: &View,
        cover: &TerrainCover,
        stacks: usize,
        (byte_cap, max_texture_size): (u64, u32),
    ) {
        self.largest = drape_size(cover.tile_size, max_texture_size);
        self.globe = view.is_globe();
        self.byte_cap = byte_cap;
        let mut kept = std::mem::take(&mut self.drapes);
        self.drapes.reserve(cover.tiles.len());
        let flat = view.mercator();
        for tile in &cover.tiles {
            let unwrapped = tile.id.to_unwrapped();
            let transform = TransformParameters::for_drape(&flat, &unwrapped);
            let mut drape = match kept.iter().position(|drape| drape.tile == tile.id) {
                Some(index) => Drape { transform, ..kept.swap_remove(index) },
                None => {
                    Drape { tile: tile.id, transform, size: 0, clip_masks: ClipMasks::default(), stacks: Vec::new() }
                }
            };
            drape.stacks.resize_with(stacks, StackDrape::default);
            for drape in &mut drape.stacks {
                drape.key.layers.clear();
                drape.key.tiles.clear();
                drape.key.bearing = None;
            }
            self.drapes.push(drape);
        }
    }

    /// How many stacks the drapes of each tile are of.
    pub(crate) fn stacks(&self) -> usize {
        self.drapes.first().map_or(0, |drape| drape.stacks.len())
    }

    /// Adds `layer`, a layer of `stack` that drapes, which draws from
    /// `source`, to the key of every drape of the stack. The stack's layers
    /// come in drawing order. A stack that the surface `lit` holds what
    /// doesn't turn with the map.
    pub(crate) fn key_layer(
        &mut self,
        stack: usize,
        layer: &dyn RenderLayer,
        source: Option<LayerSource<'_>>,
        lit: bool,
    ) {
        let base = layer.base();
        let by_bearing = !lit && layer.drapes_by_bearing();
        for drape in &mut self.drapes {
            let Some(StackDrape { key, .. }) = drape.stacks.get_mut(stack) else {
                continue;
            };
            let start = key.tiles.len();
            layer.drape_stamps(source, (drape.tile, self.globe), &mut key.tiles);
            if by_bearing {
                key.bearing = Some(drape.transform.view.angle());
            }
            key.layers
                .push(LayerKey { evaluated: Arc::clone(&base.evaluated_properties), tiles: key.tiles.len() - start });
        }
    }

    /// Settles the side of each tile's drapes and chooses the drapes whose
    /// drawings the frame's layers update, once every draped layer gave its
    /// key, with the frame's `parameters`.
    ///
    /// Each tile's drapes take the largest side, lowered where the drapes
    /// that hold a texture, and the depth and stencil of each side, would
    /// hold more than the byte cap ([`cap_sizes`]). The cap counts the
    /// drapes that have a texture and keep their key, and those whose key
    /// changed, which may draw something. A drape updates where its key
    /// differs from the one it was drawn from, or it has a texture of another
    /// side than its tile's.
    pub(crate) fn choose(&mut self, parameters: &mut PaintParameters<'_>) {
        let mut held = Vec::with_capacity(self.drapes.len());
        for Drape { tile, transform, stacks, .. } in &mut self.drapes {
            let (clip_masks, composite) = (&mut ClipMasks::default(), &mut LayerComposite::default());
            let drape = parameters.drape(transform, clip_masks, composite, Size::new(0, 0), tile.to_unwrapped());
            let paint = drape.global_paint_params();
            let mut tile_held = 0;
            for drape in stacks.iter_mut() {
                drape.key.paint = paint;
                let kept = drape.drawn.as_ref().is_some_and(|drawn| drape.key.same(drawn));
                drape.updated = !kept;
                tile_held += usize::from(!kept || drape.texture.is_some());
            }
            held.push(tile_held);
        }
        let mut sizes = vec![self.largest; self.drapes.len()];
        cap_sizes(&mut sizes, &held, self.byte_cap);
        for (drape, size) in self.drapes.iter_mut().zip(sizes) {
            drape.size = size;
            for drape in &mut drape.stacks {
                drape.updated |= drape.texture.as_ref().is_some_and(|&(_, side)| side != size);
                if drape.updated {
                    drape.previous = std::mem::take(&mut drape.layers);
                }
            }
        }
    }

    /// Makes the draws of `layer`, a layer of `stack` that drapes, in every
    /// drape of the stack that [`choose`](Self::choose) chose, with the
    /// drape's `parameters`, from its `source`, for the surface to shade if
    /// it `lit` the stack ([`PaintParameters::drape_lit`]). The stack's
    /// layers update in drawing order.
    pub(crate) fn update_layer(
        &mut self,
        stack: usize,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        layer: &mut dyn RenderLayer,
        source: Option<LayerSource<'_>>,
        lit: bool,
    ) {
        let globe = self.globe;
        let style = Arc::clone(&layer.base().style);
        // The draws' updates don't read what the layers composite with, which
        // the upload makes.
        let layer_composite = &mut LayerComposite::default();
        for Drape { tile, transform, size, clip_masks, stacks, .. } in &mut self.drapes {
            let Some(StackDrape { layers, previous, .. }) = stacks.get_mut(stack).filter(|drape| drape.updated) else {
                continue;
            };
            // A drawing made for another style of the layer goes, as the
            // layer's own draws do.
            let kept = previous.iter().position(|kept| Arc::ptr_eq(&kept.style, &style));
            let kept = kept.map(|index| previous.swap_remove(index)).or_else(|| {
                let drawing = layer.new_drape_drawing();
                debug_assert!(drawing.is_some(), "a layer that doesn't drape in a drape");
                Some(DrapeLayer { style: Arc::clone(&style), drawing: drawing? })
            });
            let Some(mut kept) = kept else { continue };
            let (unwrapped, size) = (tile.to_unwrapped(), Size::new(*size, *size));
            let mut parameters = parameters.drape(transform, clip_masks, layer_composite, size, unwrapped);
            parameters.drape_lit = lit;
            layer.update_drape_draws(gpu, &mut parameters, source, (*tile, globe), &mut kept.drawing);
            let global = parameters.global_paint_params();
            kept.drawing.write_uniform(gpu, PAINT_PARAMS_ID, &global, parameters.context);
            layers.push(kept);
        }
    }

    /// Ends the frame's update of the drapes: the drawings of layers that no
    /// longer drape in their stack go. An updated drape whose layers draw
    /// something has a texture of its tile's side, made in the stores of
    /// `context`, which it draws again; one whose layers draw nothing lets
    /// go of its texture and draws no pass.
    pub(crate) fn finish_update(&mut self, context: &mut Context) {
        for Drape { size, stacks, .. } in &mut self.drapes {
            for drape in stacks.iter_mut() {
                drape.previous.clear();
                if !drape.updated {
                    continue;
                }
                let mut draws = false;
                for layer in &drape.layers {
                    layer.drawing.visit_groups(&mut |group| draws |= group.draws());
                }
                if !draws {
                    drape.texture = None;
                    drape.updated = false;
                    drape.drawn = Some(std::mem::take(&mut drape.key));
                } else if drape.texture.as_ref().is_none_or(|&(_, side)| side != *size) {
                    drape.texture = Some((drape_texture(context, *size), *size));
                }
            }
        }
    }

    /// The tile and stack of each drape that has a texture, in the cover's
    /// order.
    #[cfg(test)]
    pub(crate) fn textured(&self) -> Vec<(OverscaledTileID, usize)> {
        let stacks = |drape: &Drape| {
            let tile = drape.tile;
            let textured = drape.stacks.iter().enumerate().filter(|(_, stack)| stack.texture.is_some());
            textured.map(move |(stack, _)| (tile, stack)).collect::<Vec<_>>()
        };
        self.drapes.iter().flat_map(stacks).collect()
    }

    /// The tile and stack of each drape that the last frame drew, in the
    /// cover's order.
    #[cfg(test)]
    pub(crate) fn drawn(&self) -> Vec<(OverscaledTileID, usize)> {
        let stacks = |drape: &Drape| {
            let tile = drape.tile;
            let drawn = (drape.stacks.iter().enumerate())
                .filter(|(_, stack)| stack.updated && stack.texture.is_some() && stack.drawn.is_some());
            drawn.map(move |(stack, _)| (tile, stack)).collect::<Vec<_>>()
        };
        self.drapes.iter().flat_map(stacks).collect()
    }

    /// The tiles of the drapes and the side their drapes take.
    #[cfg(test)]
    pub(crate) fn sizes(&self) -> Vec<(OverscaledTileID, u32)> {
        self.drapes.iter().map(|drape| (drape.tile, drape.size)).collect()
    }

    /// The texture of the drape of `stack` of the terrain tile `tile`:
    /// `None` where the stack draws nothing in the tile.
    pub(crate) fn texture(&self, stack: usize, tile: &OverscaledTileID) -> Option<&SharedTexture> {
        let drape = self.drapes.iter().find(|drape| drape.tile == *tile)?;
        drape.stacks.get(stack)?.texture.as_ref().map(|(texture, _)| texture)
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
    /// their keys don't tell, such as the style's images, or the frame that
    /// drew them failed.
    pub(crate) fn redraw(&mut self) {
        for drape in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
            drape.drawn = None;
        }
    }

    /// Counts in the rendering stats of `context` the drapes whose textures
    /// are made, and the bytes of their textures and of the depths, stencils
    /// and composites they draw with.
    pub(crate) fn count(&self, context: &mut Context) {
        let (mut drapes, mut bytes) = (0, 0);
        let textures = self.drapes.iter().flat_map(|drape| &drape.stacks).filter_map(|drape| drape.texture.as_ref());
        for (texture, _) in textures {
            if let Some(texture) = context.texture(texture).filter(|texture| texture.gpu_texture().is_some()) {
                drapes += 1;
                bytes += texture.bytes();
            }
        }
        let depth_stencil: u64 = (self.targets.iter())
            .filter(|targets| targets.depth_stencil.is_some())
            .map(|targets| attachment_bytes(Size::new(targets.side, targets.side), DEPTH_STENCIL))
            .sum();
        let composite: u64 = self.targets.iter().map(|targets| targets.layer_composite.bytes()).sum();
        let held = bytes as u64 + depth_stencil + composite;
        let stats = &mut context.rendering_stats().terrain;
        stats.drapes = drapes;
        stats.mem_drapes = usize::try_from(held).unwrap_or(usize::MAX);
    }

    /// Makes what the drapes that draw again draw with, and uploads the GPU
    /// objects of their draws and the masks of the tiles they clip to,
    /// through `gpu`, the context of `parameters`, the frame's. The drapes
    /// of a side share what they draw with, kept while a drape's texture has
    /// that side: a depth and stencil, made once one of them draws, and a
    /// layer composite, which takes the composited draw groups of those that
    /// draw again, and drops its textures where there are none.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let drapes = || self.drapes.iter().flat_map(|drape| &drape.stacks);
        let sides: SmallVec<[u32; 8]> =
            drapes().filter_map(|drape| drape.texture.as_ref().map(|&(_, side)| side)).collect();
        self.targets.retain(|targets| sides.contains(&targets.side));
        for &side in &sides {
            if self.targets.iter().all(|targets| targets.side != side) {
                let layer_composite = LayerComposite::default();
                self.targets.push(Targets { side, depth_stencil: None, layer_composite });
            }
        }
        let redraws_at = |drape: &StackDrape, side: u32| {
            drape.updated && drape.texture.as_ref().is_some_and(|&(_, texture_side)| texture_side == side)
        };
        for targets in self.targets.iter_mut().filter(|targets| targets.depth_stencil.is_none()) {
            if !drapes().any(|drape| redraws_at(drape, targets.side)) {
                continue;
            }
            // The drapes of the side then fail to draw.
            let size = Size::new(targets.side, targets.side);
            match parameters.context.device().attachment(gpu, "drape depth and stencil", size, DEPTH_STENCIL) {
                Ok(depth_stencil) => targets.depth_stencil = Some(depth_stencil),
                Err(error) => parameters.context.fail(format!("Making the drapes' depth and stencil failed: {error}")),
            }
        }
        for Drape { tile, transform, size, clip_masks, stacks, .. } in &mut self.drapes {
            if !stacks.iter().any(|drape| drape.updated) {
                continue;
            }
            let composite = &mut LayerComposite::default();
            let size = Size::new(*size, *size);
            let parameters = parameters.drape(transform, clip_masks, composite, size, tile.to_unwrapped());
            for drape in stacks.iter_mut().filter(|drape| drape.updated) {
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
                    |tiles| parameters.stencil_clear_tiles(tiles),
                );
            }
            // The masks of the tiles of every stack that draws again, which
            // their passes clip to in turn.
            let mut clip_tiles: Vec<_> = (stacks.iter().filter(|drape| drape.updated))
                .flat_map(|drape| drape.groups.clip_tiles.iter().copied())
                .collect();
            clip_tiles.sort_unstable();
            clip_tiles.dedup();
            let transform = parameters.transform_params;
            parameters.clip_masks.update(gpu, parameters.context, false, &clip_tiles, |tile| {
                to_projection_ubo(&transform.projection_data_for_tile(tile, false))
            });
        }
        for targets in &mut self.targets {
            let redrawn =
                self.drapes.iter().flat_map(|drape| &drape.stacks).filter(|drape| redraws_at(drape, targets.side));
            let composited: Vec<_> = redrawn.flat_map(|drape| drape.groups.composited.iter().copied()).collect();
            let size = Size::new(targets.side, targets.side);
            targets.layer_composite.update(gpu, parameters.context, size, Some(COLOR), &composited);
        }
    }

    /// Draws each drape that draws again into its texture, through `gpu`,
    /// the context of `parameters`, the frame's, outside any pass, and then
    /// its texture's mips: its layers' opaque draws front to back, then
    /// their translucent ones back to front, over transparent texels. A
    /// drape with a layer that blends in the shader draws into the layer
    /// composite's frame texture, which is then copied into its own. A drape
    /// that fails to draw fails the frame, and draws again in the next.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let formats = TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) };
        for Drape { tile, transform, clip_masks, stacks, .. } in &mut self.drapes {
            for drape in stacks.iter_mut().filter(|drape| drape.updated) {
                let StackDrape { texture: Some((texture, side)), layers, groups, drawn, key, .. } = drape else {
                    continue;
                };
                *drawn = None;
                // Without its side's depth and stencil, the upload said why.
                let Some(Targets { depth_stencil: Some(depth_stencil), layer_composite, .. }) =
                    self.targets.iter_mut().find(|targets| targets.side == *side)
                else {
                    continue;
                };
                let (depth_stencil, size) = (depth_stencil.id(), Size::new(*side, *side));
                let color = match parameters.context.create_texture(gpu, texture) {
                    Ok(color) => color,
                    Err(error) => {
                        parameters.context.fail(format!("Making a drape's texture failed: {error}"));
                        continue;
                    }
                };
                let clear = LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 0.0));
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
                    targets: formats,
                };
                let unwrapped = tile.to_unwrapped();
                let mut drape_parameters = parameters.drape(transform, clip_masks, layer_composite, size, unwrapped);
                drape_parameters.stencil_clear.clone_from(&groups.stencil_clear);
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
                    }
                    Err(error) => parameters.context.fail(format!("Drawing a drape failed: {error}")),
                }
            }
        }
    }
}

/// A drape's texture of `side` texels a side, made in `context`'s store,
/// mipmapped and sampled through its mips, anisotropically, clamped to its
/// edges.
fn drape_texture(context: &mut Context, side: u32) -> SharedTexture {
    context.add_texture_with(|texture| {
        texture.set_size(Size::new(side, side));
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

#[cfg(test)]
mod tests {
    use mln_style::geo::LatLng;
    use mln_style::tile_id::CanonicalTileID;
    use mln_style::types::ProjectionDefinition;

    use super::*;
    use crate::camera::transform::Transform;
    use crate::camera::{CameraOptions, ConstrainMode, ViewportMode};
    use crate::render::terrain::{ElevationRanges, TerrainTile};

    /// A view of the globe at `zoom` over 512 by 512 pixels.
    fn globe(center: LatLng, zoom: f64) -> View {
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(mln_style::size::Size::new(512, 512)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(&CameraOptions { center: Some(center), zoom: Some(zoom), ..Default::default() });
        assert!(transform.view().is_globe());
        transform.view().clone()
    }

    /// On the globe, a drape draws its layers as on Mercator: its view isn't
    /// the globe's, and its tile projects through the Mercator matrix onto
    /// its texture, as GL JS draws its drapes.
    #[test]
    fn a_globe_drape_draws_on_mercator() {
        let view = globe(LatLng::new(30.0, 20.0).unwrap(), 3.0);
        let id = OverscaledTileID::new(3, 0, CanonicalTileID::new(3, 4, 3));
        let cover = TerrainCover {
            source_id: "dem".to_owned(),
            tile_size: 512,
            tiles: vec![TerrainTile { id, dem: None }],
            center: None,
            dem_tiles: Vec::new(),
            ranges: ElevationRanges::default(),
        };
        let mut drapes = Drapes::default();
        drapes.begin(&view, &cover, 1, (u64::MAX, 4096));
        let transform = &drapes.drapes[0].transform;
        assert!(!transform.view.is_globe());
        let tile = id.to_unwrapped();
        let projection = transform.projection_data_for_tile(&tile, false);
        let mercator = TransformParameters::for_drape(&view.mercator(), &tile);
        assert_eq!(mercator.proj_matrix * view.mercator().tile_matrix(&tile), projection.main_matrix);
        assert_eq!(0.0, projection.projection_transition);
    }

    /// The largest side is GL JS's, or the device's largest texture's: a
    /// DEM of 512 pixels makes terrain tiles of 1024 and drapes of 2048,
    /// and one of 1024 or 2048 drapes of 4096 or 8192, larger than many
    /// devices take.
    #[test]
    fn the_largest_drape_fits_the_device() {
        assert_eq!(2048, drape_size(1024, 16384));
        assert_eq!(2048, drape_size(2048, 2048));
        assert_eq!(4096, drape_size(4096, 4096));
        assert_eq!(1, drape_size(1024, 0));
    }

    /// Three tiles, nearest the center first, with two drapes each of 2048
    /// texels a side, lowered under `cap`.
    fn capped(cap: u64) -> [u32; 3] {
        let mut sizes = [2048; 3];
        cap_sizes(&mut sizes, &[2; 3], cap);
        sizes
    }

    /// The bytes of three tiles' two drapes each, of `sizes`, and of the
    /// depth and stencil of each side.
    fn held(sizes: [u32; 3]) -> u64 {
        held_bytes(&sizes, &[2; 3])
    }

    /// The cap lowers the largest side a step at a time, for the tiles
    /// farthest from the center first: every tile takes a side before any
    /// takes the next one down.
    #[test]
    fn the_cap_lowers_the_sides_of_the_farthest_tiles_first() {
        assert_eq!([2048, 2048, 2048], capped(u64::MAX));
        assert_eq!([2048, 2048, 1024], capped(held([2048, 2048, 1024])));
        assert_eq!([2048, 1024, 1024], capped(held([2048, 1024, 1024])));
        assert_eq!([1024, 1024, 1024], capped(held([2048, 1024, 1024]) - 1));
        assert_eq!([1024, 1024, 512], capped(held([1024, 1024, 1024]) - 1));
        assert_eq!([1024, 512, 512], capped(held([1024, 1024, 512]) - 1));
    }

    /// The cap counts a depth and stencil for each side the drapes have, and
    /// nothing for a tile whose drapes hold no texture.
    #[test]
    fn the_cap_counts_the_depth_and_stencil_of_each_side() {
        let side = |size: u32| attachment_bytes(Size::new(size, size), DEPTH_STENCIL);
        assert_eq!(
            4 * texture_bytes(2048) + 2 * texture_bytes(1024) + side(2048) + side(1024),
            held([2048, 2048, 1024])
        );
        assert_eq!(texture_bytes(1024) + side(1024), held_bytes(&[2048, 1024], &[0, 1]));
    }

    /// The cap lowers the sides as far as it needs, to a texel.
    #[test]
    fn the_cap_lowers_the_sides_as_far_as_it_needs() {
        assert_eq!([128, 128, 64], capped(held([128, 128, 128]) - 1));
        assert_eq!([1, 1, 1], capped(0));
    }
}
