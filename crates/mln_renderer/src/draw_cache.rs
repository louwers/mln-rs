//! What the renderer keeps of each layer's drawing between frames, and the
//! draws of a frame.
//!
//! The orchestrator owns a [`LayerDraws`] for each render layer and lends it
//! to the layer's calls. Each tile's entry says which bucket the tile's draws
//! were built from, and what of that bucket's vertex data changed since, as
//! the frame's prepare reports it. A layer keeps the GPU objects of its
//! draws in caches of state of its own type ([`DrawCache`]): one in each
//! tile's entry, and one of the layer's draws that no tile holds. Its update
//! leaves the frame's draws, as values ([`FrameDraw`]), in
//! [`LayerDraws::frame`] and their uniforms in the layer's buffers, and the
//! frame's encode issues them ([`DrawGroup`]).

use std::any::Any;
use std::ops::BitOrAssign;
use std::rc::Rc;

use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;

use crate::draw::DrawObjects;
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::layer_composite::Composite;
use crate::layers::render_custom_layer::CustomLayerDraw;
use crate::paint_parameters::PaintParameters;
use crate::render_pass::RenderPass;

/// The order of a layer's draws that its draws' ids don't give: lower
/// priorities draw first.
pub(crate) type DrawPriority = i64;

/// What of a bucket's vertex data changed since a tile's draws copied it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BucketChanges {
    /// The values of the paint properties that read feature state.
    pub(crate) paint: bool,
    /// A symbol bucket's opacities, which placement sets.
    pub(crate) opacity: bool,
    /// A symbol bucket's positions of labels along lines or at variable
    /// anchors.
    pub(crate) dynamic: bool,
    /// The order of a symbol bucket's triangles.
    pub(crate) sort: bool,
}

impl BucketChanges {
    pub(crate) const ALL: BucketChanges = BucketChanges { paint: true, opacity: true, dynamic: true, sort: true };

    /// Whether anything changed.
    #[must_use]
    pub(crate) fn any(self) -> bool {
        self != BucketChanges::default()
    }
}

impl BitOrAssign for BucketChanges {
    fn bitor_assign(&mut self, other: BucketChanges) {
        self.paint |= other.paint;
        self.opacity |= other.opacity;
        self.dynamic |= other.dynamic;
        self.sort |= other.sort;
    }
}

/// The buckets whose vertex data changed, by bucket id, with what changed.
#[derive(Default)]
pub(crate) struct ChangedBuckets(HashMap<SimpleIdentity, BucketChanges>);

impl ChangedBuckets {
    /// Records that `changes` of the bucket `bucket` changed.
    pub(crate) fn mark(&mut self, bucket: SimpleIdentity, changes: BucketChanges) {
        if changes.any() {
            *self.0.entry(bucket).or_default() |= changes;
        }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn get(&self, bucket: SimpleIdentity) -> Option<BucketChanges> {
        self.0.get(&bucket).copied()
    }
}

/// What a layer type keeps of its draws between frames, of a tile or of the
/// layer: the GPU objects of the draws, and its own state of them.
pub(crate) trait DrawCache: Any {
    /// The GPU objects of the draw `id`.
    fn objects_mut(&mut self, id: SimpleIdentity) -> Option<&mut DrawObjects>;
}

/// The GPU objects of one draw a layer keeps, with the id that draws bind
/// them by. Ids are made with the objects, so they order the draws by when
/// their objects were made.
pub(crate) struct KeptDraw {
    pub(crate) id: SimpleIdentity,
    pub(crate) objects: DrawObjects,
}

impl KeptDraw {
    #[must_use]
    pub(crate) fn new(objects: DrawObjects) -> KeptDraw {
        KeptDraw { id: SimpleIdentity::new(), objects }
    }
}

impl DrawCache for KeptDraw {
    fn objects_mut(&mut self, id: SimpleIdentity) -> Option<&mut DrawObjects> {
        (self.id == id).then_some(&mut self.objects)
    }
}

/// What the renderer keeps of a tile a layer draws.
#[derive(Default)]
struct TileEntry {
    /// The bucket the tile's draws were built from, `None` until a layer's
    /// update sets it.
    bucket: Option<SimpleIdentity>,
    /// What of the bucket's vertex data changed since the draws copied it.
    changes: BucketChanges,
    /// The layer type's draws of the tile, of a layer that keeps them here.
    cache: Option<Box<dyn DrawCache>>,
}

/// How a draw tests and writes depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawDepth {
    /// It draws without a depth test.
    Off,
    /// It tests the depth of a sub-layer of the layer's depth, and writes it
    /// as `mask` says.
    Layer { sub_layer: u8, mask: DepthMaskType },
    /// It tests the depth range of 3D geometry, which the 3D draws of all
    /// layers share, and writes it as `mask` says.
    Volume { mask: DepthMaskType },
}

