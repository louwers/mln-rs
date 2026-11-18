//! What a layer keeps of its drawing between frames, and the draws of a
//! frame.
//!
//! Each layer type keeps the GPU objects of its draws in types of its own,
//! those of a tile in a [`TileMap`], which also says which bucket the
//! tile's draws were built from and what of that bucket's vertex data
//! changed since. Its update makes the frame's draws, values
//! ([`FrameDraw`]) in a [`DrawList`] with their uniform values and the part
//! of the layer's GPU objects each binds, which it writes to the buffers of
//! their [`LayerDraws`]. The layer type then uploads and draws them in
//! loops of its own: [`FrameDraws::begin`] begins a pass's draws, and
//! [`DrawPass::draw`] draws one draw's objects. A layer type that draws one
//! group of its tiles' draws keeps them in a [`TileDraws`], whose tiles name
//! the objects of their draws by a part of their own type
//! ([`TileObjects`]).

use std::ops::BitOrAssign;

use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;
use mln_vec_map::VecMap;

use crate::draw::DrawObjects;
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::layer_composite::Composite;
use crate::paint_parameters::{Composited, PaintParameters};
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

/// What a layer keeps of a tile it draws, whatever the layer's type.
#[derive(Default)]
struct TileState {
    /// The bucket the tile's draws were built from, `None` until a layer's
    /// update sets it.
    bucket: Option<SimpleIdentity>,
    /// What of the bucket's vertex data changed since the draws copied it.
    changes: BucketChanges,
}

/// The frame's tiles a layer draws, in id order, each with its
/// [`TileState`]: the part of a [`TileMap`] that doesn't depend on what the
/// layer keeps of a tile.
#[derive(Default)]
struct TileSet {
    tiles: VecMap<OverscaledTileID, TileState>,
    /// The tiles that `set_tiles` makes the set's.
    incoming: Vec<OverscaledTileID>,
    /// For each tile of the set, the index it had before the set's tiles
    /// last changed, `None` for a tile new to the set.
    moves: Vec<Option<usize>>,
}

impl TileSet {
    /// Makes `incoming` the set's tiles: keeps the states of those it has,
    /// and drops the others. Returns whether that changed any tile's index,
    /// which `moves` then says.
    fn set_tiles(&mut self) -> bool {
        let incoming = &mut self.incoming;
        // The stable sort, which the renderer's other sorts of tile ids
        // share, allocates nothing for a frame's tens of tiles.
        incoming.sort();
        incoming.dedup();
        let keys = self.tiles.keys().as_slice();
        if keys == incoming.as_slice() {
            return false;
        }
        self.moves.clear();
        let mut old = 0;
        for id in incoming.iter() {
            while keys.get(old).is_some_and(|key| key < id) {
                old += 1;
            }
            self.moves.push(keys.get(old).filter(|key| *key == id).map(|_| old));
        }
        // The tiles that stay, whose old indexes go up in `moves`.
        let (mut kept, mut index) = (self.moves.iter().flatten().peekable(), 0);
        self.tiles.retain(|_, _| {
            let keep = kept.next_if_eq(&&index).is_some();
            index += 1;
            keep
        });
        for (id, old) in incoming.iter().zip(&self.moves) {
            if old.is_none() {
                self.tiles.insert(*id, TileState::default());
            }
        }
        true
    }

    /// Moves the tiles' states to the tiles `wrap_delta` world copies east
    /// of theirs. Returns whether that changed their order, which `moves`
    /// then says; only a wrap that overflows does.
    fn shift_wraps(&mut self, wrap_delta: i16) -> bool {
        let shift = |id: &OverscaledTileID| id.unwrap_to(id.wrap.wrapping_add(wrap_delta));
        if self.tiles.map_keys_in_order(shift) {
            return false;
        }
        // The shifted ids, in their old order.
        self.incoming.clear();
        for (id, state) in std::mem::take(&mut self.tiles) {
            let id = shift(&id);
            self.incoming.push(id);
            self.tiles.insert(id, state);
        }
        self.moves.clear();
        self.moves.resize(self.tiles.len(), None);
        for (old, id) in self.incoming.iter().enumerate() {
            if let Some(new) = self.tiles.index_of(id) {
                self.moves[new] = Some(old);
            }
        }
        true
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for state in self.tiles.values_mut() {
            if let Some(changes) = state.bucket.and_then(|bucket| changed.get(bucket)) {
                state.changes |= changes;
            }
        }
    }
}

