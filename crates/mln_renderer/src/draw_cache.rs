//! What a layer keeps of its drawing between frames, and the draws of a
//! frame.
//!
//! A layer keeps the GPU objects of its draws in [`DrawCaches`]: a cache of
//! its own type in each tile's entry, which also says which bucket the
//! tile's draws were built from and what of that bucket's vertex data
//! changed since, and a cache of the draws no tile holds. Its update makes
//! the frame's draws, values ([`FrameDraw`]) that name their GPU objects by
//! their cache and their slot in it, with their uniforms in the buffers of
//! their [`FrameDraws`], which [`DrawCaches::upload`] and
//! [`DrawCaches::render`] issue.

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

/// A cache of the GPU objects of draws, which the frame's draws bind by
/// their slot in it.
pub(crate) trait DrawSlots {
    /// The GPU objects in `slot`, if the cache has any there.
    fn slot_mut(&mut self, slot: u32) -> Option<&mut DrawObjects>;
}

/// The cache of a layer without draws of its kind.
impl DrawSlots for () {
    fn slot_mut(&mut self, _slot: u32) -> Option<&mut DrawObjects> {
        None
    }
}

/// The slots of the cache, if there is one.
impl<T: DrawSlots> DrawSlots for Option<T> {
    fn slot_mut(&mut self, slot: u32) -> Option<&mut DrawObjects> {
        self.as_mut()?.slot_mut(slot)
    }
}

/// The GPU objects of one draw a layer keeps, in slot 0, with the id that
/// orders the draws by when their objects were made.
pub(crate) struct KeptDraw {
    pub(crate) id: SimpleIdentity,
    pub(crate) objects: DrawObjects,
}

impl KeptDraw {
    #[must_use]
    pub(crate) fn new(objects: DrawObjects) -> KeptDraw {
        KeptDraw { id: SimpleIdentity::new(), objects }
    }

    /// A draw of the objects, in the cache of `tile` or of the layer for
    /// `None` (see [`FrameDraw::new`]).
    #[must_use]
    pub(crate) fn frame_draw(&self, tile: Option<OverscaledTileID>) -> FrameDraw {
        FrameDraw::new(tile, 0, self.id)
    }
}

impl DrawSlots for KeptDraw {
    fn slot_mut(&mut self, slot: u32) -> Option<&mut DrawObjects> {
        (slot == 0).then_some(&mut self.objects)
    }
}

/// Kept draws, each in the slot of its index.
impl DrawSlots for Vec<KeptDraw> {
    fn slot_mut(&mut self, slot: u32) -> Option<&mut DrawObjects> {
        self.get_mut(slot as usize).map(|draw| &mut draw.objects)
    }
}

/// What a layer keeps of a tile it draws.
struct TileEntry<T> {
    /// The bucket the tile's draws were built from, `None` until a layer's
    /// update sets it.
    bucket: Option<SimpleIdentity>,
    /// What of the bucket's vertex data changed since the draws copied it.
    changes: BucketChanges,
    /// The layer's cache of the tile's draws, boxed so that the entries stay
    /// small to move.
    cache: Option<Box<T>>,
    /// Whether the tile is one of those [`DrawCaches::set_tiles`] was last
    /// given.
    kept: bool,
}