/// One draw of a layer's frame: the cached GPU objects it binds, and the
/// state it draws them with. Its uniform values are the entries at its index
/// in the layer's storage buffers.
pub(crate) struct FrameDraw {
    /// The tile whose cache holds the draw's GPU objects, or `None` for the
    /// layer's own cache.
    pub(crate) tile: Option<OverscaledTileID>,
    /// The id of the draw's GPU objects in their cache.
    pub(crate) id: SimpleIdentity,
    pub(crate) priority: DrawPriority,
    /// The passes the draw draws in.
    pub(crate) pass: RenderPass,
    pub(crate) depth: DrawDepth,
    /// Whether the draw clips: a draw at the layer's depth to the mask of
    /// its tile, a 3D one to where none of the layer's 3D draws of the pass
    /// drew before it, so no pixel blends twice.
    pub(crate) clip: bool,
    /// Gives the draw the textures of its tile's atlases before it draws.
    pub(crate) atlases: Option<Rc<AtlasBinding>>,
}

impl FrameDraw {
    /// A draw of the GPU objects `id` in the cache of `tile`, or of the
    /// layer for `None`, in the translucent pass at the layer's depth, which
    /// it reads, without clipping.
    #[must_use]
    pub(crate) fn new(tile: Option<OverscaledTileID>, id: SimpleIdentity) -> FrameDraw {
        FrameDraw {
            tile,
            id,
            priority: 0,
            pass: RenderPass::Translucent,
            depth: DrawDepth::Layer { sub_layer: 0, mask: DepthMaskType::ReadOnly },
            clip: false,
            atlases: None,
        }
    }

    /// The order of the layer's draws: by priority, then by the id of their
    /// GPU objects, which is the order the objects were made in.
    #[must_use]
    pub(crate) fn order(&self) -> (DrawPriority, SimpleIdentity) {
        (self.priority, self.id)
    }
}

/// A layer's draws of the frame, or of a render target it draws, and what
/// they share.
pub(crate) struct FrameDraws {
    /// Tells the layer's draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in [`FrameDraw::order`]. A draw's index is the index of its
    /// entries in the layer's storage buffers, which shaders read as
    /// `globalIndex`.
    pub(crate) draws: Vec<FrameDraw>,
    /// The buffers the draws bind, by id.
    pub(crate) uniform_buffers: UniformBufferArray,
    /// How the draws composite onto the frame in the translucent pass, as one
    /// surface, after they draw into a texture of their own; `None` where
    /// they draw straight onto the frame.
    pub(crate) composite: Option<Composite>,
    /// The tiles whose masks the stencil buffer holds when the draws draw.
    pub(crate) stencil_tiles: Option<Rc<Vec<UnwrappedTileID>>>,
}

impl FrameDraws {
    #[must_use]
    pub(crate) fn new() -> FrameDraws {
        FrameDraws {
            id: SimpleIdentity::new(),
            draws: Vec::new(),
            uniform_buffers: UniformBufferArray::new(),
            composite: None,
            stencil_tiles: None,
        }
    }
}

/// What the renderer keeps of a layer's drawing between frames: its draws of
/// the frame, an entry for each of the frame's tiles it draws, and the draws
/// it draws over its own.
#[derive(Default)]
pub(crate) struct LayerDraws {
    /// The layer's draws of the frame, from its first update with tiles to
    /// draw. While the layer is renderable, they take a place in the layers'
    /// order and depth, even when there are none.
    pub(crate) frame: Option<FrameDraws>,
    tiles: HashMap<OverscaledTileID, TileEntry>,
    /// The layer type's draws that no tile holds.
    layer_cache: Option<Box<dyn DrawCache>>,
    /// The draws the layer draws over its own, which take a place of their
    /// own in the layers' order, after the layer's, and have entries for the
    /// same tiles.
    overlay: Option<Box<LayerDraws>>,
}