/// What a layer keeps of each of the frame's tiles it draws, by tile id: the
/// bucket the tile's draws were built from and what of it changed since,
/// and the layer's own `T` of the tile, from when the layer builds one.
pub(crate) struct TileMap<T> {
    set: TileSet,
    /// Each tile's `T`, at the tile's index in the set.
    values: Vec<Option<T>>,
    /// The values before the set's tiles changed, while they move to their
    /// tiles' new indexes; empty otherwise.
    moving: Vec<Option<T>>,
}

impl<T> Default for TileMap<T> {
    fn default() -> Self {
        TileMap { set: TileSet::default(), values: Vec::new(), moving: Vec::new() }
    }
}

impl<T> TileMap<T> {
    /// Makes `tiles`, the frame's tiles of the layer in any order, those the
    /// map has entries for: it keeps the entries of those it had, and drops
    /// the others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        self.set.incoming.clear();
        self.set.incoming.extend(tiles);
        if self.set.set_tiles() {
            self.apply_moves();
        }
    }

    /// Moves the values to their tiles' indexes, as the set's `moves` say,
    /// and drops those of the tiles that left.
    fn apply_moves(&mut self) {
        std::mem::swap(&mut self.values, &mut self.moving);
        let moving = &mut self.moving;
        self.values.extend(self.set.moves.iter().map(|old| old.and_then(|old| moving.get_mut(old)?.take())));
        moving.clear();
    }

    /// Drops the entries of the tiles.
    pub(crate) fn clear_tiles(&mut self) {
        self.set.tiles.clear();
        self.values.clear();
    }

    /// Moves the tiles' entries to the tiles `wrap_delta` world copies east
    /// of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        if self.set.shift_wraps(wrap_delta) {
            self.apply_moves();
        }
    }

    /// Adds `changed` to the changes of the tiles whose draws were built from
    /// a bucket in it.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.set.mark_changed(changed);
    }

    /// Drops what the layer keeps of the tiles. The tiles' entries stay.
    pub(crate) fn clear(&mut self) {
        self.values.fill_with(|| None);
    }

    /// Whether `tile_id` is one of the frame's tiles of the layer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_tile(&self, tile_id: &OverscaledTileID) -> bool {
        self.set.tiles.contains_key(tile_id)
    }

    /// What the layer keeps of `tile_id`, if the tile is one of the frame's
    /// and the layer keeps anything of it.
    #[must_use]
    pub(crate) fn get(&self, tile_id: &OverscaledTileID) -> Option<&T> {
        self.values.get(self.set.tiles.index_of(tile_id)?)?.as_ref()
    }

    /// What the layer keeps of `tile_id`, if the tile is one of the frame's
    /// and the layer keeps anything of it.
    pub(crate) fn get_mut(&mut self, tile_id: &OverscaledTileID) -> Option<&mut T> {
        self.values.get_mut(self.set.tiles.index_of(tile_id)?)?.as_mut()
    }

    /// Makes `value` what the layer keeps of `tile_id`, if the tile is one
    /// of the frame's, and returns it.
    pub(crate) fn insert(&mut self, tile_id: &OverscaledTileID, value: T) -> Option<&mut T> {
        Some(self.entry(tile_id)?.value.insert(value))
    }

    /// Drops what the layer keeps of `tile_id`.
    pub(crate) fn drop_value(&mut self, tile_id: &OverscaledTileID) {
        if let Some(entry) = self.entry(tile_id) {
            *entry.value = None;
        }
    }

    /// The entry of `tile_id`, if the tile is one of the frame's.
    pub(crate) fn entry(&mut self, tile_id: &OverscaledTileID) -> Option<TileEntry<'_, T>> {
        let index = self.set.tiles.index_of(tile_id)?;
        let (_, state) = self.set.tiles.get_index_mut(index)?;
        Some(TileEntry { state, value: self.values.get_mut(index)? })
    }
}

/// The entry of a tile in a [`TileMap`].
pub(crate) struct TileEntry<'a, T> {
    state: &'a mut TileState,
    /// What the layer keeps of the tile, if anything.
    pub(crate) value: &'a mut Option<T>,
}

impl<T> TileEntry<'_, T> {
    /// The bucket the tile's draws were built from, if they were.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn bucket(&self) -> Option<SimpleIdentity> {
        self.state.bucket
    }

    /// Records that the tile's draws are built from `bucket`. Where they
    /// were built from another bucket, this drops what the layer keeps of
    /// the tile. Returns whether the bucket changed, which drops the changes
    /// of the one before.
    pub(crate) fn set_bucket(&mut self, bucket: SimpleIdentity) -> bool {
        if self.state.bucket == Some(bucket) {
            return false;
        }
        if self.state.bucket.is_some() {
            *self.value = None;
        }
        self.state.bucket = Some(bucket);
        self.state.changes = BucketChanges::default();
        true
    }

    /// What of the tile's bucket changed since its draws last copied it,
    /// which they are about to copy again.
    pub(crate) fn take_changes(&mut self) -> BucketChanges {
        std::mem::take(&mut self.state.changes)
    }
}