impl<T> Default for TileEntry<T> {
    fn default() -> Self {
        TileEntry { bucket: None, changes: BucketChanges::default(), cache: None, kept: true }
    }
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

/// One draw of a frame: the cached GPU objects it binds, and the state it
/// draws them with. Its uniform values are the entries at its index in the
/// storage buffers of its [`FrameDraws`].
pub(crate) struct FrameDraw {
    /// The tile whose cache holds the draw's GPU objects, or `None` for the
    /// layer's cache.
    pub(crate) tile: Option<OverscaledTileID>,
    /// The slot of the draw's GPU objects in their cache.
    pub(crate) slot: u32,
    /// The id of the draw's GPU objects, which orders the draws by when
    /// their objects were made.
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
    /// A draw of the GPU objects `id` in `slot` of the cache of `tile`, or
    /// of the layer for `None`, in the translucent pass at the layer's
    /// depth, which it reads, without clipping.
    #[must_use]
    pub(crate) fn new(tile: Option<OverscaledTileID>, slot: u32, id: SimpleIdentity) -> FrameDraw {
        FrameDraw {
            tile,
            slot,
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

/// Draws of a frame, and what they share.
pub(crate) struct FrameDraws {
    /// Tells the draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in [`FrameDraw::order`]. A draw's index is the index of its
    /// entries in the storage buffers, which shaders read as `globalIndex`.
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

    /// How the draws composite onto the frame, with the id that tells them
    /// apart in the layer composite, if there are any and they composite.
    #[must_use]
    pub(crate) fn composited(&self) -> Option<(SimpleIdentity, Composite)> {
        self.composite.filter(|_| !self.draws.is_empty()).map(|composite| (self.id, composite))
    }
}

/// What the frame draws of a layer in one place in the layers' order and
/// depth.
#[derive(Clone, Copy)]
pub(crate) enum DrawGroup<'a> {
    /// Draws of the layer's caches.
    Draws(&'a FrameDraws),
    /// What a custom layer's host draws, with objects of its own.
    Custom,
}

impl<'a> DrawGroup<'a> {
    /// The tiles whose masks the stencil buffer holds when the group draws.
    #[must_use]
    pub(crate) fn stencil_tiles(self) -> Option<&'a Rc<Vec<UnwrappedTileID>>> {
        match self {
            DrawGroup::Draws(frame) => frame.stencil_tiles.as_ref(),
            DrawGroup::Custom => None,
        }
    }

    /// How the group composites onto the frame, with the id that tells it
    /// apart in the layer composite, if it draws anything and composites.
    #[must_use]
    pub(crate) fn composite(self) -> Option<(SimpleIdentity, Composite)> {
        match self {
            DrawGroup::Draws(frame) => frame.composited(),
            DrawGroup::Custom => None,
        }
    }
}

/// The caches of the GPU objects of a layer's draws: an entry of each of the
/// frame's tiles the layer draws, with the tile's cache of type `T`, and the
/// cache of type `L` of the draws no tile holds.
pub(crate) struct DrawCaches<T, L = ()> {
    tiles: HashMap<OverscaledTileID, TileEntry<T>>,
    layer: L,
}

impl<T, L: Default> Default for DrawCaches<T, L> {
    fn default() -> Self {
        DrawCaches::of_layer(L::default())
    }
}

impl<T, L> DrawCaches<T, L> {
    /// Caches with `layer` as the cache of the draws no tile holds, and no
    /// tiles.
    #[must_use]
    pub(crate) fn of_layer(layer: L) -> Self {
        DrawCaches { tiles: HashMap::default(), layer }
    }

    /// Makes `tiles`, the frame's tiles of the layer, those the caches have
    /// entries for: it keeps the entries of those it had, and drops the
    /// others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        for entry in self.tiles.values_mut() {
            entry.kept = false;
        }
        for id in tiles {
            self.tiles.entry(id).or_default().kept = true;
        }
        self.tiles.retain(|_, entry| entry.kept);
    }

    /// Drops the entries of the tiles.
    pub(crate) fn clear_tiles(&mut self) {
        self.tiles.clear();
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

    /// Moves the tiles' entries and their caches to the tiles `wrap_delta`
    /// world copies east of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        self.tiles = std::mem::take(&mut self.tiles)
            .into_iter()
            .map(|(id, entry)| (id.unwrap_to(id.wrap.wrapping_add(wrap_delta)), entry))
            .collect();
    }

    /// The cache of `tile_id`'s draws, if the tile is one of the frame's and
    /// has one.
    pub(crate) fn cache(&self, tile_id: &OverscaledTileID) -> Option<&T> {
        self.tiles.get(tile_id)?.cache.as_deref()
    }

    /// The cache of `tile_id`'s draws, if the tile is one of the frame's and
    /// has one.
    pub(crate) fn cache_mut(&mut self, tile_id: &OverscaledTileID) -> Option<&mut T> {
        self.tiles.get_mut(tile_id)?.cache.as_deref_mut()
    }

    /// Makes `cache` that of `tile_id`'s draws, if the tile is one of the
    /// frame's, and returns it.
    pub(crate) fn set_cache(&mut self, tile_id: &OverscaledTileID, cache: T) -> Option<&mut T> {
        let entry = self.tiles.get_mut(tile_id)?;
        Some(entry.cache.insert(Box::new(cache)))
    }

    /// Drops the cache of `tile_id`'s draws.
    pub(crate) fn drop_cache(&mut self, tile_id: &OverscaledTileID) {
        if let Some(entry) = self.tiles.get_mut(tile_id) {
            entry.cache = None;
        }
    }

