//! What the renderer keeps of each layer's drawing between frames, and the
//! draws of a frame.
//!
//! The orchestrator owns a [`LayerDraws`] for each render layer and lends it
//! to the layer's calls. Each tile's entry says which bucket the tile's draws
//! were built from, and what of that bucket's vertex data changed since, as
//! the frame's prepare reports it. A layer keeps the GPU objects of its
//! draws either in drawables of its layer group, or in each tile's entry,
//! with state of its own type ([`TileCache`]); such a layer's update returns
//! the frame's draws as values ([`TileDraw`]), which the frame's encode
//! issues ([`DrawGroup`]).

use std::any::Any;
use std::ops::BitOrAssign;
use std::rc::Rc;

use mln_gfx::gpu::Gpu;
use mln_gfx::types::DepthMaskType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;

use crate::draw::DrawObjects;
use crate::drawable::DrawPriority;
use crate::gfx::context::Context;
use crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::layer_composite::Composite;
use crate::layer_group::LayerGroup;
use crate::paint_parameters::PaintParameters;
use crate::render_pass::RenderPass;

/// What of a bucket's vertex data changed since a tile's drawables copied it.
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

/// What a layer type keeps of a tile it draws between frames: the GPU
/// objects of the tile's draws, and its own state of them.
pub(crate) trait TileCache: Any {
    /// The GPU objects of the tile's draw `id`.
    fn objects_mut(&mut self, id: SimpleIdentity) -> Option<&mut DrawObjects>;
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
    cache: Option<Box<dyn TileCache>>,
}

/// One draw of a layer's frame: the cached GPU objects it binds, and the
/// state it draws them with. Its uniform values are the entries at its index
/// in the layer's storage buffers.
pub(crate) struct TileDraw {
    /// The tile whose cache holds the draw's GPU objects, and whose clipping
    /// mask the draw clips to.
    pub(crate) tile: OverscaledTileID,
    /// The id of the draw's GPU objects in the tile's cache.
    pub(crate) id: SimpleIdentity,
    pub(crate) priority: DrawPriority,
    /// The passes the draw draws in.
    pub(crate) pass: RenderPass,
    /// The sub-layer of the layer's depth the draw tests and writes at.
    pub(crate) sub_layer: u8,
    pub(crate) depth: DepthMaskType,
    /// Gives the draw the textures of its tile's atlases before it draws.
    pub(crate) atlases: Option<Rc<DrawableAtlasesTweaker>>,
}

impl TileDraw {
    /// The order of the layer's draws: by priority, then by the id of their
    /// GPU objects, which is the order the objects were made in.
    #[must_use]
    pub(crate) fn order(&self) -> (DrawPriority, SimpleIdentity) {
        (self.priority, self.id)
    }
}

/// A layer's draws of the frame, and what they share.
pub(crate) struct FrameDraws {
    /// Tells the layer's draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in [`TileDraw::order`]. A draw's index is the index of its
    /// entries in the layer's storage buffers, which shaders read as
    /// `globalIndex`.
    pub(crate) draws: Vec<TileDraw>,
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

/// What the renderer keeps of a layer's drawing between frames: the group of
/// its drawables or its draws of the frame, and an entry for each of the
/// frame's tiles it draws.
#[derive(Default)]
pub(crate) struct LayerDraws {
    /// The group of the layer's drawables, which draws while the layer is
    /// renderable.
    pub(crate) group: Option<LayerGroup>,
    /// The layer's draws of the frame, of a layer whose update returns them,
    /// from its first update with tiles to draw. While the layer is
    /// renderable, they take a place in the layers' order and depth, even
    /// when there are none.
    pub(crate) frame: Option<FrameDraws>,
    tiles: HashMap<OverscaledTileID, TileEntry>,
}

impl LayerDraws {
    /// Makes `tiles`, the frame's tiles of the layer, those the layer has
    /// entries for: it keeps the entries of those it had, and drops the
    /// others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        let mut previous = std::mem::take(&mut self.tiles);
        self.tiles = tiles.into_iter().map(|id| (id, previous.remove(&id).unwrap_or_default())).collect();
    }

    /// Drops the entries of the tiles.
    pub(crate) fn clear_tiles(&mut self) {
        self.tiles.clear();
    }