/// The GPU objects of one draw a layer keeps, with the id that orders the
/// draws by when their objects were made.
pub(crate) struct KeptDraw {
    pub(crate) id: SimpleIdentity,
    pub(crate) objects: DrawObjects,
}

impl KeptDraw {
    #[must_use]
    pub(crate) fn new(objects: DrawObjects) -> KeptDraw {
        KeptDraw { id: SimpleIdentity::new(), objects }
    }

    /// A draw of the objects, which `tile` holds, or the layer for `None`
    /// (see [`FrameDraw::new`]).
    #[must_use]
    pub(crate) fn frame_draw(&self, tile: Option<OverscaledTileID>) -> FrameDraw {
        FrameDraw::new(tile, self.id)
    }
}

/// What a layer keeps of a tile, whose draws a frame's draws name by a part
/// of the layer's own type.
pub(crate) trait TileObjects {
    /// Which of the tile's draws a frame's draw draws.
    type Part: Copy;

    /// The GPU objects of the tile's draw `part`, with what gives them the
    /// textures of the tile's atlases.
    fn objects(&mut self, part: Self::Part) -> Option<(&mut DrawObjects, Option<&AtlasBinding>)>;
}

impl TileObjects for KeptDraw {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<(&mut DrawObjects, Option<&AtlasBinding>)> {
        Some((&mut self.objects, None))
    }
}

/// The GPU objects of `draw`, `part` of its tile's, as
/// [`LayerDraws::upload`] and [`LayerDraws::render`] find them.
pub(crate) fn tile_objects<'a, T: TileObjects>(
    tiles: &'a mut TileMap<T>,
    draw: &FrameDraw,
    part: T::Part,
) -> Option<(&'a mut DrawObjects, Option<&'a AtlasBinding>)> {
    tiles.get_mut(&draw.tile?)?.objects(part)
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

/// One draw of a frame: the state it draws its GPU objects with. Its uniform
/// values are the entries at its index in the storage buffers of its
/// [`FrameDraws`].
pub(crate) struct FrameDraw {
    /// The tile whose draw it is, or `None` for a draw of the layer's.
    pub(crate) tile: Option<OverscaledTileID>,
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
}

impl FrameDraw {
    /// A draw of the GPU objects `id`, of `tile` or of the layer for `None`,
    /// in the translucent pass at the layer's depth, which it reads, without
    /// clipping.
    #[must_use]
    pub(crate) fn new(tile: Option<OverscaledTileID>, id: SimpleIdentity) -> FrameDraw {
        FrameDraw {
            tile,
            id,
            priority: 0,
            pass: RenderPass::Translucent,
            depth: DrawDepth::Layer { sub_layer: 0, mask: DepthMaskType::ReadOnly },
            clip: false,
        }
    }

    /// The order of the layer's draws: by priority, then by the id of their
    /// GPU objects, which is the order the objects were made in.
    #[must_use]
    pub(crate) fn order(&self) -> (DrawPriority, SimpleIdentity) {
        (self.priority, self.id)
    }
}

/// The draws an update makes for a frame, each with its uniform values of
/// type `U` and the part of the layer's GPU objects it binds, of type `P`,
/// before they become the draws of a [`LayerDraws`]. The list keeps its
/// memory from frame to frame, so that a frame of no more draws than one
/// before it allocates nothing.
pub(crate) struct DrawList<U, P = ()> {
    /// The draws, each with the index of its values in `uniforms` and of its
    /// part in `parts`. The draws sort without their parts, so that the
    /// sort is one function for all the layers' lists.
    draws: Vec<(FrameDraw, usize)>,
    uniforms: Vec<U>,
    parts: Vec<P>,
    /// The bytes of a storage buffer's entries, as [`UniformWriter`] writes
    /// them.
    bytes: Vec<u8>,
}

impl<U, P> Default for DrawList<U, P> {
    fn default() -> Self {
        DrawList { draws: Vec::new(), uniforms: Vec::new(), parts: Vec::new(), bytes: Vec::new() }
    }
}

impl<U> DrawList<U> {
    pub(crate) fn push(&mut self, draw: FrameDraw, uniforms: U) {
        self.push_part(draw, (), uniforms);
    }
}

impl<U, P: Copy> DrawList<U, P> {
    /// Drops the list's draws.
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
        self.uniforms.clear();
        self.parts.clear();
    }

    /// Adds `draw`, of the GPU objects of the layer's `part`.
    pub(crate) fn push_part(&mut self, draw: FrameDraw, part: P, uniforms: U) {
        self.draws.push((draw, self.uniforms.len()));
        self.uniforms.push(uniforms);
        self.parts.push(part);
    }

    /// Makes the list's draws `layer`'s, in [`FrameDraw::order`] and in the
    /// order they were pushed among draws of the same order, and empties the
    /// list. Where there are any, `write` first writes the buffers they read
    /// to `layer`'s.
    pub(crate) fn finish(
        &mut self,
        layer: &mut LayerDraws<P>,
        context: &Context,
        write: impl FnOnce(&mut UniformWriter<'_, U>),
    ) {
        sort_draws(&mut self.draws);
        if !self.draws.is_empty() {
            write(&mut UniformWriter {
                draws: &self.draws,
                uniforms: &self.uniforms,
                bytes: &mut self.bytes,
                buffers: &mut layer.frame.uniform_buffers,
                context,
            });
        }
        layer.parts.clear();
        layer.parts.extend(self.draws.iter().map(|&(_, index)| self.parts[index]));
        layer.frame.draws.clear();
        layer.frame.draws.extend(self.draws.drain(..).map(|(draw, _)| draw));
        self.uniforms.clear();
        self.parts.clear();
    }
}

/// Sorts `draws`, each with the index it was pushed at, in
/// [`FrameDraw::order`], and in the order they were pushed among draws of the
/// same order.
fn sort_draws(draws: &mut [(FrameDraw, usize)]) {
    draws.sort_unstable_by_key(|(draw, index)| (draw.order(), *index));
}

/// Writes the buffers that a frame's draws read, each draw's entries at its
/// index in drawing order.
pub(crate) struct UniformWriter<'a, U> {
    draws: &'a [(FrameDraw, usize)],
    uniforms: &'a [U],
    bytes: &'a mut Vec<u8>,
    buffers: &'a mut UniformBufferArray,
    context: &'a Context,
}

