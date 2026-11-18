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
//! [`DrawPass::draw`] draws one draw's objects.

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
    /// The tiles `rebuild` makes the set's, in any order.
    next: Vec<OverscaledTileID>,
    /// The map `rebuild` builds, kept for its memory.
    spare: VecMap<OverscaledTileID, TileState>,
    /// For each tile of the set, the index it had before the last
    /// `rebuild`, `None` for a tile new to the set.
    moves: Vec<Option<usize>>,
}

impl TileSet {
    /// Makes the tiles of `next`, each `wrap_delta` world copies east of
    /// the one whose entry it takes, the set's, and drops the entries no
    /// tile takes. Returns whether any tile's index changed, which `moves`
    /// then says.
    fn rebuild(&mut self, wrap_delta: i16) -> bool {
        self.spare.clear();
        for id in &self.next {
            self.spare.insert(*id, TileState::default());
        }
        if wrap_delta == 0 && self.spare.keys().eq(self.tiles.keys()) {
            return false;
        }
        self.moves.clear();
        for index in 0..self.spare.len() {
            let Some((id, state)) = self.spare.get_index_mut(index) else {
                break;
            };
            let old = self.tiles.index_of(&id.unwrap_to(id.wrap.wrapping_sub(wrap_delta)));
            if let Some((_, old_state)) = old.and_then(|old| self.tiles.get_index_mut(old)) {
                *state = std::mem::take(old_state);
            }
            self.moves.push(old);
        }
        std::mem::swap(&mut self.tiles, &mut self.spare);
        let old_len = self.spare.len();
        self.spare.clear();
        old_len != self.moves.len() || self.moves.iter().enumerate().any(|(i, old)| *old != Some(i))
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
    /// The values `apply_moves` builds, kept for their memory.
    spare: Vec<Option<T>>,
}

impl<T> Default for TileMap<T> {
    fn default() -> Self {
        TileMap { set: TileSet::default(), values: Vec::new(), spare: Vec::new() }
    }
}

impl<T> TileMap<T> {
    /// Makes `tiles`, the frame's tiles of the layer, those the map has
    /// entries for: it keeps the entries of those it had, and drops the
    /// others.
    pub(crate) fn set_tiles(&mut self, tiles: impl IntoIterator<Item = OverscaledTileID>) {
        self.set.next.clear();
        self.set.next.extend(tiles);
        self.rebuild(0);
    }

    /// Drops the entries of the tiles.
    pub(crate) fn clear_tiles(&mut self) {
        self.set.tiles.clear();
        self.values.clear();
    }

    /// Moves the tiles' entries to the tiles `wrap_delta` world copies east
    /// of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        self.set.next.clear();
        self.set.next.extend(self.set.tiles.keys().map(|id| id.unwrap_to(id.wrap.wrapping_add(wrap_delta))));
        self.rebuild(wrap_delta);
    }

    fn rebuild(&mut self, wrap_delta: i16) {
        if !self.set.rebuild(wrap_delta) {
            return;
        }
        let values = &mut self.values;
        self.spare.clear();
        self.spare.extend(self.set.moves.iter().map(|old| old.and_then(|i| values.get_mut(i)?.take())));
        std::mem::swap(&mut self.values, &mut self.spare);
        self.spare.clear();
    }