impl LayerDraws {
    /// Makes `tiles`, the frame's tiles of the layer, those the layer has
    /// entries for: it keeps the entries of those it had, and drops the
    /// others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        let mut previous = std::mem::take(&mut self.tiles);
        self.tiles = tiles.into_iter().map(|id| (id, previous.remove(&id).unwrap_or_default())).collect();
        if let Some(overlay) = &mut self.overlay {
            overlay.set_tiles(self.tiles.keys().copied());
        }
    }

    /// Drops the entries of the tiles.
    pub(crate) fn clear_tiles(&mut self) {
        self.tiles.clear();
        if let Some(overlay) = &mut self.overlay {
            overlay.clear_tiles();
        }
    }

    /// Whether `tile_id` is one of the frame's tiles of the layer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_tile(&self, tile_id: &OverscaledTileID) -> bool {
        self.tiles.contains_key(tile_id)
    }

    /// The bucket the draws of `tile_id` were built from, if the tile is one
    /// of the frame's and they were.
    #[must_use]
    pub(crate) fn bucket(&self, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
        self.tiles.get(tile_id).and_then(|entry| entry.bucket)
    }

    /// Records that the draws of `tile_id` are built from `bucket`, which
    /// drops the changes of the bucket they were built from before. Returns
    /// whether that changed it; a tile that isn't one of the frame's takes no
    /// bucket.
    pub(crate) fn set_bucket(&mut self, tile_id: &OverscaledTileID, bucket: SimpleIdentity) -> bool {
        match self.tiles.get_mut(tile_id) {
            Some(entry) if entry.bucket != Some(bucket) => {
                entry.bucket = Some(bucket);
                entry.changes = BucketChanges::default();
                true
            }
            _ => false,
        }
    }

    /// What of the bucket of `tile_id` changed since its draws last copied
    /// it, which they are about to copy again.
    pub(crate) fn take_changes(&mut self, tile_id: &OverscaledTileID) -> BucketChanges {
        self.tiles.get_mut(tile_id).map(|entry| std::mem::take(&mut entry.changes)).unwrap_or_default()
    }

    /// Adds `changed` to the changes of the tiles whose draws were built from
    /// a bucket in it.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for entry in self.tiles.values_mut() {
            if let Some(changes) = entry.bucket.and_then(|bucket| changed.get(bucket)) {
                entry.changes |= changes;
            }
        }
    }

    /// Moves the tiles' entries and their draws to the tiles `wrap_delta`
    /// world copies east of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        self.tiles = std::mem::take(&mut self.tiles)
            .into_iter()
            .map(|(id, entry)| (id.unwrap_to(id.wrap.wrapping_add(wrap_delta)), entry))
            .collect();
        if let Some(overlay) = &mut self.overlay {
            overlay.shift_wraps(wrap_delta);
        }
    }

    /// The cache of `tile_id`'s draws, if the tile is one of the frame's and
    /// has one of type `C`.
    pub(crate) fn cache_mut<C: DrawCache>(&mut self, tile_id: &OverscaledTileID) -> Option<&mut C> {
        let cache: &mut dyn Any = self.tiles.get_mut(tile_id)?.cache.as_deref_mut()?;
        cache.downcast_mut()
    }

    /// Makes `cache` that of `tile_id`'s draws, if the tile is one of the
    /// frame's.
    pub(crate) fn set_cache(&mut self, tile_id: &OverscaledTileID, cache: impl DrawCache) {
        if let Some(entry) = self.tiles.get_mut(tile_id) {
            entry.cache = Some(Box::new(cache));
        }
    }

    /// Drops the cache of `tile_id`'s draws.
    pub(crate) fn drop_cache(&mut self, tile_id: &OverscaledTileID) {
        if let Some(entry) = self.tiles.get_mut(tile_id) {
            entry.cache = None;
        }
    }

    /// Makes `cache` that of the layer's draws that no tile holds.
    pub(crate) fn set_layer_cache(&mut self, cache: impl DrawCache) {
        self.layer_cache = Some(Box::new(cache));
    }

    /// Drops the cache of the layer's draws that no tile holds.
    pub(crate) fn drop_layer_cache(&mut self) {
        self.layer_cache = None;
    }

    /// The draws the layer draws over its own, if it has made them.
    pub(crate) fn overlay(&mut self) -> Option<&mut LayerDraws> {
        self.overlay.as_deref_mut()
    }

    /// The draws the layer draws over its own, which this makes, with
    /// entries for the layer's tiles and a place in the layers' order, the
    /// first time it is asked for. They keep the place from then on.
    pub(crate) fn make_overlay(&mut self) -> &mut LayerDraws {
        let tiles = &self.tiles;
        self.overlay.get_or_insert_with(|| {
            let mut overlay = LayerDraws { frame: Some(FrameDraws::new()), ..LayerDraws::default() };
            overlay.set_tiles(tiles.keys().copied());
            Box::new(overlay)
        })
    }

    /// Removes the draws of the frame and the caches of them, the overlay's
    /// with them. The tiles' entries stay.
    pub(crate) fn clear_draws(&mut self) {
        if let Some(frame) = &mut self.frame {
            frame.draws.clear();
        }
        for entry in self.tiles.values_mut() {
            entry.cache = None;
        }
        self.layer_cache = None;
        if let Some(overlay) = &mut self.overlay {
            overlay.clear_draws();
        }
    }

    /// Uploads the GPU objects of `draws`, draws of the layer's caches, in
    /// drawing order, and destroys the buffers each upload replaced.
    pub(crate) fn upload_draws(&mut self, draws: &FrameDraws, context: &Context) {
        upload_draws(draws, &mut self.caches(), context);
    }

    /// Draws `draws`, draws of the layer's caches, of the pass into the open
    /// pass of `gpu`, the context's.
    pub(crate) fn render_draws(&mut self, draws: &FrameDraws, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if !draws.draws.is_empty() && parameters.pass_target().is_some() {
            render_draws(draws, &mut self.caches(), gpu, parameters);
        }
    }

    /// Adds what the frame draws of the layer to `groups`, in drawing order:
    /// its draws of the frame, then those of its overlay.
    pub(crate) fn draw_groups<'a>(&'a mut self, groups: &mut Vec<DrawGroup<'a>>) {
        let LayerDraws { frame, tiles, layer_cache, overlay } = self;
        if let Some(frame) = frame {
            groups.push(DrawGroup(GroupKind::Draws { frame, caches: Caches::Layer { tiles, layer: layer_cache } }));
        }
        if let Some(overlay) = overlay {
            overlay.draw_groups(groups);
        }
    }

    fn caches(&mut self) -> Caches<'_> {
        Caches::Layer { tiles: &mut self.tiles, layer: &mut self.layer_cache }
    }
}