impl<U> UniformWriter<'_, U> {
    /// Writes each draw's `entry` of its uniform values to the storage
    /// buffer `id`.
    pub(crate) fn storage<P: bytemuck::Pod>(&mut self, id: usize, entry: impl Fn(&U) -> P) {
        self.bytes.clear();
        for &(_, index) in self.draws {
            self.bytes.extend_from_slice(bytemuck::bytes_of(&entry(&self.uniforms[index])));
        }
        self.buffers.write_storage(id, self.bytes, self.context);
    }

    /// Writes `value`, which all the draws read, to the uniform buffer `id`.
    pub(crate) fn uniform<P: bytemuck::Pod>(&mut self, id: usize, value: &P) {
        self.buffers.create_or_update_pod(id, value, self.context);
    }

    /// The buffers the draws read, by id.
    pub(crate) fn buffers(&mut self) -> &mut UniformBufferArray {
        self.buffers
    }

    pub(crate) fn context(&self) -> &Context {
        self.context
    }
}

/// Uploads `objects`, and destroys the buffers the upload replaced.
pub(crate) fn upload_objects(objects: &mut DrawObjects, context: &Context) {
    objects.upload(context);
    context.destroy_released();
}

/// Draws of a frame, and what they share.
pub(crate) struct FrameDraws {
    /// Tells the draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in [`FrameDraw::order`]. A draw's index is the index of its
    /// entries in the storage buffers, which shaders read as `globalIndex`.
    draws: Vec<FrameDraw>,
    /// The buffers the draws bind, by id.
    pub(crate) uniform_buffers: UniformBufferArray,
    /// How the draws composite onto the frame in the translucent pass, as one
    /// surface, after they draw into a texture of their own; `None` where
    /// they draw straight onto the frame.
    composite: Option<Composite>,
    /// The tiles whose masks the stencil buffer holds when the draws draw.
    /// Draws that don't clip need none, and neither does a layer without
    /// tiles, whose draws are none.
    stencil_tiles: Vec<UnwrappedTileID>,
}

impl FrameDraws {
    #[must_use]
    pub(crate) fn new() -> FrameDraws {
        FrameDraws {
            id: SimpleIdentity::new(),
            draws: Vec::new(),
            uniform_buffers: UniformBufferArray::new(),
            composite: None,
            stencil_tiles: Vec::new(),
        }
    }

    /// Adds `draw`, which draws after the others.
    pub(crate) fn push(&mut self, draw: FrameDraw) {
        self.draws.push(draw);
    }