    /// Adds `changed` to the changes of the tiles whose draws were built from
    /// a bucket in it.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.set.mark_changed(changed);
    }

    /// Drops what the layer keeps of the tiles. The tiles' entries stay.
    pub(crate) fn clear(&mut self) {
        for value in &mut self.values {
            *value = None;
        }
    }

    /// Whether `tile_id` is one of the frame's tiles of the layer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_tile(&self, tile_id: &OverscaledTileID) -> bool {
        self.set.tiles.contains_key(tile_id)
    }

    /// The bucket the draws of `tile_id` were built from, if the tile is one
    /// of the frame's and they were.
    #[must_use]
    pub(crate) fn bucket(&self, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
        self.set.tiles.get(tile_id).and_then(|state| state.bucket)
    }

    /// Records that the draws of `tile_id` are built from `bucket`, which
    /// drops the changes of the bucket they were built from before. Returns
    /// whether that changed it; a tile that isn't one of the frame's takes no
    /// bucket.
    pub(crate) fn set_bucket(&mut self, tile_id: &OverscaledTileID, bucket: SimpleIdentity) -> bool {
        match self.set.tiles.get_mut(tile_id) {
            Some(state) if state.bucket != Some(bucket) => {
                state.bucket = Some(bucket);
                state.changes = BucketChanges::default();
                true
            }
            _ => false,
        }
    }

    /// What of the bucket of `tile_id` changed since its draws last copied
    /// it, which they are about to copy again.
    pub(crate) fn take_changes(&mut self, tile_id: &OverscaledTileID) -> BucketChanges {
        self.set.tiles.get_mut(tile_id).map(|state| std::mem::take(&mut state.changes)).unwrap_or_default()
    }

    /// What the layer keeps of `tile_id`, if the tile is one of the frame's
    /// and the layer keeps anything of it.
    #[must_use]
    pub(crate) fn cache(&self, tile_id: &OverscaledTileID) -> Option<&T> {
        self.values.get(self.set.tiles.index_of(tile_id)?)?.as_ref()
    }

    /// What the layer keeps of `tile_id`, if the tile is one of the frame's
    /// and the layer keeps anything of it.
    pub(crate) fn cache_mut(&mut self, tile_id: &OverscaledTileID) -> Option<&mut T> {
        self.values.get_mut(self.set.tiles.index_of(tile_id)?)?.as_mut()
    }

    /// Makes `value` what the layer keeps of `tile_id`, if the tile is one
    /// of the frame's, and returns it.
    pub(crate) fn set_cache(&mut self, tile_id: &OverscaledTileID, value: T) -> Option<&mut T> {
        let index = self.set.tiles.index_of(tile_id)?;
        if self.values.len() <= index {
            self.values.resize_with(index + 1, || None);
        }
        Some(self.values[index].insert(value))
    }

    /// Drops what the layer keeps of `tile_id`.
    pub(crate) fn drop_cache(&mut self, tile_id: &OverscaledTileID) {
        if let Some(value) = self.set.tiles.index_of(tile_id).and_then(|index| self.values.get_mut(index)) {
            *value = None;
        }
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

/// The GPU objects of the draw of a tile of a layer that keeps one draw of
/// each tile, as [`LayerDraws::upload`] and [`LayerDraws::render`] find
/// them.
pub(crate) fn kept_tile_draw<'a>(
    tiles: &'a mut TileMap<KeptDraw>,
    draw: &FrameDraw,
    (): (),
) -> Option<(&'a mut DrawObjects, Option<&'a AtlasBinding>)> {
    Some((&mut tiles.cache_mut(&draw.tile?)?.objects, None))
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
    /// part in `parts`.
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

    /// Adds `draw`, of the GPU objects of the layer's `part` of its tile.
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
        self.draws.sort_unstable_by_key(|(draw, index)| (draw.order(), *index));
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
    pub(crate) draws: Vec<FrameDraw>,
    /// The buffers the draws bind, by id.
    pub(crate) uniform_buffers: UniformBufferArray,
    /// How the draws composite onto the frame in the translucent pass, as one
    /// surface, after they draw into a texture of their own; `None` where
    /// they draw straight onto the frame.
    pub(crate) composite: Option<Composite>,
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
        let mut pass = DrawPass {
            frame: self,
            pass: parameters.pass.underlying_type(),
            drawing: composited.as_ref().is_none_or(Composited::draws),
            volume_stencil: None,
            composited,
        };
        if pass.drawing {
            let in_pass = pass.draws().map(|(_, draw)| draw);
            let (volume, volume_clips) = in_pass.fold((false, false), |(volume, clips), draw| match draw.depth {
                DrawDepth::Volume { .. } => (true, clips || draw.clip),
                _ => (volume, clips),
            });
            if volume {
                pass.volume_stencil = volume_clips.then(|| parameters.stencil_mode_for_3d(gpu));
            } else if !self.stencil_tiles.is_empty() {
                parameters.render_tile_clipping_masks(gpu, &self.stencil_tiles);
            }
        }
        Some(pass)
    }

    /// Uploads `objects`, which all the draws bind, if there are any draws.
    pub(crate) fn upload_one(&self, objects: Option<&mut DrawObjects>, context: &Context) {
        if let Some(objects) = objects.filter(|_| !self.draws.is_empty()) {
            upload_objects(objects, context);
        }
    }

    /// Draws the pass's draws, all of which bind `objects`, into the open
    /// pass of `gpu`, the context's.
    pub(crate) fn render_one(
        &self,
        mut objects: Option<&mut DrawObjects>,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
    ) {
        let Some(pass) = self.begin(gpu, parameters) else {
            return;
        };
        for (index, _) in pass.draws() {
            match objects.as_deref_mut() {
                Some(objects) => pass.draw(gpu, parameters, objects, index, None),
                None => debug_assert!(false, "a draw has no GPU objects"),
            }
        }
        pass.end(gpu, parameters);
    }
}

/// The draws of a pass of a [`FrameDraws`], from [`FrameDraws::begin`] to
/// [`DrawPass::end`].
pub(crate) struct DrawPass<'a> {
    frame: &'a FrameDraws,
    /// The pass, as a [`RenderPass`]'s bits.
    pass: u8,
    /// Whether the draws draw: false where their layer composite's pass
    /// didn't begin.
    drawing: bool,
    /// The stencil of the 3D draws that clip.
    volume_stencil: Option<StencilMode>,
    composited: Option<Composited>,
}

