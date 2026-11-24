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
//! A drape of a stack that the terrain's surface lights holds a hillshade's
//! slopes, not its colors, which the surface makes of them with the frame's
//! lights: such a drape is the same at every bearing.
//!
//! A drape draws again only when what its layers would draw it from
//! differs from what they drew it from ([`DrapeKey`]); while the camera
//! of a continuous map moves, a drape drawn at another zoom stays, as in
//! GL JS, and at rest a continuous map draws those stale only by the
//! camera one tile a frame ([`Drapes::choose`]). A drape whose layers make
//! no draw has no texture, so a stack that draws nothing in a tile holds no
//! memory there.
//!
//! A drape's side follows its tile's on screen ([`shown_size`]): GL JS's
//! where the tile shows large, fewer texels where it shows small, and no
//! more than the device's largest texture. A drape of layers that show no
//! more with more texels takes no more than they have across its tile
//! ([`detail`]): a hillshade's, those of its DEM. A byte cap ([`BYTE_CAP`]) lowers
//! the sides of the tiles that show smallest where the drapes that hold a
//! texture, and what they draw with, would hold more; it never drops a
//! tile. [`Drapes::choose`] picks the drapes whose drawings the frame's
//! layers update, by their keys alone; the frame's sides are settled once
//! they have, which tells which drapes hold a texture
//! ([`Drapes::finish_update`]). A drape's drawings are always those of the
//! key it draws in the frame or of the key it was drawn from, and don't
//! depend on its side, whose uniforms are written as it uploads. A drape
//! whose side changes and that the frame didn't update thus draws again at
//! the new side from the drawings it has, keeping what it shows: at once
//! where it shrinks, so that the cap holds in every frame, and where it
//! grows, once the camera rests, in its tile's turn, as one drawn at
//! another zoom does.

use std::sync::Arc;

use geometry::Point;
use glam::{DMat4, DVec2, DVec4};
use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::shaders::layer_ubo::GlobalPaintParamsUBO;
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_map_types::{MapDebugOptions, TileProjector, View};
use mln_style::layer_type::ErasedLayerStyle;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::size::Size;
use smallvec::SmallVec;

use super::TerrainCover;
use crate::clip_masks::ClipMasks;
use crate::gfx::context::Context;
use crate::gfx::resources::{GpuTexture, attachment_bytes};
use crate::gfx::store::SharedTexture;
use crate::layer_composite::{CompositeTextures, LayerComposite};
use crate::layer_drawing::{ChangedBuckets, DrapeDrawing, DrawGroups, DrawOrder};
use crate::layer_type::ErasedEvaluatedLayer;
use crate::paint_parameters::{OpenPass, PaintParameters, StencilClear, TransformParameters};
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
/// pixels, on a device whose textures have at most `max_texture_size`
/// texels a side: twice as wide, whatever the pixel ratio (GL JS's quality
/// factor, the side of all of GL JS's drapes), or the device's largest.
pub(crate) fn drape_size(tile_size: u16, max_texture_size: u32) -> u32 {
    (u32::from(tile_size) * 2).min(max_texture_size).max(1)
}

/// How many times a drape's side may halve the most for its tile's size
/// on screen: to a sixteenth, 128 texels over a 512-pixel DEM.
const SMALLEST_SHIFT: u32 = 4;