    /// Drops the draws.
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
    }

    /// Makes the draws composite onto the frame as `composite` says, or
    /// draw straight onto it for `None`.
    pub(crate) fn set_composite(&mut self, composite: Option<Composite>) {
        self.composite = composite;
    }

    /// Makes `tiles` those whose masks the stencil buffer holds when the
    /// draws draw.
    pub(crate) fn set_stencil_tiles(&mut self, tiles: impl IntoIterator<Item = UnwrappedTileID>) {
        self.stencil_tiles.clear();
        self.stencil_tiles.extend(tiles);
    }

    /// How the draws composite onto the frame, with the id that tells them
    /// apart in the layer composite, if there are any and they composite.
    #[must_use]
    pub(crate) fn composited(&self) -> Option<(SimpleIdentity, Composite)> {
        self.composite.filter(|_| !self.draws.is_empty()).map(|composite| (self.id, composite))
    }

    /// Begins drawing the draws of `parameters`' pass into the open pass of
    /// `gpu`, the context's: into the layer composite's texture in the
    /// translucent pass where they composite, and onto the frame otherwise.
    /// Where none of the pass's draws is 3D, this draws the masks of the
    /// tiles that those that clip clip to; 3D draws that clip share one
    /// stencil value of their own. `None` where there is nothing to draw
    /// into or there are no draws; [`DrawPass::end`] ends what this begins.
    pub(crate) fn begin(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<DrawPass<'_>> {
        if self.draws.is_empty() || parameters.pass_target().is_none() {
            return None;
        }
        let composited = (self.composite.is_some() && parameters.pass == RenderPass::Translucent)
            .then(|| parameters.begin_composited(gpu, self.id))
            .flatten();
        let drawing = composited.as_ref().is_none_or(Composited::draws);
        let draws = if drawing { &self.draws[..] } else { &[] };
        let mut pass = DrawPass { frame: self, draws, pass: parameters.pass, volume_stencil: None, composited };
        let (volume, volume_clips) = pass.draws().fold((false, false), |(volume, clips), (_, draw)| match draw.depth {
            DrawDepth::Volume { .. } => (true, clips || draw.clip),
            _ => (volume, clips),
        });
        if volume {
            pass.volume_stencil = volume_clips.then(|| parameters.stencil_mode_for_3d(gpu));
        } else if drawing && !self.stencil_tiles.is_empty() {
            parameters.render_tile_clipping_masks(gpu, &self.stencil_tiles);
        }
        Some(pass)
    }

    /// Uploads `objects`, which all the draws bind, if there are any draws.
    pub(crate) fn upload_one(&self, objects: &mut DrawObjects, context: &Context) {
        if !self.draws.is_empty() {
            upload_objects(objects, context);
        }
    }

    /// Draws the pass's draws, all of which bind `objects`, into the open
    /// pass of `gpu`, the context's.
    pub(crate) fn render_one(
        &self,
        objects: &mut DrawObjects,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
    ) {
        let Some(pass) = self.begin(gpu, parameters) else {
            return;
        };
        for (index, _) in pass.draws() {
            pass.draw(gpu, parameters, objects, index, None);
        }
        pass.end(gpu, parameters);
    }
}

/// The draws of a pass of a [`FrameDraws`], from [`FrameDraws::begin`] to
/// [`DrawPass::end`], which every pass that begins must reach.
#[must_use = "a pass's draws end with `DrawPass::end`"]
pub(crate) struct DrawPass<'a> {
    frame: &'a FrameDraws,
    /// The frame's draws, none where their layer composite's pass didn't
    /// begin.
    draws: &'a [FrameDraw],
    pass: RenderPass,
    /// The stencil of the 3D draws that clip.
    volume_stencil: Option<StencilMode>,
    composited: Option<Composited>,
}

impl<'a> DrawPass<'a> {
    /// The draws of the pass, in drawing order, each with its index.
    pub(crate) fn draws(&self) -> impl Iterator<Item = (usize, &'a FrameDraw)> + use<'a> {
        let pass = self.pass;
        self.draws.iter().enumerate().filter(move |(_, draw)| draw.pass & pass != RenderPass::None)
    }

    /// Draws `objects`, the GPU objects of the draw at `index`, with the
    /// textures of `atlases` where they sample a tile's atlases.
    pub(crate) fn draw(
        &self,
        gpu: &mut dyn Gpu,
        parameters: &PaintParameters<'_>,
        objects: &mut DrawObjects,
        index: usize,
        atlases: Option<&AtlasBinding>,
    ) {
        let draw = &self.draws[index];
        if let Some(atlases) = atlases {
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
                self.volume_stencil.filter(|_| draw.clip).unwrap_or_else(StencilMode::disabled),
            ),
        };
        objects.draw(gpu, parameters, &self.frame.uniform_buffers, depth, stencil, index as u32);
    }

    /// Ends the pass's draws: composites what they drew where they drew
    /// into the layer composite's texture.
    pub(crate) fn end(self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(composited) = self.composited {
            parameters.end_composited(gpu, composited);
        }
    }
}