impl FrameDraws {
    /// Uploads the GPU objects of the draws, all of which `cache` holds, in
    /// drawing order, and destroys the buffers each upload replaced.
    pub(crate) fn upload_from(&self, cache: &mut dyn DrawCache, context: &Context) {
        upload_draws(self, &mut Caches::One(cache), context);
    }

    /// Draws the draws of the pass, all of whose GPU objects `cache` holds,
    /// into the open pass of `gpu`, the context's.
    pub(crate) fn render_from(
        &self,
        cache: &mut dyn DrawCache,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
    ) {
        if !self.draws.is_empty() && parameters.pass_target().is_some() {
            render_draws(self, &mut Caches::One(cache), gpu, parameters);
        }
    }
}

/// The caches that hold the GPU objects of draws: a layer's, or one of its
/// own.
enum Caches<'a> {
    /// The caches of a layer's tiles and of the layer.
    Layer { tiles: &'a mut HashMap<OverscaledTileID, TileEntry>, layer: &'a mut Option<Box<dyn DrawCache>> },
    /// One cache, which holds the GPU objects of every draw.
    One(&'a mut dyn DrawCache),
}

impl Caches<'_> {
    /// The GPU objects of `draw` in its cache.
    fn objects_mut(&mut self, draw: &FrameDraw) -> Option<&mut DrawObjects> {
        let cache = match self {
            Caches::Layer { tiles, layer } => match draw.tile {
                Some(tile) => tiles.get_mut(&tile)?.cache.as_deref_mut()?,
                None => layer.as_deref_mut()?,
            },
            Caches::One(cache) => &mut **cache,
        };
        cache.objects_mut(draw.id)
    }
}

/// Uploads the GPU objects of `frame`'s draws, in drawing order, and
/// destroys the buffers each upload replaced.
fn upload_draws(frame: &FrameDraws, caches: &mut Caches<'_>, context: &Context) {
    for draw in &frame.draws {
        if let Some(objects) = caches.objects_mut(draw) {
            objects.upload(context);
            context.destroy_released();
        }
    }
}