    /// Whether `tile_id` is one of the frame's tiles of the layer.
    #[must_use]
    pub(crate) fn has_tile(&self, tile_id: &OverscaledTileID) -> bool {
        self.tiles.contains_key(tile_id)
    }

    /// The bucket the drawables of `tile_id` were built from, if the tile is
    /// one of the frame's and they were.
    #[must_use]
    pub(crate) fn bucket(&self, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
        self.tiles.get(tile_id).and_then(|entry| entry.bucket)
    }

    /// Records that the drawables of `tile_id` are built from `bucket`, which
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

    /// What of the bucket of `tile_id` changed since its drawables last
    /// copied it, which they are about to copy again.
    pub(crate) fn take_changes(&mut self, tile_id: &OverscaledTileID) -> BucketChanges {
        self.tiles.get_mut(tile_id).map(|entry| std::mem::take(&mut entry.changes)).unwrap_or_default()
    }

    /// Adds `changed` to the changes of the tiles whose drawables were built
    /// from a bucket in it.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for entry in self.tiles.values_mut() {
            if let Some(changes) = entry.bucket.and_then(|bucket| changed.get(bucket)) {
                entry.changes |= changes;
            }
        }
    }

    /// Moves the tiles' entries and drawables to the tiles `wrap_delta`
    /// world copies east of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        self.tiles = std::mem::take(&mut self.tiles)
            .into_iter()
            .map(|(id, entry)| (id.unwrap_to(id.wrap.wrapping_add(wrap_delta)), entry))
            .collect();
        if let Some(group) = &mut self.group {
            group.shift_wraps(wrap_delta);
        }
    }

    /// The cache of `tile_id`'s draws, if the tile is one of the frame's and
    /// has one of type `C`.
    pub(crate) fn cache_mut<C: TileCache>(&mut self, tile_id: &OverscaledTileID) -> Option<&mut C> {
        let cache: &mut dyn Any = self.tiles.get_mut(tile_id)?.cache.as_deref_mut()?;
        cache.downcast_mut()
    }

    /// Makes `cache` that of `tile_id`'s draws, if the tile is one of the
    /// frame's.
    pub(crate) fn set_cache(&mut self, tile_id: &OverscaledTileID, cache: impl TileCache) {
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

    /// Removes the drawables, the draws of the frame and the tiles' caches
    /// of them. The tiles' entries stay.
    pub(crate) fn clear_drawables(&mut self) {
        if let Some(group) = &mut self.group {
            group.clear_drawables();
        }
        if let Some(frame) = &mut self.frame {
            frame.draws.clear();
        }
        for entry in self.tiles.values_mut() {
            entry.cache = None;
        }
    }

    /// Uploads the GPU objects of the frame's draws, in drawing order, and
    /// destroys the buffers each upload replaced.
    fn upload_frame(&mut self, context: &Context) {
        let Some(frame) = &self.frame else { return };
        for draw in &frame.draws {
            if let Some(objects) = objects_mut(&mut self.tiles, draw) {
                objects.upload(context);
                context.destroy_released();
            }
        }
    }

    /// Draws the frame's draws of the pass into the open pass of `gpu`, the
    /// context's: into the layer composite's texture in the translucent pass
    /// of a layer that composites, and onto the frame otherwise.
    fn render_frame(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(frame) = &self.frame else { return };
        if frame.draws.is_empty() || parameters.pass_target().is_none() {
            return;
        }
        let tiles = &mut self.tiles;
        if frame.composite.is_some() && parameters.pass == RenderPass::Translucent {
            parameters.draw_composited(gpu, frame.id, |gpu, parameters| render_draws(frame, tiles, gpu, parameters));
        } else {
            render_draws(frame, tiles, gpu, parameters);
        }
    }
}

/// The GPU objects of `draw` in its tile's cache.
fn objects_mut<'a>(
    tiles: &'a mut HashMap<OverscaledTileID, TileEntry>,
    draw: &TileDraw,
) -> Option<&'a mut DrawObjects> {
    tiles.get_mut(&draw.tile)?.cache.as_mut()?.objects_mut(draw.id)
}