/// A layer's draws of a frame, each with the part of the layer's GPU objects
/// it binds, of the layer's type `P`.
pub(crate) struct LayerDraws<P = ()> {
    frame: FrameDraws,
    /// Each draw's part, at the draw's index.
    parts: Vec<P>,
}

impl<P> Default for LayerDraws<P> {
    fn default() -> Self {
        LayerDraws { frame: FrameDraws::new(), parts: Vec::new() }
    }
}

impl<P: Copy> LayerDraws<P> {
    #[must_use]
    pub(crate) fn new() -> LayerDraws<P> {
        LayerDraws::default()
    }

    /// The draws, with what they share.
    #[must_use]
    pub(crate) fn frame(&self) -> &FrameDraws {
        &self.frame
    }

    /// See [`FrameDraws::set_composite`].
    pub(crate) fn set_composite(&mut self, composite: Option<Composite>) {
        self.frame.set_composite(composite);
    }

    /// See [`FrameDraws::set_stencil_tiles`].
    pub(crate) fn set_stencil_tiles(&mut self, tiles: impl IntoIterator<Item = UnwrappedTileID>) {
        self.frame.set_stencil_tiles(tiles);
    }

    /// Drops the draws.
    pub(crate) fn clear(&mut self) {
        self.frame.draws.clear();
        self.parts.clear();
    }

    /// Uploads the GPU objects of the draws, in drawing order, which
    /// `objects` finds in `caches`.
    pub(crate) fn upload<C: ?Sized>(
        &self,
        caches: &mut C,
        objects: impl for<'c> Fn(&'c mut C, &FrameDraw, P) -> Option<(&'c mut DrawObjects, Option<&'c AtlasBinding>)>,
        context: &Context,
    ) {
        for (draw, &part) in self.frame.draws.iter().zip(&self.parts) {
            if let Some((objects, _)) = objects(caches, draw, part) {
                upload_objects(objects, context);
            }
        }
    }

    /// Draws the pass's draws, whose GPU objects `objects` finds in
    /// `caches`, into the open pass of `gpu`, the context's.
    pub(crate) fn render<C: ?Sized>(
        &self,
        caches: &mut C,
        objects: impl for<'c> Fn(&'c mut C, &FrameDraw, P) -> Option<(&'c mut DrawObjects, Option<&'c AtlasBinding>)>,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
    ) {
        let Some(pass) = self.frame.begin(gpu, parameters) else {
            return;
        };
        for (index, draw) in pass.draws() {
            match objects(caches, draw, self.parts[index]) {
                Some((objects, atlases)) => pass.draw(gpu, parameters, objects, index, atlases),
                None => debug_assert!(false, "a draw of tile {:?} has no GPU objects", draw.tile),
            }
        }
        pass.end(gpu, parameters);
    }
}

/// One draw's GPU objects and a frame's draws of them, with the buffers they
/// bind.
pub(crate) struct SingleDraw {
    pub(crate) objects: DrawObjects,
    pub(crate) frame: FrameDraws,
}

impl SingleDraw {
    /// One draw of `objects`, as `draw` draws, without buffers.
    #[must_use]
    pub(crate) fn new(objects: DrawObjects, draw: FrameDraw) -> SingleDraw {
        let mut frame = FrameDraws::new();
        frame.push(draw);
        SingleDraw { objects, frame }
    }

    pub(crate) fn upload(&mut self, context: &Context) {
        self.frame.upload_one(&mut self.objects, context);
    }

    /// Draws the draw, if it draws in the pass, into the open pass of `gpu`,
    /// the context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render_one(&mut self.objects, gpu, parameters);
    }
}

/// What the frame draws of a layer in one place in the layers' order and
/// depth.
#[derive(Clone, Copy)]
pub(crate) enum DrawGroup<'a> {
    /// Draws of the layer's GPU objects.
    Draws(&'a FrameDraws),
    /// What a custom layer's host draws, with objects of its own.
    Custom,
}