/// Draws `frame`'s draws of the pass. Where none of them is 3D, those that
/// clip clip each to its tile's mask, which this draws first; 3D draws that
/// clip share one stencil value of their own.
fn render_draws(frame: &FrameDraws, caches: &mut Caches<'_>, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
    let pass = parameters.pass.underlying_type();
    let in_pass = |draw: &&FrameDraw| draw.pass.underlying_type() & pass != 0;
    let is_volume = |draw: &&FrameDraw| matches!(draw.depth, DrawDepth::Volume { .. });
    let volume_stencil = if frame.draws.iter().filter(in_pass).any(|draw| is_volume(&draw)) {
        let clips = frame.draws.iter().filter(in_pass).filter(is_volume).any(|draw| draw.clip);
        clips.then(|| parameters.stencil_mode_for_3d(gpu))
    } else {
        if let Some(stencil_tiles) = frame.stencil_tiles.as_ref().filter(|tiles| !tiles.is_empty()) {
            parameters.render_tile_clipping_masks(gpu, stencil_tiles);
        }
        None
    };
    for (index, draw) in frame.draws.iter().enumerate() {
        if !in_pass(&draw) {
            continue;
        }
        let Some(objects) = caches.objects_mut(draw) else {
            debug_assert!(false, "a draw of tile {:?} has no GPU objects", draw.tile);
            continue;
        };
        if let Some(atlases) = &draw.atlases {
            atlases.bind(objects, parameters);
        }
        let tile_stencil = || match draw.tile.filter(|_| draw.clip) {
            Some(tile) => parameters.stencil_mode_for_clipping(&tile.to_unwrapped()),
            None => StencilMode::disabled(),
        };
        let (depth, stencil) = match draw.depth {
            DrawDepth::Off => (DepthMode::disabled(), tile_stencil()),
            DrawDepth::Layer { sub_layer, mask } => {
                (parameters.depth_mode_for_sublayer(sub_layer, mask), tile_stencil())
            }
            DrawDepth::Volume { mask } => (
                parameters.depth_mode_for_3d(mask),
                volume_stencil.filter(|_| draw.clip).unwrap_or_else(StencilMode::disabled),
            ),
        };
        objects.draw(gpu, parameters, Some(&frame.uniform_buffers), depth, stencil, index as u32);
    }
}

/// What the frame draws of a layer, in one place in the layers' order and
/// depth.
pub(crate) struct DrawGroup<'a>(GroupKind<'a>);

enum GroupKind<'a> {
    /// A layer's draws of the frame, with the caches that hold their GPU
    /// objects.
    Draws { frame: &'a FrameDraws, caches: Caches<'a> },
    /// A custom layer, whose host draws with objects of its own.
    Custom(&'a CustomLayerDraw),
}

