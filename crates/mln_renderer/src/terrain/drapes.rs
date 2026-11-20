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
//!
//! A drape's side follows its tile's on screen ([`shown_size`]): GL JS's
//! where the tile shows large, fewer texels where it shows small. A byte
//! cap ([`BYTE_CAP`]) lowers the sides of the tiles that show smallest
//! where the drapes would hold more; it never drops a tile. A drape whose
//! side changes draws again once the camera rests, as one drawn at another
//! zoom does, or at once where it is larger, so that the cap holds while
//! the camera moves.

use std::sync::Arc;

use glam::{DMat4, DVec2, DVec4};
use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_map_types::{MapDebugOptions, View};
use mln_style::layer_type::ErasedLayerStyle;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::constants::EXTENT;
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

/// The most texels a side of the drapes of terrain tiles of `tile_size`
/// pixels: twice as wide, whatever the pixel ratio (GL JS's quality
/// factor, the side of all of GL JS's drapes).
pub(crate) fn drape_size(tile_size: u16) -> u32 {
    u32::from(tile_size) * 2
}

/// How many times a drape's side may halve the most: to a sixteenth, 128
/// texels over a 512-pixel DEM.
const SMALLEST_SHIFT: u32 = 4;

/// The most bytes the drapes' textures hold, with their mips, before the
/// tiles that show smallest take fewer texels: less on phones and in the
/// browser, whose GPU shares a smaller memory with the system.
pub(crate) const BYTE_CAP: usize = if cfg!(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "visionos",
    target_env = "ohos",
    target_family = "wasm"
)) {
    256 << 20
} else {
    384 << 20
};

/// The bytes of a drape's texture of `size` texels a side, with its mips,
/// as [`Texture2D::bytes`](crate::gfx::texture2d::Texture2D::bytes)
/// counts them.
fn texture_bytes(size: u32) -> usize {
    let bytes = attachment_bytes(Size::new(size, size), COLOR);
    bytes + bytes / 3
}

/// The logical pixels that `tile` spans in `view`, whose projection is
/// `projection`, level with the view's center: its longest side on
/// screen, and infinite where it reaches behind the eye, under the camera,
/// where it shows larger than anywhere.
fn shown_px(view: &View, projection: &DMat4, tile: &UnwrappedTileID) -> f64 {
    let matrix = *projection * view.tile_matrix(tile);
    let (altitude, extent) = (view.center_altitude(), f64::from(EXTENT));
    let size = view.size();
    let half = DVec2::new(f64::from(size.width), f64::from(size.height)) / 2.0;
    let mut corners = [(0.0, 0.0), (extent, 0.0), (extent, extent), (0.0, extent)].into_iter().map(|(x, y)| {
        let clip = matrix * DVec4::new(x, y, altitude, 1.0);
        (clip.w > 0.0).then(|| DVec2::new(clip.x, clip.y) / clip.w * half)
    });
    let Some(corners) = corners.try_fold(Vec::with_capacity(4), |mut corners, corner| {
        corners.push(corner?);
        Some(corners)
    }) else {
        return f64::INFINITY;
    };
    (0..4).map(|k| corners[k].distance(corners[(k + 1) % 4])).fold(0.0, f64::max)
}

/// The side of the drape of a tile that spans `shown` logical pixels on
/// screen, on a target of `pixel_ratio` physical pixels a logical one: the
/// power of two that covers them at two texels a logical pixel, or a texel
/// a physical pixel where that is more, from a sixteenth of `largest`, GL
/// JS's side, to `largest`, which a tile that shows at more than half its
/// side takes.
pub(crate) fn shown_size(shown: f64, pixel_ratio: f32, largest: u32) -> u32 {
    let texels = shown * f64::from(pixel_ratio).max(2.0);
    if texels.is_nan() || texels >= f64::from(largest) {
        return largest;
    }
    (texels.ceil() as u32).next_power_of_two().clamp(largest >> SMALLEST_SHIFT, largest)
}