impl<'a> DrawGroup<'a> {
    /// The tiles whose masks the stencil buffer holds when the group draws;
    /// `None` for a group that needs no masks.
    #[must_use]
    pub(crate) fn stencil_tiles(self) -> Option<&'a [UnwrappedTileID]> {
        match self {
            DrawGroup::Draws(frame) => Some(&frame.stencil_tiles[..]).filter(|tiles| !tiles.is_empty()),
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

/// The order the frame draws the layers' draw groups in, each at the depth
/// of [`PaintParameters::current_layer`], which [`DrawOrder::step`] moves to
/// the next group's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawOrder {
    /// The layers' order, bottom to top, with the depth going down by one
    /// after each group, to 0.
    BottomUp,
    /// The reverse order, top to bottom, with the depth going up by one
    /// after each group.
    TopDown,
}

impl DrawOrder {
    /// Moves `parameters` to the depth of the group after the one it is at.
    pub(crate) fn step(self, parameters: &mut PaintParameters<'_>) {
        match self {
            DrawOrder::BottomUp => parameters.current_layer = parameters.current_layer.saturating_sub(1),
            DrawOrder::TopDown => parameters.current_layer += 1,
        }
    }
}

/// The drawing of a layer that draws one group of draws, each of a part of
/// the tile it draws, and keeps a `T` of each tile.
pub(crate) struct TileDraws<T: TileObjects> {
    pub(crate) tiles: TileMap<T>,
    /// The frame's draws, from the layer's first update with something to
    /// draw.
    pub(crate) frame: Option<LayerDraws<T::Part>>,
}

impl<T: TileObjects> Default for TileDraws<T> {
    fn default() -> Self {
        TileDraws { tiles: TileMap::default(), frame: None }
    }
}

impl<T: TileObjects> TileDraws<T> {
    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload_draws(&mut self, context: &Context) {
        if let Some(frame) = &self.frame {
            frame.upload(&mut self.tiles, tile_objects, context);
        }
    }

    /// Draws the pass's draws of the frame into the open pass of `gpu`, the
    /// context's. Returns whether the layer has their group.
    pub(crate) fn render_draws(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        let Some(frame) = &self.frame else {
            return false;
        };
        frame.render(&mut self.tiles, tile_objects, gpu, parameters);
        true
    }
}

impl<T: TileObjects> LayerDrawing for TileDraws<T> {
    type Tile = T;

    fn tiles(&mut self) -> &mut TileMap<T> {
        &mut self.tiles
    }

    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a FrameDraws)) {
        if let Some(frame) = &self.frame {
            visit(&frame.frame);
        }
    }

    fn clear(&mut self) {
        self.tiles.clear();
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn upload(&mut self, context: &Context) {
        self.upload_draws(context);
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if self.render_draws(gpu, parameters) {
            order.step(parameters);
        }
    }
}

/// What a layer type keeps of its drawing: the GPU objects of its draws, in
/// types of its own, those of its tiles in a [`TileMap`], and its draw
/// groups of the frame, whose draws it uploads and draws in loops of its
/// own.
pub(crate) trait LayerDrawing {
    /// What the layer keeps of a tile.
    type Tile;

    /// What the layer keeps of the frame's tiles.
    fn tiles(&mut self) -> &mut TileMap<Self::Tile>;

    /// Visits the layer's draw groups of the frame, in drawing order: from
    /// its first update with something to draw, a group of its draws, and
    /// any of draws over them. While the layer is renderable, a group takes
    /// a place in the layers' order and depth, even when it has no draws.
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a FrameDraws));

    /// Drops the frame's draws, and the GPU objects of the tiles' draws and
    /// of the others. The tiles' entries stay.
    fn clear(&mut self);

    /// Uploads the GPU objects of the draw groups' draws.
    fn upload(&mut self, context: &Context);

    /// Draws the draw groups' draws of the pass into the open pass of `gpu`,
    /// the context's, in `order`, each at its depth.
    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder);
}

#[cfg(test)]
mod tests {
    use mln_tile_id::OverscaledTileID;
    use mln_util::identity::SimpleIdentity;

    use super::{BucketChanges, ChangedBuckets, TileMap};

    /// Sets what `tiles` keeps of `tile_id` to `value`, if the tile is one
    /// of the map's; returns whether it is.
    fn set<T>(tiles: &mut TileMap<T>, tile_id: &OverscaledTileID, value: T) -> bool {
        tiles.entry(tile_id).map(|entry| *entry.value = Some(value)).is_some()
    }

    /// The bucket of `tile_id` in `tiles`.
    fn bucket_of<T>(tiles: &mut TileMap<T>, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
        tiles.entry(tile_id)?.bucket()
    }

    /// A tile's changes are those of the bucket its draws were built from,
    /// until the draws take them; a tile that leaves the frame
    /// takes its entry with it.
    #[test]
    fn changes_follow_the_tiles_bucket() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut tiles = TileMap::<()>::default();
        tiles.set_tiles([tile, other]);
        assert!(tiles.entry(&tile).is_some_and(|mut entry| entry.set_bucket(bucket)));
        assert!(tiles.entry(&tile).is_some_and(|mut entry| !entry.set_bucket(bucket)));
        assert!(tiles.entry(&other).is_some_and(|mut entry| entry.set_bucket(other_bucket)));

        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        changed.mark(bucket, BucketChanges { sort: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);

        let mut take = |tile_id| tiles.entry(tile_id).map(|mut entry| entry.take_changes());
        assert_eq!(Some(BucketChanges { paint: true, sort: true, ..BucketChanges::default() }), take(&tile));
        assert_eq!(Some(BucketChanges::default()), take(&tile));
        assert_eq!(Some(BucketChanges::default()), take(&other));

        tiles.set_tiles([other]);
        assert!(!tiles.has_tile(&tile));
        assert_eq!(Some(other_bucket), bucket_of(&mut tiles, &other));
        tiles.shift_wraps(1);
        assert_eq!(Some(other_bucket), bucket_of(&mut tiles, &other.unwrap_to(1)));
    }