/// Draws `frame`'s draws of the pass, each clipped to its tile's mask, which
/// this draws first.
fn render_draws(
    frame: &FrameDraws,
    tiles: &mut HashMap<OverscaledTileID, TileEntry>,
    gpu: &mut dyn Gpu,
    parameters: &mut PaintParameters<'_>,
) {
    if let Some(stencil_tiles) = frame.stencil_tiles.as_ref().filter(|tiles| !tiles.is_empty()) {
        parameters.render_tile_clipping_masks(gpu, stencil_tiles);
    }
    let pass = parameters.pass.underlying_type();
    for (index, draw) in frame.draws.iter().enumerate() {
        if draw.pass.underlying_type() & pass == 0 {
            continue;
        }
        let Some(objects) = objects_mut(tiles, draw) else {
            debug_assert!(false, "a draw of tile {:?} has no GPU objects", draw.tile);
            continue;
        };
        if let Some(atlases) = &draw.atlases {
            atlases.execute(objects, parameters);
        }
        let depth = parameters.depth_mode_for_sublayer(draw.sub_layer, draw.depth);
        let stencil = parameters.stencil_mode_for_clipping(&draw.tile.to_unwrapped());
        objects.draw(gpu, parameters, Some(&frame.uniform_buffers), depth, stencil, index as u32);
    }
}

/// What the frame draws of a layer, in one place in the layers' order and
/// depth.
pub(crate) enum DrawGroup<'a> {
    /// A layer group of drawables.
    Drawables(&'a mut LayerGroup),
    /// A layer's draws of the frame, with the caches that hold their GPU
    /// objects.
    Draws(&'a mut LayerDraws),
}

impl<'a> DrawGroup<'a> {
    /// What the frame draws of the layer `draws` keeps, if it draws.
    #[must_use]
    pub(crate) fn of(draws: &'a mut LayerDraws) -> Option<DrawGroup<'a>> {
        if draws.frame.is_some() {
            Some(DrawGroup::Draws(draws))
        } else {
            draws.group.as_mut().map(DrawGroup::Drawables)
        }
    }

    /// Uploads what the group draws, before the frame draws it.
    pub(crate) fn upload(&mut self, context: &Context) {
        match self {
            DrawGroup::Drawables(group) => group.upload(context),
            DrawGroup::Draws(draws) => draws.upload_frame(context),
        }
    }

    /// Draws the group's draws of the pass into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        match self {
            DrawGroup::Drawables(group) => group.render(gpu, parameters),
            DrawGroup::Draws(draws) => draws.render_frame(gpu, parameters),
        }
    }

    /// The tiles whose masks the stencil buffer holds when the group draws.
    #[must_use]
    pub(crate) fn stencil_tiles(&self) -> Option<&Rc<Vec<UnwrappedTileID>>> {
        match self {
            DrawGroup::Drawables(group) => group.stencil_tiles.as_ref(),
            DrawGroup::Draws(draws) => draws.frame.as_ref()?.stencil_tiles.as_ref(),
        }
    }

    /// How the group composites onto the frame, with the id that tells it
    /// apart in the layer composite, if it draws anything and composites.
    #[must_use]
    pub(crate) fn composite(&self) -> Option<(SimpleIdentity, Composite)> {
        match self {
            DrawGroup::Drawables(group) => {
                group.composite.filter(|_| group.enabled && !group.empty()).map(|composite| (group.id, composite))
            }
            DrawGroup::Draws(draws) => {
                let frame = draws.frame.as_ref()?;
                frame.composite.filter(|_| !frame.draws.is_empty()).map(|composite| (frame.id, composite))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mln_tile_id::OverscaledTileID;
    use mln_util::identity::SimpleIdentity;

    use super::{BucketChanges, ChangedBuckets, LayerDraws, TileCache};
    use crate::draw::DrawObjects;

    /// A tile's changes are those of the bucket its drawables were built
    /// from, until the drawables take them; a tile that leaves the frame
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

    impl TileCache for Cache {
        fn objects_mut(&mut self, id: SimpleIdentity) -> Option<&mut DrawObjects> {
            (self.0 == id).then_some(&mut self.1)
        }
    }

    /// A tile's cache of its draws moves with the tile across a wrap jump,
    /// and goes when the tile leaves the frame or the drawables are
    /// cleared.
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

        draws.clear_drawables();
        assert!(draws.has_tile(&moved));
        assert!(draws.cache_mut::<Cache>(&moved).is_none());
    }
}