    /// The cache of the draws that no tile holds.
    pub(crate) fn layer_cache_mut(&mut self) -> &mut L {
        &mut self.layer
    }
}

impl<T, L: Default> DrawCaches<T, L> {
    /// Drops the caches of the tiles' draws, and empties that of the draws
    /// no tile holds. The tiles' entries stay.
    pub(crate) fn clear(&mut self) {
        for entry in self.tiles.values_mut() {
            entry.cache = None;
        }
        self.layer = L::default();
    }
}

impl<T: DrawSlots, L: DrawSlots> DrawCaches<T, L> {
    /// The GPU objects `draw` binds.
    fn objects_mut(&mut self, draw: &FrameDraw) -> Option<&mut DrawObjects> {
        match draw.tile {
            Some(tile) => self.tiles.get_mut(&tile)?.cache.as_mut()?.slot_mut(draw.slot),
            None => self.layer.slot_mut(draw.slot),
        }
    }

    /// Uploads the GPU objects of `frame`'s draws, all of which the caches
    /// hold, in drawing order, and destroys the buffers each upload replaced.
    pub(crate) fn upload(&mut self, frame: &FrameDraws, context: &Context) {
        for draw in &frame.draws {
            if let Some(objects) = self.objects_mut(draw) {
                objects.upload(context);
                context.destroy_released();
            }
        }
    }

    /// Draws `frame`'s draws of the pass, all of whose GPU objects the caches
    /// hold, into the open pass of `gpu`, the context's: into the layer
    /// composite's texture in the translucent pass where they composite, and
    /// onto the frame otherwise.
    pub(crate) fn render(&mut self, frame: &FrameDraws, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if frame.draws.is_empty() || parameters.pass_target().is_none() {
            return;
        }
        if frame.composite.is_some() && parameters.pass == RenderPass::Translucent {
            parameters.draw_composited(gpu, frame.id, |gpu, parameters| self.render_pass(frame, gpu, parameters));
        } else {
            self.render_pass(frame, gpu, parameters);
        }
    }

    /// Draws `frame`'s draws of the pass. Where none of them is 3D, those
    /// that clip clip each to its tile's mask, which this draws first; 3D
    /// draws that clip share one stencil value of their own.
    fn render_pass(&mut self, frame: &FrameDraws, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
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
            let Some(objects) = self.objects_mut(draw) else {
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
}

#[cfg(test)]
mod tests {
    use mln_tile_id::OverscaledTileID;
    use mln_util::identity::SimpleIdentity;

    use super::{BucketChanges, ChangedBuckets, DrawCaches};

    /// A tile's changes are those of the bucket its draws were built from,
    /// until the draws take them; a tile that leaves the frame
    /// takes its entry with it.
    #[test]
    fn changes_follow_the_tiles_bucket() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut caches = DrawCaches::<()>::default();
        caches.set_tiles([tile, other]);
        assert!(caches.set_bucket(&tile, bucket));
        assert!(!caches.set_bucket(&tile, bucket));
        assert!(caches.set_bucket(&other, other_bucket));

        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        changed.mark(bucket, BucketChanges { sort: true, ..BucketChanges::default() });
        caches.mark_changed(&changed);

        assert_eq!(BucketChanges { paint: true, sort: true, ..BucketChanges::default() }, caches.take_changes(&tile));
        assert_eq!(BucketChanges::default(), caches.take_changes(&tile));
        assert_eq!(BucketChanges::default(), caches.take_changes(&other));

        caches.set_tiles([other]);
        assert!(!caches.has_tile(&tile));
        assert_eq!(Some(other_bucket), caches.bucket(&other));
        caches.shift_wraps(1);
        assert_eq!(Some(other_bucket), caches.bucket(&other.unwrap_to(1)));
    }

    /// A tile's cache of its draws moves with the tile across a wrap jump,
    /// and goes when the tile leaves the frame or the caches are cleared.
    #[test]
    fn caches_follow_their_tiles() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let mut caches = DrawCaches::<u32>::default();
        caches.set_tiles([tile, other]);
        assert!(caches.set_cache(&tile, 1).is_some());
        assert!(caches.set_cache(&other, 2).is_some());
        assert!(caches.set_cache(&OverscaledTileID::with_zxy(1, 0, 1, 1, 1), 3).is_none());

        caches.shift_wraps(1);
        let moved = tile.unwrap_to(1);
        assert_eq!(Some(&mut 1), caches.cache_mut(&moved));
        caches.set_tiles([moved]);
        assert!(caches.cache_mut(&other.unwrap_to(1)).is_none());
        caches.set_tiles([moved, other]);
        assert!(caches.cache_mut(&other).is_none());

        caches.clear();
        assert!(caches.has_tile(&moved));
        assert!(caches.cache_mut(&moved).is_none());
    }
}