    /// A tile whose draws are built from another bucket drops what the layer
    /// kept of it, and the changes of the bucket before.
    #[test]
    fn another_bucket_drops_the_value() {
        let tile = OverscaledTileID::with_zxy(1, 0, 1, 1, 0);
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([tile]);
        assert!(set(&mut tiles, &tile, 1));
        assert!(tiles.entry(&tile).is_some_and(|mut entry| entry.set_bucket(bucket)));
        assert_eq!(Some(&1), tiles.get(&tile));
        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);
        assert!(tiles.entry(&tile).is_some_and(|mut entry| entry.set_bucket(other_bucket)));
        assert!(tiles.get(&tile).is_none());
        assert_eq!(Some(BucketChanges::default()), tiles.entry(&tile).map(|mut entry| entry.take_changes()));
    }

    /// Each tile keeps its value and bucket as tiles before and after it
    /// come and go, and in whatever order the frame gives the tiles.
    #[test]
    fn values_stay_with_their_tiles() {
        let ids: Vec<OverscaledTileID> = (0..6).map(|x| OverscaledTileID::with_zxy(3, 0, 3, x, 2)).collect();
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([ids[4], ids[1], ids[2]]);
        for &i in &[1, 2, 4] {
            assert!(set(&mut tiles, &ids[i], i as u32));
        }
        let bucket = SimpleIdentity::new();
        assert!(tiles.entry(&ids[2]).is_some_and(|mut entry| entry.set_bucket(bucket)));
        tiles.set_tiles([ids[5], ids[2], ids[0], ids[4], ids[3]]);
        let values: Vec<Option<u32>> = ids.iter().map(|id| tiles.get(id).copied()).collect();
        assert_eq!(vec![None, None, Some(2), None, Some(4), None], values);
        assert_eq!(Some(bucket), bucket_of(&mut tiles, &ids[2]));
        assert!(set(&mut tiles, &ids[0], 0));
        tiles.set_tiles([ids[4], ids[0], ids[4]]);
        let values: Vec<Option<u32>> = ids.iter().map(|id| tiles.get(id).copied()).collect();
        assert_eq!(vec![Some(0), None, None, None, Some(4), None], values);
        assert!(!tiles.has_tile(&ids[2]));
    }

    /// What the layer keeps of a tile moves with the tile across a wrap jump,
    /// even one that changes the tiles' order, and as other tiles come and
    /// go, and goes when the tile leaves the frame or the map is cleared.
    #[test]
    fn caches_follow_their_tiles() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([tile, other]);
        assert!(set(&mut tiles, &tile, 1));
        assert!(set(&mut tiles, &other, 2));
        assert!(!set(&mut tiles, &OverscaledTileID::with_zxy(1, 0, 1, 1, 1), 3));

        tiles.shift_wraps(1);
        let moved = tile.unwrap_to(1);
        assert_eq!(Some(&mut 1), tiles.get_mut(&moved));
        tiles.set_tiles([moved]);
        assert!(tiles.get_mut(&other.unwrap_to(1)).is_none());
        tiles.set_tiles([other, moved]);
        assert!(tiles.get_mut(&other).is_none());
        assert_eq!(Some(&1), tiles.get(&moved));
        tiles.set_tiles([moved]);
        assert_eq!(Some(&1), tiles.get(&moved));

        let (high, low) = (tile.unwrap_to(i16::MAX), other.unwrap_to(i16::MAX - 1));
        tiles.set_tiles([high, low]);
        assert!(set(&mut tiles, &high, 3));
        assert!(set(&mut tiles, &low, 4));
        tiles.shift_wraps(1);
        assert_eq!(Some(&3), tiles.get(&tile.unwrap_to(i16::MIN)));
        assert_eq!(Some(&4), tiles.get(&other.unwrap_to(i16::MAX)));

        tiles.clear();
        assert!(tiles.has_tile(&other.unwrap_to(i16::MAX)));
        assert!(tiles.get(&other.unwrap_to(i16::MAX)).is_none());
    }
}