/// The most bytes the drapes' textures hold, with their mips, and the
/// depth, stencil and layer composite they draw with, before the tiles
/// that show smallest take fewer texels: less on phones and in the
/// browser, whose GPU shares a smaller memory with the system.
pub(crate) const BYTE_CAP: u64 = if cfg!(any(
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
fn texture_bytes(size: u32) -> u64 {
    let bytes = attachment_bytes(Size::new(size, size), COLOR);
    bytes + bytes / 3
}

/// What the drapes of a tile hold, which the byte cap counts: for each of
/// them with a texture, the most texels a side it takes
/// ([`StackDrape::limit`]), and what the layer composite of its side holds
/// while it draws.
#[derive(Clone, Default)]
struct Held {
    drapes: SmallVec<[(u32, CompositeTextures); 4]>,
}

/// The bytes that the drapes of tiles of `sizes`, each holding what
/// `held` does, hold with what each side draws with: a depth and stencil,
/// and the layer composite. A drape takes its tile's side, or its own limit
/// where that is less.
fn held_bytes(sizes: &[u32], held: &[Held]) -> u64 {
    let mut sides: Vec<(u32, CompositeTextures)> = Vec::new();
    let mut bytes = 0;
    for (&size, held) in sizes.iter().zip(held) {
        for &(limit, drape) in &held.drapes {
            let size = side_within(size, limit);
            bytes += texture_bytes(size);
            match sides.iter_mut().find(|(side, _)| *side == size) {
                Some((_, composite)) => *composite = composite.union(drape),
                None => sides.push((size, drape)),
            }
        }
    }
    for (side, composite) in sides {
        let size = Size::new(side, side);
        bytes += attachment_bytes(size, DEPTH_STENCIL) + composite.bytes(size);
    }
    bytes
}

/// The side of a drape of a tile whose drapes draw at `size`, which takes at
/// most `limit` texels a side.
fn side_within(size: u32, limit: u32) -> u32 {
    size.min(limit).max(1)
}

/// The texels a side that hold all that a layer shows in the drape of
/// `drape`, from `tiles`, its tiles there that show something, each of
/// `texels` a side: those of its deepest tile across the drape's tile, to
/// the power of two that covers them, so that a drape of that side takes
/// each of the tile's texels as it is.
fn detail(texels: u32, drape: &OverscaledTileID, tiles: &[TileStamp]) -> u32 {
    let across = |tile: &OverscaledTileID| {
        let deeper = i32::from(tile.canonical.z) - i32::from(drape.canonical.z);
        let texels = u64::from(texels);
        if deeper >= 0 { texels << deeper.min(31) } else { texels >> (-deeper).min(63) }
    };
    let most = tiles.iter().filter(|stamp| stamp.bucket.is_some()).map(|stamp| across(&stamp.tile)).max();
    most.map_or(1, |texels| texels.clamp(1, 1 << 31) as u32).next_power_of_two()
}

/// The pieces each side of a tile is measured in on the globe, so that its
/// length on screen follows the side where the globe curves it.
const SIDE_PIECES: u32 = 8;

/// The logical pixels that `tile` spans in `view`, whose projection is
/// `projection`, level with the view's center: its longest side on
/// screen, as the view draws it, and infinite where it reaches behind the
/// eye, under the camera, where it shows larger than anywhere. On
/// Mercator, whose sides are straight, the side between two corners; on
/// the globe, the side through [`SIDE_PIECES`] pieces.
fn shown_px(view: &View, projection: &DMat4, tile: &UnwrappedTileID) -> f64 {
    let (altitude, size) = (view.center_altitude(), view.size());
    let half = DVec2::new(f64::from(size.width), f64::from(size.height)) / 2.0;
    if view.is_globe() {
        let projector = TileProjector::with_data(view, tile, view.projection_data_with(tile, projection));
        longest_side(SIDE_PIECES, |at| {
            let point = projector.project_as_drawn(&Point::new(at.x, at.y), altitude);
            let Point { x, y } = point.point;
            (point.signed_distance_from_camera > 0.0).then(|| DVec2::new(x, y) * half)
        })
    } else {
        let matrix = *projection * view.tile_matrix(tile);
        longest_side(1, |at| {
            let clip = matrix * DVec4::new(at.x, at.y, altitude, 1.0);
            (clip.w > 0.0).then(|| DVec2::new(clip.x, clip.y) / clip.w * half)
        })
    }
}

/// The length of the longest side of a tile, each through `pieces` pieces
/// between the points `drawn` draws at, in tile units; infinite where it
/// draws one of them nowhere.
fn longest_side(pieces: u32, drawn: impl Fn(DVec2) -> Option<DVec2>) -> f64 {
    let extent = f64::from(EXTENT);
    let corners = [DVec2::ZERO, DVec2::new(extent, 0.0), DVec2::splat(extent), DVec2::new(0.0, extent)];
    let [Some(a), Some(b), Some(c), Some(d)] = corners.map(&drawn) else {
        return f64::INFINITY;
    };
    let mut longest = 0.0f64;
    for (k, (from, to)) in [(a, b), (b, c), (c, d), (d, a)].into_iter().enumerate() {
        let (mut length, mut last) = (0.0, from);
        for piece in 1..pieces {
            let at = corners[k].lerp(corners[(k + 1) % 4], f64::from(piece) / f64::from(pieces));
            let Some(point) = drawn(at) else {
                return f64::INFINITY;
            };
            length += last.distance(point);
            last = point;
        }
        longest = longest.max(length + last.distance(to));
    }
    longest
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
    (texels.ceil() as u32).next_power_of_two().clamp((largest >> SMALLEST_SHIFT).max(1), largest)
}

/// Lowers `sizes`, each the side of the drapes of a tile that hold what
/// `held` does, until they hold at most `cap` bytes ([`held_bytes`]): the
/// largest side allowed falls a step at a time, for the tiles that show
/// smallest first, which `order` lists last, down to a texel, below the
/// smallest side a tile's size on screen gives where the cap needs it.
fn cap_sizes(sizes: &mut [u32], held: &[Held], order: &[usize], cap: u64) {
    let mut limit = sizes.iter().copied().max().unwrap_or(0);
    while held_bytes(sizes, held) > cap && limit > 1 {
        let lower = limit / 2;
        for &tile in order.iter().rev() {
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

/// The tile of `drapes` whose drapes take their turn in a frame that takes
/// turns, of those that `wait`: the one drawn longest ago, the nearest the
/// center of those, so that a camera that jumps every frame draws each tile
/// in turn.
fn turn(drapes: &[Drape], turns: bool, wait: impl Fn(&Drape) -> bool) -> Option<usize> {
    (drapes.iter().enumerate())
        .filter(|&(_, drape)| turns && wait(drape))
        .min_by_key(|&(index, drape)| (drape.drawn_at, index))
        .map(|(index, _)| index)
}

/// A drawing of a layer that a drape keeps, with the style it was made for.
struct DrapeLayer {
    style: Arc<dyn ErasedLayerStyle>,
    drawing: DrapeDrawing,
}

/// What a draped layer draws a drape with.
#[cfg_attr(test, derive(Clone))]
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
#[cfg_attr(test, derive(Clone))]
struct DrapeKey {
    /// The draped layers, in drawing order.
    layers: Vec<LayerKey>,
    /// The layers' tiles, each layer's after the one before's.
    tiles: Vec<TileStamp>,
    zoom: f64,
    /// The map's bearing, which a layer draws by, or `None`.
    bearing: Option<f64>,
    /// Whether the terrain's surface lights the drape, whose layer then
    /// draws what the surface lights, not its colors.
    lit: bool,
}

/// How a drape's key differs from the one it was drawn from, least to most
/// severe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Difference {
    None,
    /// Only by the camera: the zoom, or the layers' values evaluated at
    /// another zoom.
    Camera,
    /// By what the drape shows: its layers, their styles, paint revisions
    /// or tiles, or the bearing a layer draws by, which turns what the layer
    /// draws in every drape of the view at once.
    Content,
}

impl DrapeKey {
    /// How the texels this key would draw differ from those of a drape
    /// drawn from `drawn`: by its content unless it has the same layers,
    /// with the same styles and paint revisions, from the same tiles, at
    /// the same bearing if a layer draws by it, and else by the camera
    /// unless they have the same values at the same zoom.
    fn difference(&self, drawn: &DrapeKey) -> Difference {
        let same_content = self.tiles == drawn.tiles
            && self.bearing == drawn.bearing
            && self.lit == drawn.lit
            && self.layers.len() == drawn.layers.len()
            && self.layers.iter().zip(&drawn.layers).all(|(layer, drawn)| {
                Arc::ptr_eq(&layer.style, &drawn.style)
                    && layer.paint_revision == drawn.paint_revision
                    && layer.tiles == drawn.tiles
            });
        let same_camera = self.zoom == drawn.zoom
            && self
                .layers
                .iter()
                .zip(&drawn.layers)
                .all(|(layer, drawn)| Arc::ptr_eq(&layer.evaluated, &drawn.evaluated));
        match (same_content, same_camera) {
            (false, _) => Difference::Content,
            (true, false) => Difference::Camera,
            (true, true) => Difference::None,
        }
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
    /// Whether the frame's layers update its drawings, which it then draws
    /// from its key.
    updated: bool,
    /// Whether the frame draws it again into its texture: updated, where
    /// its layers draw something, or at a new side from the drawings it
    /// has.
    redraws: bool,
    /// How its key differs from the one it was drawn from.
    difference: Difference,
    /// The most texels a side its texture takes: those that hold all its
    /// layers show in its tile ([`RenderLayer::drape_texels`]), where each
    /// of them shows no more.
    limit: u32,
    /// What the layer composite holds while its layers draw it, as they
    /// last drew it.
    composite: CompositeTextures,
    /// The global paint values of the frame that last updated its
    /// drawings, which its draws read with those of its side.
    paint: GlobalPaintParamsUBO,
    /// The key its drawings were made from.
    #[cfg(test)]
    drawings: Option<DrapeKey>,
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
        self.redraws && self.side() == Some(side)
    }

    /// The side of its texture, if it has one.
    fn side(&self) -> Option<u32> {
        self.texture.as_ref().map(|&(_, side)| side)
    }

    /// The side its texture takes in a tile whose drapes draw at `size`.
    fn side_in(&self, size: u32) -> u32 {
        side_within(size, self.limit)
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
            updated: false,
            redraws: false,
            difference: Difference::Content,
            limit: u32::MAX,
            composite: CompositeTextures::default(),
            paint: GlobalPaintParamsUBO::default(),
            #[cfg(test)]
            drawings: None,
        }
    }
}

/// The drapes of a terrain tile, one for each stack.
struct Drape {
    tile: OverscaledTileID,
    transform: TransformParameters,
    /// The logical pixels the tile spans on screen ([`shown_px`]).
    shown: f64,
    /// The side in texels its drapes draw at in the frame, each up to its
    /// limit ([`StackDrape::side_in`]), which [`Drapes::finish_update`]
    /// settles.
    size: u32,
    /// The frame that last updated one of its drapes, or gave them their
    /// turn ([`turn`]).
    drawn_at: u64,
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
    /// Whether the frame's view is on the globe, where a drape draws the
    /// tiles over its own whatever their world copy.
    globe: bool,
    /// The most texels a side of a drape ([`drape_size`]).
    largest: u32,
    /// The physical pixels of the frame's target a logical one.
    pixel_ratio: f32,
    /// The most bytes the drapes and what they draw with hold before the
    /// tiles that show smallest take fewer texels.
    byte_cap: u64,
    /// What the drapes draw with, for each side that a drape's texture and
    /// its tile have.
    targets: Vec<Targets>,
    /// The frames that [`choose`](Self::choose) chose drapes for.
    frame: u64,
    /// Whether the frame's camera is at rest, as the drapes take it, and
    /// whether it draws the drapes stale by the camera one tile a frame
    /// ([`choose`](Self::choose)).
    at_rest: bool,
    paced: bool,
    /// Whether [`choose`](Self::choose) gave a tile its turn in the frame.
    turn_taken: bool,
    /// Whether drapes stale by the camera only, or smaller than their
    /// tile's side, wait for a later frame.
    waiting: bool,
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
    /// the side its tile shows at ([`shown_size`]), at most
    /// `max_texture_size`, which [`finish_update`](Self::finish_update)
    /// lowers where they would hold more than `byte_cap` bytes. The frame's
    /// draped layers then give their keys ([`key_layer`](Self::key_layer)),
    /// which [`choose`](Self::choose) the drapes whose drawings they then
    /// update ([`update_layer`](Self::update_layer)). The layers draw into
    /// the drapes as on Mercator, on the globe too ([`View::mercator`]), as
    /// GL JS draws its drapes.
    pub(crate) fn begin(
        &mut self,
        (view, projection, pixel_ratio): (&View, &DMat4, f32),
        cover: &TerrainCover,
        stacks: usize,
        (byte_cap, max_texture_size): (u64, u32),
    ) {
        let largest = drape_size(cover.tile_size, max_texture_size);
        if largest != self.largest {
            *self = Drapes { largest, ..Drapes::default() };
        }
        (self.stacks, self.globe) = (stacks, view.is_globe());
        (self.byte_cap, self.pixel_ratio) = (byte_cap, pixel_ratio);
        let mut kept = std::mem::take(&mut self.drapes);
        self.drapes.reserve(cover.tiles.len());
        let flat = view.mercator();
        for tile in &cover.tiles {
            let unwrapped = tile.id.to_unwrapped();
            let transform = TransformParameters::for_drape(&flat, &unwrapped);
            let shown = shown_px(view, projection, &unwrapped);
            let mut drape = match kept.iter().position(|drape| drape.tile == tile.id) {
                Some(index) => Drape { transform, shown, ..kept.swap_remove(index) },
                None => Drape { tile: tile.id, transform, shown, size: 0, drawn_at: 0, stacks: Vec::new() },
            };
            drape.stacks.resize_with(stacks, StackDrape::new);
            for drape in &mut drape.stacks {
                drape.key.layers.clear();
                drape.key.tiles.clear();
                drape.key.zoom = view.zoom();
                drape.key.bearing = None;
                drape.key.lit = false;
                drape.limit = 0;
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
    /// come in drawing order. The surface lights the stack if `lit`: its
    /// layer then drapes the same at every bearing.
    pub(crate) fn key_layer(
        &mut self,
        stack: usize,
        layer: &dyn RenderLayer,
        source: Option<LayerSource<'_>>,
        lit: bool,
    ) {
        let base = layer.base();
        for drape in &mut self.drapes {
            let Some(StackDrape { key, limit, .. }) = drape.stacks.get_mut(stack) else {
                continue;
            };
            let start = key.tiles.len();
            layer.drape_stamps(source, (drape.tile, self.globe), &mut key.tiles);
            let texels = layer.drape_texels();
            *limit = (*limit).max(texels.map_or(u32::MAX, |texels| detail(texels, &drape.tile, &key.tiles[start..])));
            key.lit = lit;
            if layer.drapes_by_bearing() && !lit {
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

    /// Chooses the drapes whose drawings the frame's layers update, once
    /// every draped layer gave its key: those that draw again from their
    /// key. Their sides don't come into it:
    /// [`finish_update`](Self::finish_update) settles them.
    ///
    /// A drape not drawn yet or stale, or whose key differs from the one it
    /// was drawn from by its content ([`DrapeKey::difference`]), draws again
    /// at once. With the camera `at_rest`, and in a still, which isn't
    /// `paced`, a drape whose key differs only by the camera draws again
    /// too: `paced`, in a continuous map, only the drapes of one tile a
    /// frame, which takes its [`turn`], and those of the tiles whose content
    /// changed, as in GL JS. The others [wait](Self::waiting) for a later
    /// frame. While the camera of a continuous map moves, a drape drawn at
    /// another zoom stays. A drape that a layer draws by the bearing differs
    /// by its content at another bearing, so it draws again in every frame
    /// that turns the map.
    pub(crate) fn choose(&mut self, at_rest: bool, paced: bool) {
        let at_rest = at_rest || !paced;
        (self.at_rest, self.paced) = (at_rest, paced);
        self.frame += 1;
        for drape in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
            drape.difference = drape.drawn.as_ref().map_or(Difference::Content, |drawn| drape.key.difference(drawn));
            drape.updated = drape.difference == Difference::Content;
        }
        let updated = |drape: &Drape| drape.stacks.iter().any(|drape| drape.updated);
        let stale = |drape: &Drape| drape.stacks.iter().any(|drape| drape.difference == Difference::Camera);
        let next = turn(&self.drapes, paced && at_rest, |drape| !updated(drape) && stale(drape));
        self.turn_taken = next.is_some();
        for (index, drape) in self.drapes.iter_mut().enumerate() {
            let now = at_rest && (!paced || next == Some(index) || updated(drape));
            for drape in &mut drape.stacks {
                drape.updated |= now && drape.difference == Difference::Camera;
                if drape.updated {
                    drape.previous = std::mem::take(&mut drape.layers);
                }
            }
            if updated(drape) {
                drape.drawn_at = self.frame;
            }
        }
    }

    /// Sets the side of each tile's drapes: the one its tile shows at,
    /// lowered where the drapes that `hold` a texture, and what they draw
    /// with, would hold more than the byte cap ([`cap_sizes`]).
    fn cap(&mut self, hold: impl Fn(&StackDrape) -> bool) {
        let held: Vec<Held> = (self.drapes.iter())
            .map(|drape| {
                let held = drape.stacks.iter().filter(|drape| hold(drape));
                Held { drapes: held.map(|drape| (drape.limit, drape.composite)).collect() }
            })
            .collect();
        let mut order: Vec<usize> = (0..self.drapes.len()).collect();
        order.sort_by(|&a, &b| self.drapes[b].shown.total_cmp(&self.drapes[a].shown));
        let (pixel_ratio, largest) = (self.pixel_ratio, self.largest);
        let mut sizes: Vec<u32> =
            self.drapes.iter().map(|drape| shown_size(drape.shown, pixel_ratio, largest)).collect();
        cap_sizes(&mut sizes, &held, &order, self.byte_cap);
        for (drape, size) in self.drapes.iter_mut().zip(sizes) {
            drape.size = size;
        }
    }

    /// Whether the last frame left drapes stale by the camera only, or
    /// smaller than their tile's side, for a later frame to draw
    /// ([`choose`](Self::choose), [`finish_update`](Self::finish_update)):
    /// the map then isn't done drawing.
    pub(crate) fn waiting(&self) -> bool {
        self.waiting
    }

    /// Makes the draws of `layer`, a layer of `stack` that drapes, in every
    /// drape of the stack that [`choose`](Self::choose) chose, with the
    /// drape's `parameters`, from its `source`. The stack's layers update in
    /// drawing order. The surface lights the stack if `lit`.
    #[expect(clippy::too_many_arguments, reason = "the frame's GPU and atlases, and the layer in its stack")]
    pub(crate) fn update_layer(
        &mut self,
        stack: usize,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        atlases: &Atlases,
        layer: &mut dyn RenderLayer,
        source: Option<LayerSource<'_>>,
        lit: bool,
    ) {
        let style = Arc::clone(&layer.base().style);
        // The layers' draw updates don't read the composite, which only
        // the passes do, nor the drape's size, which `finish_update`
        // settles.
        let layer_composite = &mut LayerComposite::default();
        let (size, globe) = (Size::new(self.largest, self.largest), self.globe);
        for Drape { tile, transform, stacks, .. } in &mut self.drapes {
            let Some(StackDrape { clip_masks, layers, previous, .. }) =
                stacks.get_mut(stack).filter(|drape| drape.updated)
            else {
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
            let mut parameters = parameters.drape(transform, clip_masks, layer_composite, size, tile.to_unwrapped());
            parameters.drape_lit = lit;
            layer.update_drape_draws(gpu, &mut parameters, atlases, source, (*tile, globe), &mut kept.drawing);
            layers.push(kept);
        }
    }

    /// Ends the frame's update of the drapes and settles their sides, now
    /// that it is known which of them hold a texture. The drawings of layers
    /// that no longer drape in their stack go. A drape the frame's layers
    /// updated whose layers draw nothing has no texture and draws no pass,
    /// drawn from its key.
    ///
    /// Each tile's drapes then take the side its tile shows at, lowered
    /// where the drapes that hold a texture after the frame, and what they
    /// draw with, would hold more than the byte cap ([`cap_sizes`]); a still
    /// thus draws its drapes at the sides the same still drawn again would.
    /// A drape updated that draws something draws at its tile's side, into
    /// a texture of that side, made in `context`. A drape not updated whose
    /// texture has another side draws again at the new side from the
    /// drawings it has, which are those of the key it was drawn from, so
    /// that what it shows stays and only its texels change: at once where
    /// its texture is larger, so that the cap holds; where it is smaller,
    /// with the camera at rest, in a tile whose drapes the frame updated, or
    /// in the tile's [`turn`] if `choose` gave none, as a drape stale by the
    /// camera draws.
    pub(crate) fn finish_update(&mut self, context: &mut Context) {
        for drape in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
            drape.previous.clear();
            drape.redraws = false;
            if !drape.updated {
                continue;
            }
            #[cfg(test)]
            {
                drape.drawings = Some(drape.key.clone());
            }
            let (mut draws, mut composited) = (false, Vec::new());
            for layer in &drape.layers {
                layer.drawing.visit_groups(&mut |group| {
                    draws = draws || group.draws();
                    composited.extend(group.composite().map(|(_, composite)| composite));
                });
            }
            drape.composite = CompositeTextures::of(composited, Some(COLOR));
            drape.redraws = draws;
            if !draws {
                drape.texture = None;
                drape.drawn = Some(std::mem::take(&mut drape.key));
            }
        }
        self.cap(|drape| drape.redraws || drape.texture.is_some());
        let (at_rest, paced, frame) = (self.at_rest, self.paced, self.frame);
        let grows = |drape: &Drape| {
            (drape.stacks.iter())
                .any(|stack| !stack.updated && stack.side().is_some_and(|side| side < stack.side_in(drape.size)))
        };
        let turns = paced && at_rest && !self.turn_taken;
        if let Some(index) = turn(&self.drapes, turns, |drape| drape.drawn_at != frame && grows(drape)) {
            self.drapes[index].drawn_at = frame;
        }
        self.waiting = false;
        for Drape { size, drawn_at, stacks, .. } in &mut self.drapes {
            let now = at_rest && (!paced || *drawn_at == frame);
            for drape in stacks.iter_mut() {
                let size = drape.side_in(*size);
                if let Some(side) = drape.side().filter(|_| !drape.updated) {
                    drape.redraws = side > size || (now && side < size);
                }
                if drape.redraws && drape.side() != Some(size) {
                    drape.texture = Some((drape_texture(context, size), size));
                }
                let stale = !drape.updated && drape.difference == Difference::Camera;
                self.waiting |= at_rest && (stale || drape.side().is_some_and(|side| side < size));
            }
        }
        #[cfg(test)]
        self.assert_drawings();
    }

    /// Asserts that each drape's drawings are those of the key it draws in
    /// the frame, where the frame updated them, and else those of the key
    /// it was drawn from.
    #[cfg(test)]
    fn assert_drawings(&self) {
        for (tile, drape) in self.drapes.iter().flat_map(|drape| drape.stacks.iter().map(|stack| (drape.tile, stack))) {
            let key = if drape.updated && drape.redraws { Some(&drape.key) } else { drape.drawn.as_ref() };
            let same = key
                .zip(drape.drawings.as_ref())
                .is_some_and(|(key, drawings)| key.difference(drawings) == Difference::None);
            assert!(same, "the drawings of a drape of {tile:?} aren't those of its key");
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
    /// their keys don't tell, such as the style's images, or the frame
    /// that drew them failed.
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
                bytes += texture.bytes() as u64;
            }
        }
        let depth_stencil: u64 = (self.targets.iter())
            .filter(|targets| targets.depth_stencil.is_some())
            .map(|targets| attachment_bytes(Size::new(targets.size, targets.size), DEPTH_STENCIL))
            .sum();
        let composite: u64 = self.targets.iter().map(|targets| targets.layer_composite.bytes()).sum();
        let stats = &mut context.rendering_stats().terrain;
        let usize = |bytes: u64| usize::try_from(bytes).unwrap_or(usize::MAX);
        stats.stacks = self.stacks;
        stats.drapes = drapes;
        stats.mem_drapes = usize(bytes);
        stats.mem_drape_depth_stencil = usize(depth_stencil);
        stats.mem_drape_composite = usize(composite);
    }

    /// The tiles and stacks of the drapes drawn since the last call.
    #[cfg(test)]
    pub(crate) fn take_passes(&mut self) -> Vec<(OverscaledTileID, usize)> {
        std::mem::take(&mut self.passes)
    }

    /// Makes the textures of the drapes that draw again and what they draw
    /// with, and uploads the GPU objects of their draws, with the uniforms
    /// of their side and of a pattern atlas of `pattern_atlas_size`, and the
    /// masks of the tiles they clip to, through `gpu`, the context of
    /// `parameters`, the frame's. The drapes of a side share what they draw
    /// with, kept while a drape's texture has that side and its tile's: a
    /// depth and stencil, made once one of them draws again, and a layer
    /// composite, which takes the composited draw groups of those that draw
    /// again, and drops its textures where there are none.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, pattern_atlas_size: Size) {
        if self.drapes.is_empty() {
            self.targets.clear();
            return;
        }
        // A drape that waits to grow draws at its tile's side once it does,
        // never at its texture's, so what that side draws with goes when
        // only such drapes have it, as the cap counts them at their tile's.
        let sides: Vec<u32> = (self.drapes.iter())
            .flat_map(|drape| {
                (drape.stacks.iter()).filter_map(|stack| stack.side().filter(|&side| side == stack.side_in(drape.size)))
            })
            .collect();
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
            // The drapes of the side then fail to draw.
            match parameters.context.device().attachment(gpu, "drape depth and stencil", size, DEPTH_STENCIL) {
                Ok(depth_stencil) => targets.depth_stencil = Some(depth_stencil),
                Err(error) => parameters.context.fail(format!("Making the drapes' depth and stencil failed: {error}")),
            }
        }
        for Drape { tile, transform, size, stacks, .. } in &mut self.drapes {
            for drape in stacks.iter_mut().filter(|drape| drape.redraws) {
                let size = drape.side_in(*size);
                let size = Size::new(size, size);
                let composite = &mut LayerComposite::default();
                let drape_parameters =
                    parameters.drape(transform, &mut drape.clip_masks, composite, size, tile.to_unwrapped());
                let frame = drape_parameters.global_paint_params(pattern_atlas_size);
                let stencil_clear = drape_parameters.stencil_clear_tiles(&[tile.to_unwrapped()]);
                let context = &mut *parameters.context;
                // The drawings read the camera of the frame that updated
                // them, with the drape's side and the frame's atlas.
                if drape.updated {
                    drape.paint = frame;
                }
                let global = GlobalPaintParamsUBO {
                    pattern_atlas_texsize: frame.pattern_atlas_texsize,
                    world_size: frame.world_size,
                    ..drape.paint
                };
                for layer in &mut drape.layers {
                    layer.drawing.write_uniform(gpu, ID_GLOBAL_PAINT_PARAMS_UBO, &global, context);
                    layer.drawing.upload(gpu, context);
                }
                if let Some((texture, _)) = &drape.texture
                    && let Err(error) = context.create_texture(gpu, texture)
                {
                    context.fail(format!("Making a drape's texture failed: {error}"));
                }
                // A drape drawn again only at a new side keeps the groups and
                // masks of its drawings.
                if !drape.updated {
                    continue;
                }
                let layers = &drape.layers;
                drape.groups = DrawGroups::of(
                    |visit| {
                        for layer in layers {
                            layer.drawing.visit_groups(visit);
                        }
                    },
                    |tiles| if tiles.is_empty() { StencilClear::new() } else { stencil_clear },
                );
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
    /// composite's frame texture, which is then copied into its own. A
    /// drape that fails to draw, its pass or a draw in it, fails the frame
    /// ([`Context::fail`](crate::gfx::context::Context::fail)), and every
    /// drape of a frame that failed draws again
    /// ([`redraw`](Self::redraw)).
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let formats = TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) };
        for Drape { tile, transform, stacks, .. } in &mut self.drapes {
            #[cfg_attr(not(test), expect(unused_variables, reason = "only tests record the passes' stacks"))]
            for (stack, drape) in stacks.iter_mut().enumerate().filter(|(_, drape)| drape.redraws) {
                let StackDrape { texture, clip_masks, layers, groups, drawn, key, updated, .. } = drape;
                // A drape that fails to draw draws again in the next frame.
                // One that draws again only at a new side keeps the key it was
                // drawn from.
                let kept = drawn.take();
                let Some((texture, side)) = texture.as_ref() else {
                    parameters.context.fail("Drawing a drape failed: it has no texture".to_owned());
                    continue;
                };
                // Without its side's depth and stencil, the upload said why.
                let Some(Targets { depth_stencil: Some(depth_stencil), layer_composite, .. }) =
                    self.targets.iter_mut().find(|targets| targets.size == *side)
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
                        *drawn = if *updated { Some(std::mem::take(key)) } else { kept };
                        parameters.context.rendering_stats().terrain.drapes_drawn += 1;
                        #[cfg(test)]
                        self.passes.push((*tile, stack));
                    }
                    Err(error) => parameters.context.fail(format!("Drawing a drape failed: {error}")),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mln_map::transform::Transform;
    use mln_map_types::{CameraOptions, ConstrainMode, ViewportMode};
    use mln_style_types::ProjectionDefinition;
    use mln_tile_id::CanonicalTileID;
    use mln_util::geo::LatLng;
    use mln_util::identity::SimpleIdentity;
    use smallvec::smallvec;

    use super::*;
    use crate::layer_composite::Composite;

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

    /// A view of the globe at `zoom` over 512 by 512 pixels.
    fn globe(center: LatLng, zoom: f64) -> View {
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(Size::new(512, 512)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        transform.jump_to(&CameraOptions { center: Some(center), zoom: Some(zoom), ..Default::default() });
        assert!(transform.view().is_globe());
        transform.view().clone()
    }

    /// On the globe, a drape draws its layers as on Mercator: its view isn't
    /// the globe's, and its tiles project through the Mercator matrix onto
    /// its texture, as GL JS draws its drapes.
    #[test]
    fn a_globe_drape_draws_on_mercator() {
        let view = globe(LatLng::new(30.0, 20.0).unwrap(), 3.0);
        let id = OverscaledTileID::new(3, 0, CanonicalTileID::new(3, 4, 3));
        let cover = TerrainCover {
            source_id: "dem".to_owned(),
            tile_size: 512,
            tiles: vec![crate::terrain::TerrainTile { id, dem: None, dem_loading: false }],
            center: None,
            dem_tiles: Vec::new(),
            standing_dems: Vec::new(),
            ranges: crate::terrain::ElevationRanges::default(),
        };
        let mut drapes = Drapes::default();
        drapes.begin((&view, &view.projection_matrix(), 1.0), &cover, 1, (BYTE_CAP, 4096));
        let transform = &drapes.drapes[0].transform;
        assert!(!transform.view.is_globe());
        let tile = id.to_unwrapped();
        let projection = transform.projection_data_for_tile(&tile, false);
        let mercator = TransformParameters::for_drape(&view.mercator(), &tile);
        assert_eq!(mercator.proj_matrix * view.mercator().tile_matrix(&tile), projection.main_matrix);
        assert_eq!(0.0, projection.projection_transition);
    }

    /// On the globe, a tile's sides curve: the tile of zoom 0, whose corners
    /// are close together behind the planet, shows about as wide as the
    /// planet, and its drape takes GL JS's side.
    #[test]
    fn a_tile_on_the_globe_spans_its_curved_sides() {
        let view = globe(LatLng::new(0.0, 0.0).unwrap(), 1.0);
        let shown = shown_px(&view, &view.projection_matrix(), &UnwrappedTileID::new(0, 0, 0));
        // The planet is 1024 / π pixels across at zoom 1.
        let planet = 1024.0 / std::f64::consts::PI;
        assert!(shown > planet / 2.0, "{shown}");
        assert_eq!(1024, shown_size(shown, 1.0, 1024));
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
        assert_eq!(1, shown_size(0.0, 1.0, 8));
    }

    /// What a tile's two drapes hold, which take their tile's side and
    /// don't composite.
    fn two_drapes() -> Held {
        Held { drapes: smallvec![(u32::MAX, CompositeTextures::default()); 2] }
    }

    /// Three tiles, the first showing largest and the last smallest, with two
    /// drapes each, of `sizes`, lowered under `cap`.
    fn capped(cap: u64) -> [u32; 3] {
        let mut sizes = [2048, 2048, 1024];
        cap_sizes(&mut sizes, &vec![two_drapes(); 3], &[0, 1, 2], cap);
        sizes
    }

    /// The bytes of three tiles' two drapes each, of `sizes`, and of the
    /// depth and stencil of each side.
    fn held(sizes: [u32; 3]) -> u64 {
        held_bytes(&sizes, &vec![two_drapes(); 3])
    }

    /// The cap lowers the largest side a step at a time, for the tiles that
    /// show smallest first: every tile takes a side before any takes the
    /// next one down.
    #[test]
    fn the_cap_lowers_the_sides_of_the_tiles_that_show_smallest_first() {
        assert_eq!([2048, 2048, 1024], capped(u64::MAX));
        assert_eq!([2048, 1024, 1024], capped(held([2048, 1024, 1024])));
        assert_eq!([1024, 1024, 1024], capped(held([2048, 1024, 1024]) - 1));
        assert_eq!([1024, 1024, 512], capped(held([1024, 1024, 1024]) - 1));
        assert_eq!([1024, 512, 512], capped(held([1024, 1024, 512]) - 1));
    }

    /// The cap counts a depth and stencil for each side the drapes have, and
    /// the layer composite's textures where a tile's drapes composite.
    #[test]
    fn the_cap_counts_what_each_side_draws_with() {
        let side = |size: u32| attachment_bytes(Size::new(size, size), COLOR);
        assert_eq!(
            4 * texture_bytes(2048) + 2 * texture_bytes(1024) + side(2048) + side(1024),
            held([2048, 2048, 1024])
        );
        let composite =
            Composite { opacity: 0.5, blend: mln_style_types::LayerBlendType::Multiply, frame_depth: false };
        let composite = CompositeTextures::of([composite], Some(COLOR));
        let held = [Held { drapes: smallvec![(u32::MAX, composite)] }, Held::default()];
        // The drape, its depth and stencil, and the composite's color, depth
        // and stencil, frame and backdrop.
        assert_eq!(texture_bytes(2048) + 5 * side(2048), held_bytes(&[2048, 2048], &held));
    }

    /// A drape with a limit takes its tile's side only up to it, and the cap
    /// counts it, and the depth and stencil of its side, at that.
    #[test]
    fn the_cap_counts_a_drape_at_its_limit() {
        let side = |size: u32| attachment_bytes(Size::new(size, size), COLOR);
        let held =
            [Held { drapes: smallvec![(u32::MAX, CompositeTextures::default()), (512, CompositeTextures::default())] }];
        assert_eq!(texture_bytes(2048) + texture_bytes(512) + side(2048) + side(512), held_bytes(&[2048], &held));
        assert_eq!(2 * texture_bytes(256) + side(256), held_bytes(&[256], &held));
    }

    /// A layer whose tiles hold `texels` a side shows no more in a drape
    /// than its deepest tile there has across the drape's tile.
    #[test]
    fn a_drape_holds_the_texels_of_its_deepest_tile() {
        let tile = |z: u8| TileStamp {
            tile: OverscaledTileID::new(z, 0, CanonicalTileID::new(z, 0, 0)),
            bucket: Some(SimpleIdentity::new()),
            generations: [0, 0],
        };
        let drape = tile(4).tile;
        assert_eq!(512, detail(512, &drape, &[tile(4)]));
        assert_eq!(1024, detail(512, &drape, &[tile(4), tile(5)]), "two tiles of 512 texels across it");
        assert_eq!(128, detail(512, &drape, &[tile(2)]), "a quarter of an ancestor's side");
        assert_eq!(512, detail(257, &drape, &[tile(4)]), "the power of two that covers them");
        assert_eq!(1, detail(512, &drape, &[]));
        // A tile that shows nothing takes no texels.
        assert_eq!(256, detail(512, &drape, &[tile(3), TileStamp { bucket: None, ..tile(6) }]));
        assert_eq!(1 << 31, detail(4096, &drape, &[tile(30)]));
    }

    /// Where the drapes at the smallest side their tiles' size on screen
    /// gives hold more than the cap, the cap lowers them further, to a
    /// texel.
    #[test]
    fn the_cap_lowers_the_sides_below_the_smallest_where_it_needs() {
        assert_eq!(128, shown_size(0.0, 1.0, 2048));
        assert_eq!([128, 128, 64], capped(held([128, 128, 128]) - 1));
        assert_eq!([1, 1, 1], capped(0));
    }
}