impl<'a> DrawGroup<'a> {
    /// What the frame draws of a custom layer: what its host draws.
    #[must_use]
    pub(crate) fn custom(draw: &'a CustomLayerDraw) -> DrawGroup<'a> {
        DrawGroup(GroupKind::Custom(draw))
    }

    /// Uploads what the group draws, before the frame draws it.
    pub(crate) fn upload(&mut self, context: &Context) {
        if let GroupKind::Draws { frame, caches } = &mut self.0 {
            upload_draws(frame, caches, context);
        }
    }

    /// Draws the group's draws of the pass into the open pass of `gpu`, the
    /// context's: a layer's into the layer composite's texture in the
    /// translucent pass of a layer that composites, and onto the frame
    /// otherwise.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if parameters.pass_target().is_none() {
            return;
        }
        match &mut self.0 {
            GroupKind::Draws { frame, caches } => {
                if frame.draws.is_empty() {
                    return;
                }
                if frame.composite.is_some() && parameters.pass == RenderPass::Translucent {
                    parameters.draw_composited(gpu, frame.id, |gpu, parameters| {
                        render_draws(frame, caches, gpu, parameters);
                    });
                } else {
                    render_draws(frame, caches, gpu, parameters);
                }
            }
            GroupKind::Custom(custom) => custom.render(gpu, parameters),
        }
    }

    /// The tiles whose masks the stencil buffer holds when the group draws.
    #[must_use]
    pub(crate) fn stencil_tiles(&self) -> Option<&Rc<Vec<UnwrappedTileID>>> {
        match &self.0 {
            GroupKind::Draws { frame, .. } => frame.stencil_tiles.as_ref(),
            GroupKind::Custom(_) => None,
        }
    }

    /// How the group composites onto the frame, with the id that tells it
    /// apart in the layer composite, if it draws anything and composites.
    #[must_use]
    pub(crate) fn composite(&self) -> Option<(SimpleIdentity, Composite)> {
        match &self.0 {
            GroupKind::Draws { frame, .. } => {
                frame.composite.filter(|_| !frame.draws.is_empty()).map(|composite| (frame.id, composite))
            }
            GroupKind::Custom(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use mln_tile_id::OverscaledTileID;
    use mln_util::identity::SimpleIdentity;

    use super::{BucketChanges, ChangedBuckets, DrawCache, FrameDraws, LayerDraws};
    use crate::draw::DrawObjects;

    /// A tile's changes are those of the bucket its draws were built from,
    /// until the draws take them; a tile that leaves the frame
    /// takes its entry with it.
    #[test]
    fn changes_follow_the_tiles_bucket() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut draws = LayerDraws::default();
        draws.set_tiles([tile, other]);
        assert!(draws.set_bucket(&tile, bucket));
        assert!(!draws.set_bucket(&tile, bucket));
        assert!(draws.set_bucket(&other, other_bucket));

        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        changed.mark(bucket, BucketChanges { sort: true, ..BucketChanges::default() });
        draws.mark_changed(&changed);

        assert_eq!(BucketChanges { paint: true, sort: true, ..BucketChanges::default() }, draws.take_changes(&tile));
        assert_eq!(BucketChanges::default(), draws.take_changes(&tile));
        assert_eq!(BucketChanges::default(), draws.take_changes(&other));

        draws.set_tiles([other]);
        assert!(!draws.has_tile(&tile));
        assert_eq!(Some(other_bucket), draws.bucket(&other));
        draws.shift_wraps(1);
        assert_eq!(Some(other_bucket), draws.bucket(&other.unwrap_to(1)));
    }

    struct Cache(SimpleIdentity, DrawObjects);

    impl DrawCache for Cache {
        fn objects_mut(&mut self, id: SimpleIdentity) -> Option<&mut DrawObjects> {
            (self.0 == id).then_some(&mut self.1)
        }
    }

    /// A tile's cache of its draws moves with the tile across a wrap jump,
    /// and goes when the tile leaves the frame or the draws are cleared.
    #[test]
    fn caches_follow_their_tiles() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let id = SimpleIdentity::new();
        let mut draws = LayerDraws::default();
        draws.set_tiles([tile, other]);
        for tile_id in [tile, other] {
            draws.set_cache(&tile_id, Cache(id, DrawObjects::new(String::new())));
        }

        draws.shift_wraps(1);
        let moved = tile.unwrap_to(1);
        assert!(draws.cache_mut::<Cache>(&moved).is_some_and(|cache| cache.objects_mut(id).is_some()));
        draws.set_tiles([moved]);
        assert!(draws.cache_mut::<Cache>(&other.unwrap_to(1)).is_none());
        draws.set_tiles([moved, other]);
        assert!(draws.cache_mut::<Cache>(&other).is_none());

        draws.clear_draws();
        assert!(draws.has_tile(&moved));
        assert!(draws.cache_mut::<Cache>(&moved).is_none());
    }

    /// The overlay has entries for the layer's tiles, which follow the
    /// layer's, and takes a place in the layers' order after the layer's.
    #[test]
    fn the_overlay_follows_the_layers_tiles() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let id = SimpleIdentity::new();
        let mut draws = LayerDraws { frame: Some(FrameDraws::new()), ..LayerDraws::default() };
        draws.set_tiles([tile]);
        assert!(draws.overlay().is_none());
        assert!(draws.make_overlay().has_tile(&tile));
        draws.make_overlay().set_cache(&tile, Cache(id, DrawObjects::new(String::new())));
        let mut groups = Vec::new();
        draws.draw_groups(&mut groups);
        assert_eq!(2, groups.len());
        drop(groups);

        draws.set_tiles([tile, other]);
        assert!(draws.make_overlay().has_tile(&other));
        draws.shift_wraps(-1);
        assert!(draws.make_overlay().cache_mut::<Cache>(&tile.unwrap_to(-1)).is_some());
        draws.clear_draws();
        assert!(draws.make_overlay().cache_mut::<Cache>(&tile.unwrap_to(-1)).is_none());
        draws.clear_tiles();
        assert!(!draws.make_overlay().has_tile(&other.unwrap_to(-1)));
    }
}