impl<'a> DrawPass<'a> {
    /// The draws of the pass, in drawing order, each with its index.
    pub(crate) fn draws(&self) -> impl Iterator<Item = (usize, &'a FrameDraw)> + use<'a> {
        let (pass, drawing) = (self.pass, self.drawing);
        self.frame.draws.iter().enumerate().filter(move |(_, draw)| drawing && draw.pass.underlying_type() & pass != 0)
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
        let draw = &self.frame.draws[index];
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
    pub(crate) frame: FrameDraws,
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
        frame.draws.push(draw);
        SingleDraw { objects, frame }
    }

    pub(crate) fn upload(&mut self, context: &Context) {
        self.frame.upload_one(Some(&mut self.objects), context);
    }

    /// Draws the draw, if it draws in the pass, into the open pass of `gpu`,
    /// the context's.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render_one(Some(&mut self.objects), gpu, parameters);
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

/// The drawing of a layer that keeps one draw of each tile it draws, and
/// draws one group.
#[derive(Default)]
pub(crate) struct TileDraws {
    pub(crate) tiles: TileMap<KeptDraw>,
    pub(crate) frame: Option<LayerDraws>,
}

impl LayerDrawing for TileDraws {
    type Tile = KeptDraw;

    fn tiles(&mut self) -> &mut TileMap<KeptDraw> {
        &mut self.tiles
    }

    fn groups(&self) -> [Option<&FrameDraws>; 2] {
        [self.frame.as_ref().map(|frame| &frame.frame), None]
    }

    fn clear(&mut self) {
        self.tiles.clear();
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn upload(&mut self, context: &Context) {
        if let Some(frame) = &self.frame {
            frame.upload(&mut self.tiles, kept_tile_draw, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(frame) = &self.frame {
            frame.render(&mut self.tiles, kept_tile_draw, gpu, parameters);
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

    /// The layer's draw groups of the frame, in drawing order: a group of
    /// its draws, from its first update with something to draw, and one of
    /// draws over them, for debugging. While the layer is renderable, a
    /// group takes a place in the layers' order and depth, even when it has
    /// no draws.
    fn groups(&self) -> [Option<&FrameDraws>; 2];

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

    /// A tile's changes are those of the bucket its draws were built from,
    /// until the draws take them; a tile that leaves the frame
    /// takes its entry with it.
    #[test]
    fn changes_follow_the_tiles_bucket() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut tiles = TileMap::<()>::default();
        tiles.set_tiles([tile, other]);
        assert!(tiles.set_bucket(&tile, bucket));
        assert!(!tiles.set_bucket(&tile, bucket));
        assert!(tiles.set_bucket(&other, other_bucket));

        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        changed.mark(bucket, BucketChanges { sort: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);

        assert_eq!(BucketChanges { paint: true, sort: true, ..BucketChanges::default() }, tiles.take_changes(&tile));
        assert_eq!(BucketChanges::default(), tiles.take_changes(&tile));
        assert_eq!(BucketChanges::default(), tiles.take_changes(&other));

        tiles.set_tiles([other]);
        assert!(!tiles.has_tile(&tile));
        assert_eq!(Some(other_bucket), tiles.bucket(&other));
        tiles.shift_wraps(1);
        assert_eq!(Some(other_bucket), tiles.bucket(&other.unwrap_to(1)));
    }

    /// What the layer keeps of a tile moves with the tile across a wrap jump
    /// and as other tiles come and go, and goes when the tile leaves the
    /// frame or the map is cleared.
    #[test]
    fn caches_follow_their_tiles() {
        let (tile, other) = (OverscaledTileID::with_zxy(1, 0, 1, 1, 0), OverscaledTileID::with_zxy(1, 0, 1, 0, 0));
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([tile, other]);
        assert!(tiles.set_cache(&tile, 1).is_some());
        assert!(tiles.set_cache(&other, 2).is_some());
        assert!(tiles.set_cache(&OverscaledTileID::with_zxy(1, 0, 1, 1, 1), 3).is_none());

        tiles.shift_wraps(1);
        let moved = tile.unwrap_to(1);
        assert_eq!(Some(&mut 1), tiles.cache_mut(&moved));
        tiles.set_tiles([moved]);
        assert!(tiles.cache_mut(&other.unwrap_to(1)).is_none());
        tiles.set_tiles([other, moved]);
        assert!(tiles.cache_mut(&other).is_none());
        assert_eq!(Some(&1), tiles.cache(&moved));
        tiles.set_tiles([moved]);
        assert_eq!(Some(&1), tiles.cache(&moved));

        tiles.clear();
        assert!(tiles.has_tile(&moved));
        assert!(tiles.cache_mut(&moved).is_none());
    }
}