/// Lowers `sizes`, each the side of `counts`' drapes of a tile, until the
/// drapes hold at most `cap` bytes: the largest side allowed falls a step
/// at a time, for the tiles that show smallest first, which `order` lists
/// last, down to `smallest`.
fn cap_sizes(sizes: &mut [u32], counts: &[usize], order: &[usize], smallest: u32, cap: usize) {
    let mut held: usize = sizes.iter().zip(counts).map(|(&size, &count)| count * texture_bytes(size)).sum();
    let mut limit = sizes.iter().copied().max().unwrap_or(0);
    while held > cap && limit > smallest {
        let lower = limit / 2;
        for &tile in order.iter().rev() {
            if held <= cap {
                return;
            }
            if sizes[tile] > lower {
                held -= counts[tile] * (texture_bytes(sizes[tile]) - texture_bytes(lower));
                sizes[tile] = lower;
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
    /// The texture, and its side in texels, while the stack's layers draw
    /// something in the tile.
    texture: Option<(SharedTexture, u32)>,
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
    /// Whether it draws again in the frame into a texture of `side` texels a
    /// side.
    fn redraws_at(&self, side: u32) -> bool {
        self.redraws && self.texture.as_ref().is_some_and(|&(_, texture_side)| texture_side == side)
    }

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
    /// The logical pixels the tile spans on screen ([`shown_px`]).
    shown: f64,
    /// The side in texels its drapes draw at in the frame.
    size: u32,
    stacks: Vec<StackDrape>,
}

/// What the drapes of one side draw with, one after another: a depth and
/// stencil, and what their layers drawn as a whole draw into and
/// composite with.
struct Targets {
    size: u32,
    depth_stencil: Option<GpuTexture>,
    layer_composite: LayerComposite,
}

/// The drapes of the terrain tiles of a frame, in the cover's order, each
/// with a drape for each of the frame's stacks of draped layers, which has
/// a texture where the stack draws something.
#[derive(Default)]
pub(crate) struct Drapes {
    drapes: Vec<Drape>,
    /// The number of stacks of the frame.
    stacks: usize,
    /// The most texels a side of a drape ([`drape_size`]).
    largest: u32,
    /// The most bytes the drapes' textures hold before the tiles that show
    /// smallest take fewer texels.
    byte_cap: usize,
    /// What the drapes draw with, for each side a drape's texture has.
    targets: Vec<Targets>,
    /// The tiles and stacks of the drapes drawn since the last
    /// [`take_passes`](Self::take_passes).
    #[cfg(test)]
    passes: Vec<(OverscaledTileID, usize)>,
}

impl Drapes {
    /// Starts a frame's update of the drapes, those of `cover`'s tiles in
    /// `view`, whose projection is `projection`, on a target of
    /// `pixel_ratio`, and of `stacks` stacks: keeps the drapes of the tiles
    /// and stacks it had, and drops the others. Each tile's drapes draw at
    /// the side its tile shows at ([`shown_size`]), which
    /// [`choose`](Self::choose) lowers where they would hold more than
    /// `byte_cap` bytes. The frame's draped layers then give their keys
    /// ([`key_layer`](Self::key_layer)), which choose the drapes that draw
    /// again, whose drawings they then update
    /// ([`update_layer`](Self::update_layer)).
    pub(crate) fn begin(
        &mut self,
        (view, projection, pixel_ratio): (&View, &DMat4, f32),
        cover: &TerrainCover,
        stacks: usize,
        byte_cap: usize,
    ) {
        let largest = drape_size(cover.tile_size);
        if largest != self.largest {
            *self = Drapes { largest, ..Drapes::default() };
        }
        self.stacks = stacks;
        self.byte_cap = byte_cap;
        let mut kept = std::mem::take(&mut self.drapes);
        self.drapes.reserve(cover.tiles.len());
        for tile in &cover.tiles {
            let unwrapped = tile.id.to_unwrapped();
            let transform = TransformParameters::for_drape(view, &unwrapped);
            let shown = shown_px(view, projection, &unwrapped);
            let size = shown_size(shown, pixel_ratio, largest);
            let mut drape = match kept.iter().position(|drape| drape.tile == tile.id) {
                Some(index) => Drape { transform, shown, size, ..kept.swap_remove(index) },
                None => Drape { tile: tile.id, transform, shown, size, stacks: Vec::new() },
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
    /// gave its key, and the side the drapes of each tile draw at: those not
    /// drawn yet or stale, and those whose key doesn't
    /// [match](DrapeKey::matches) what they were drawn from, with the camera
    /// `at_rest` or not. Where the drapes with a texture and those that draw
    /// again would hold more than the byte cap, the sides of the tiles
    /// that show smallest go down ([`cap_sizes`]). A drape whose texture is
    /// of another side draws again at rest, and while the camera moves where
    /// its texture is larger, so that the cap holds. Their layers' drawings
    /// wait for the layers to take them.
    pub(crate) fn choose(&mut self, at_rest: bool) {
        let mut counts = Vec::with_capacity(self.drapes.len());
        for Drape { stacks, .. } in &mut self.drapes {
            let mut count = 0;
            for drape in stacks {
                drape.redraws = drape.drawn.as_ref().is_none_or(|drawn| !drape.key.matches(drawn, at_rest));
                count += usize::from(drape.redraws || drape.texture.is_some());
            }
            counts.push(count);
        }
        let mut order: Vec<usize> = (0..self.drapes.len()).collect();
        order.sort_by(|&a, &b| self.drapes[b].shown.total_cmp(&self.drapes[a].shown));
        let mut sizes: Vec<u32> = self.drapes.iter().map(|drape| drape.size).collect();
        cap_sizes(&mut sizes, &counts, &order, self.largest >> SMALLEST_SHIFT, self.byte_cap);
        for (Drape { size, stacks, .. }, capped) in self.drapes.iter_mut().zip(sizes) {
            *size = capped;
            for drape in stacks {
                let resized =
                    drape.texture.as_ref().is_some_and(|&(_, side)| side > capped || at_rest && side != capped);
                drape.redraws |= resized;
                if drape.redraws {
                    drape.previous = std::mem::take(&mut drape.layers);
                }
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
        let style = &layer.base().style;
        let style = Arc::clone(style);
        let pattern_atlas_size = atlases.pattern.pixel_size();
        // The layers' draw updates don't read the composite, which only
        // the passes do.
        let layer_composite = &mut LayerComposite::default();
        for Drape { tile, transform, size, stacks, .. } in &mut self.drapes {
            let Some(StackDrape { clip_masks, layers, previous, .. }) =
                stacks.get_mut(stack).filter(|drape| drape.redraws)
            else {
                continue;
            };
            let size = Size::new(*size, *size);
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
    /// layers draw something has a texture of its tile's side, made in
    /// `context`; one whose layers draw nothing has none and draws no pass,
    /// drawn from its key.
    pub(crate) fn finish_update(&mut self, context: &mut Context) {
        for Drape { size, stacks, .. } in &mut self.drapes {
            for drape in stacks {
                drape.previous.clear();
                if !drape.redraws {
                    continue;
                }
                let mut draws = false;
                for layer in &drape.layers {
                    layer.drawing.visit_groups(&mut |group| draws = draws || group.draws());
                }
                if !draws {
                    drape.texture = None;
                    drape.drawn = Some(std::mem::take(&mut drape.key));
                    drape.redraws = false;
                } else if drape.texture.as_ref().is_none_or(|&(_, side)| side != *size) {
                    drape.texture = Some((drape_texture(context, *size), *size));
                }
            }
        }
    }

    /// The texture of the drape of `stack` of the tile at `index` in the
    /// cover, `tile`; `None` where the stack draws nothing in the tile.
    pub(crate) fn texture(&self, stack: usize, index: usize, tile: &OverscaledTileID) -> Option<&SharedTexture> {
        let drape = self.drapes.get(index).filter(|drape| drape.tile == *tile)?;
        drape.stacks.get(stack)?.texture.as_ref().map(|(texture, _)| texture)
    }

    /// The tiles of the drapes and the side their drapes draw at.
    #[cfg(test)]
    pub(crate) fn sizes(&self) -> Vec<(OverscaledTileID, u32)> {
        self.drapes.iter().map(|drape| (drape.tile, drape.size)).collect()
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
        let textures = self.drapes.iter().flat_map(|drape| &drape.stacks).filter_map(|drape| drape.texture.as_ref());
        for (texture, _) in textures {
            if let Some(texture) = context.texture(texture).filter(|texture| texture.gpu_texture().is_some()) {
                drapes += 1;
                bytes += texture.bytes();
            }
        }
        let depth_stencil = (self.targets.iter())
            .filter(|targets| targets.depth_stencil.is_some())
            .map(|targets| attachment_bytes(Size::new(targets.size, targets.size), DEPTH_STENCIL))
            .sum();
        let composite = self.targets.iter().map(|targets| targets.layer_composite.bytes()).sum();
        let stats = &mut context.rendering_stats().terrain;
        stats.stacks = self.stacks;
        stats.drapes = drapes;
        stats.mem_drapes = bytes;
        stats.mem_drape_depth_stencil = depth_stencil;
        stats.mem_drape_composite = composite;
    }

    /// The tiles and stacks of the drapes drawn since the last call.
    #[cfg(test)]
    pub(crate) fn take_passes(&mut self) -> Vec<(OverscaledTileID, usize)> {
        std::mem::take(&mut self.passes)
    }

    /// Makes the textures of the drapes that draw again and what they draw
    /// with, and uploads the GPU objects of their draws and the masks of the
    /// tiles they clip to, through `gpu`, the context of `parameters`, the
    /// frame's. The drapes of a side share what they draw with, kept while
    /// a drape's texture has that side: a depth and stencil, made once one
    /// of them draws again, and a layer composite, which takes the
    /// composited draw groups of those that draw again, and drops its
    /// textures where there are none.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.drapes.is_empty() {
            self.targets.clear();
            return;
        }
        let stacks = self.drapes.iter().flat_map(|drape| &drape.stacks);
        let sides: Vec<u32> = stacks.filter_map(|drape| drape.texture.as_ref().map(|&(_, side)| side)).collect();
        self.targets.retain(|targets| sides.contains(&targets.size));
        for side in sides {
            if self.targets.iter().all(|targets| targets.size != side) {
                self.targets.push(Targets {
                    size: side,
                    depth_stencil: None,
                    layer_composite: LayerComposite::default(),
                });
            }
        }
        for targets in self.targets.iter_mut().filter(|targets| targets.depth_stencil.is_none()) {
            if !self.drapes.iter().flat_map(|drape| &drape.stacks).any(|drape| drape.redraws_at(targets.size)) {
                continue;
            }
            let size = Size::new(targets.size, targets.size);
            match parameters.context.device().attachment(gpu, "drape depth and stencil", size, DEPTH_STENCIL) {
                Ok(depth_stencil) => targets.depth_stencil = Some(depth_stencil),
                Err(error) => {
                    Log::error(Event::Render, format!("Making the drapes' depth and stencil failed: {error}"));
                }
            }
        }
        let stencil_clear_tile = parameters.stencil_clear_tile();
        for Drape { tile, transform, stacks, .. } in &mut self.drapes {
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
                if let Some((texture, _)) = &drape.texture
                    && let Err(error) = context.create_texture(gpu, texture)
                {
                    Log::error(Event::Render, format!("Making a drape's texture failed: {error}"));
                }
                drape.clip_masks.update(gpu, context, false, &drape.groups.clip_tiles, |tile| {
                    to_projection_ubo(&transform.projection_data_for_tile(tile, false))
                });
            }
        }
        let overdraw = parameters.debug_options.contains(MapDebugOptions::OVERDRAW);
        for targets in &mut self.targets {
            let redrawn =
                self.drapes.iter().flat_map(|drape| &drape.stacks).filter(|drape| drape.redraws_at(targets.size));
            let composited: Vec<_> = redrawn.flat_map(|drape| drape.groups.composited.iter().copied()).collect();
            let size = Size::new(targets.size, targets.size);
            targets.layer_composite.update(gpu, parameters.context, size, Some(COLOR), overdraw, &composited);
        }
    }

    /// Draws each drape that draws again into its texture, through `gpu`,
    /// the context of `parameters`, the frame's, outside any pass, and then
    /// its texture's mips: its layers' opaque draws front to back, then
    /// their translucent ones back to front, over transparent texels. A
    /// drape with a layer that blends in the shader draws into the layer
    /// composite's frame texture, which is then copied into its own.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let formats = TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) };
        for Drape { tile, transform, stacks, .. } in &mut self.drapes {
            #[cfg_attr(not(test), expect(unused_variables, reason = "only tests record the passes' stacks"))]
            for (stack, drape) in stacks.iter_mut().enumerate().filter(|(_, drape)| drape.redraws) {
                let StackDrape { texture, clip_masks, layers, groups, drawn, key, .. } = drape;
                // A drape that fails to draw draws again in the next frame.
                *drawn = None;
                let Some((texture, side)) = texture.as_ref() else {
                    continue;
                };
                let Some(Targets { depth_stencil: Some(depth_stencil), layer_composite, .. }) =
                    self.targets.iter_mut().find(|targets| targets.size == *side)
                else {
                    continue;
                };
                let (depth_stencil, size) = (depth_stencil.id(), Size::new(*side, *side));
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
                    targets: formats,
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

#[cfg(test)]
mod tests {
    use mln_map::transform::Transform;
    use mln_map_types::{CameraOptions, ConstrainMode, ViewportMode};
    use mln_tile_id::CanonicalTileID;
    use mln_util::geo::LatLng;

    use super::*;

    /// A camera over the equator at zoom 10, pitched by `pitch` degrees,
    /// over a view of 1024 by 768 pixels.
    fn view(pitch: f64) -> View {
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(Size::new(1024, 768)).unwrap();
        transform.jump_to(&CameraOptions {
            center: Some(LatLng::new(0.0, 0.0).unwrap()),
            zoom: Some(10.0),
            pitch: Some(pitch),
            ..Default::default()
        });
        transform.view().clone()
    }

    /// The tile of zoom 9 `rows` north of the one south-east of the view's
    /// center.
    fn north_of_center(rows: u32) -> UnwrappedTileID {
        UnwrappedTileID::with_wrap(0, CanonicalTileID::new(9, 256, 256 - rows))
    }

    #[test]
    fn a_tile_seen_from_above_spans_its_side() {
        let view = view(0.0);
        let shown = shown_px(&view, &view.projection_matrix(), &north_of_center(0));
        // A tile of zoom 9 at zoom 10 spans 1024 pixels.
        assert!((shown - 1024.0).abs() < 1e-3, "{shown}");
        assert_eq!(2048, shown_size(shown, 1.0, 2048));
    }

    /// A pitched view's far tiles show smaller than its near ones, and
    /// their drapes have fewer texels.
    #[test]
    fn a_far_tile_has_a_smaller_drape_than_a_near_one() {
        let view = view(60.0);
        let projection = view.projection_matrix();
        let near = shown_px(&view, &projection, &north_of_center(0));
        let far = shown_px(&view, &projection, &north_of_center(3));
        assert!(far < near / 2.0, "near {near}, far {far}");
        let (near, far) = (shown_size(near, 1.0, 2048), shown_size(far, 1.0, 2048));
        assert_eq!(2048, near);
        assert!(far < near, "near {near}, far {far}");
        // More physical pixels a logical one take more texels.
        assert!(shown_size(shown_px(&view, &projection, &north_of_center(3)), 4.0, 2048) > far);
    }

    /// A tile that reaches under the camera, and behind the eye, shows
    /// larger than anywhere.
    #[test]
    fn a_tile_behind_the_eye_shows_at_the_largest_side() {
        let view = view(60.0);
        let under = UnwrappedTileID::with_wrap(0, CanonicalTileID::new(6, 32, 32));
        let shown = shown_px(&view, &view.projection_matrix(), &under);
        assert!(shown.is_infinite(), "{shown}");
        assert_eq!(2048, shown_size(shown, 1.0, 2048));
    }

    #[test]
    fn a_drape_takes_a_sixteenth_of_the_largest_side_at_least() {
        assert_eq!(128, shown_size(1.0, 1.0, 2048));
        assert_eq!(128, shown_size(0.0, 1.0, 2048));
        assert_eq!(256, shown_size(100.0, 1.0, 2048));
        assert_eq!(512, shown_size(129.0, 1.0, 2048));
        assert_eq!(2048, shown_size(f64::INFINITY, 1.0, 2048));
    }

    /// The cap lowers the largest side a step at a time, for the tiles that
    /// show smallest first: every tile takes a side before any takes the
    /// next one down.
    #[test]
    fn the_cap_lowers_the_sides_of_the_tiles_that_show_smallest_first() {
        // Three tiles, the first showing largest and the last smallest, with
        // two drapes each.
        let order = [0, 1, 2];
        let counts = [2, 2, 2];
        let capped = |cap: usize| {
            let mut sizes = [2048, 2048, 1024];
            cap_sizes(&mut sizes, &counts, &order, 128, cap);
            sizes
        };
        let held = |sizes: [u32; 3]| sizes.iter().map(|&size| 2 * texture_bytes(size)).sum::<usize>();
        assert_eq!([2048, 2048, 1024], capped(usize::MAX));
        assert_eq!([2048, 1024, 1024], capped(held([2048, 1024, 1024])));
        assert_eq!([1024, 1024, 1024], capped(held([2048, 1024, 1024]) - 1));
        assert_eq!([1024, 1024, 512], capped(held([1024, 1024, 1024]) - 1));
        assert_eq!([1024, 512, 512], capped(held([1024, 1024, 512]) - 1));
        // Never below the smallest side, though the cap then doesn't hold.
        assert_eq!([128, 128, 128], capped(0));
    }
}
