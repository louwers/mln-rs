//! What a layer keeps of its drawing between frames, and the draws of a
//! frame.
//!
//! Each layer type keeps the meshes it draws in types of its own, those of
//! a tile in its tile value, which the engine keeps in a [`TileMap`] that
//! also says what the tile's value was made from and what of that bucket's
//! vertex data changed since. Its draw update pushes the frame's draws into
//! their [`LayerDraws`]: values ([`FrameDraw`]), each with the part of the
//! layer's meshes it draws and the values of its own of the blocks its
//! program reads of each draw ([`DrawBlocks`]), and the blocks the draws
//! share ([`LayerDraws::block`]). The engine writes them to the draws'
//! buffers ([`LayerDraws::finish`]), with a drape's paint parameters where
//! the draws go into one. The layer type then uploads and draws them in
//! loops of its own: [`FrameDraws::begin`] begins a pass's draws, and
//! [`DrawPass::draw`] draws one draw's mesh, bound with the draws' buffers.
//! A layer type that draws one group of its tiles' draws keeps them in a
//! [`TileDraws`], whose tiles name the meshes of their draws by a part of
//! their own type ([`TileObjects`]). A layer that drapes over terrain has
//! tile values and a drawing of its type in each drape too
//! ([`DrapeDrawing`]).

use std::any::Any;
use std::ops::{BitOrAssign, Range};

use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::bindings::{BUFFER_IDS, PAINT_PARAMS_ID, PROJECTION_ID};
use mln_gfx::shaders::block::{Block, DrawBlocks, UniformBlock};
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::{BufferKind, Program, Projection};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthMaskType;
use mln_style::hash::HashMap;

use crate::draw::atlas_binding::AtlasBinding;
use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::draw::objects::{DrawBuffers, Mesh};
use crate::render::layer_composite::Composite;
use crate::render::paint_parameters::{Composited, PaintParameters, StencilClear};
use crate::render::pass::RenderPass;
use crate::tile::{OverscaledTileID, UnwrappedTileID};

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

    /// Whether the bucket `bucket` changed.
    #[must_use]
    pub(crate) fn contains(&self, bucket: SimpleIdentity) -> bool {
        self.0.contains_key(&bucket)
    }

    fn get(&self, bucket: SimpleIdentity) -> Option<BucketChanges> {
        self.0.get(&bucket).copied()
    }
}

/// What the tile showed a layer that its value was made from: the
/// identity of its bucket, or of its raster or raster-dem contents, with the
/// generations of their mask or mesh and of their DEM's border.
pub(crate) type MadeFrom = (SimpleIdentity, [u64; 2]);

/// What the engine keeps of a tile a layer draws, whatever the layer's type.
#[derive(Default)]
struct TileState {
    /// What the tile's value was made from, `None` until the engine first
    /// sees the tile show the layer something.
    made_from: Option<MadeFrom>,
    /// What of the bucket's vertex data changed since the draws copied it.
    changes: BucketChanges,
}

/// The frame's tiles a layer draws, in id order, each with its
/// [`TileState`]: the part of a [`TileMap`] that doesn't depend on what the
/// layer keeps of a tile.
#[derive(Default)]
struct TileSet {
    /// The tiles' ids, in order.
    ids: Vec<OverscaledTileID>,
    /// Each tile's state, at the tile's index.
    states: Vec<TileState>,
    /// How the tiles' indexes last changed.
    moves: TileMoves,
    /// The ids that `set_tiles` makes the set's; after it, the ids the set
    /// had, whose memory the next change reuses.
    incoming: Vec<OverscaledTileID>,
    /// The index after the tile that [`find`](Self::find) last found,
    /// where it looks first.
    next: usize,
}

/// How the indexes of a set's tiles change, as swaps of the values at them
/// that move the values of the tiles that stay to their new indexes.
#[derive(Default)]
struct TileMoves {
    /// The swaps: `swaps[..split]` before the values beyond `kept` drop, and
    /// the others after the new tiles' values fill the indexes up to `len`.
    swaps: Vec<(usize, usize)>,
    split: usize,
    kept: usize,
    len: usize,
}

impl TileMoves {
    /// Moves `values`, one at each index of the set's tiles before the
    /// change, to their tiles' indexes after it, drops those of the tiles
    /// that left, and gives the new tiles `fill()`.
    fn apply<V>(&self, values: &mut Vec<V>, fill: impl FnMut() -> V) {
        let (before, after) = self.swaps.split_at(self.split);
        for &(a, b) in before {
            values.swap(a, b);
        }
        values.truncate(self.kept);
        values.resize_with(self.len, fill);
        for &(a, b) in after {
            values.swap(a, b);
        }
    }
}

impl TileSet {
    /// Makes `incoming` the set's tiles: keeps the states of those it has,
    /// and drops the others. Returns whether that changed any tile's index,
    /// which `moves` then says.
    fn set_tiles(&mut self) -> bool {
        // The stable sort, which the renderer's other sorts of tile ids
        // share, allocates nothing for a frame's tens of tiles.
        self.incoming.sort();
        self.incoming.dedup();
        if self.ids == self.incoming {
            return false;
        }
        let (ids, incoming, moves) = (&self.ids, &self.incoming, &mut self.moves);
        moves.swaps.clear();
        // The tiles that stay move to the front, in order, ahead of those
        // that leave...
        let mut kept = 0;
        for (old, _) in kept_tiles(ids, incoming) {
            if kept != old {
                moves.swaps.push((kept, old));
            }
            kept += 1;
        }
        moves.split = moves.swaps.len();
        moves.kept = kept;
        moves.len = incoming.len();
        // ...and then, from the last to the first, to their new indexes,
        // whose values are the new tiles' until then.
        for (rank, (_, new)) in kept_tiles(ids, incoming).enumerate() {
            if rank != new {
                moves.swaps.push((new, rank));
            }
        }
        moves.swaps[moves.split..].reverse();
        moves.apply(&mut self.states, TileState::default);
        std::mem::swap(&mut self.ids, &mut self.incoming);
        true
    }

    /// Moves the tiles' states to the tiles `wrap_delta` world copies east
    /// of theirs. Returns whether that changed their order, which `moves`
    /// then says; only a wrap that overflows does.
    fn shift_wraps(&mut self, wrap_delta: i16) -> bool {
        for id in &mut self.ids {
            *id = id.unwrap_to(id.wrap.wrapping_add(wrap_delta));
        }
        if self.ids.is_sorted() {
            return false;
        }
        // The shifted ids, in their old order.
        self.incoming.clone_from(&self.ids);
        self.ids.sort();
        // For each tile's old index, its new one. A wrap overflows rarely
        // enough that this allocates.
        let mut new_index: Vec<usize> =
            self.incoming.iter().map(|id| self.ids.binary_search(id).unwrap_or_default()).collect();
        let moves = &mut self.moves;
        moves.swaps.clear();
        for old in 0..new_index.len() {
            while new_index[old] != old {
                let new = new_index[old];
                moves.swaps.push((old, new));
                new_index.swap(old, new);
            }
        }
        moves.split = moves.swaps.len();
        moves.kept = self.ids.len();
        moves.len = self.ids.len();
        moves.apply(&mut self.states, TileState::default);
        true
    }

    /// The index of `tile_id`, if it is one of the set's.
    fn index_of(&self, tile_id: &OverscaledTileID) -> Option<usize> {
        self.ids.binary_search(tile_id).ok()
    }

    /// [`index_of`](Self::index_of), looking first at the index after the
    /// tile it last found: the engine readies a layer's tile values and the
    /// layer then makes its draws of them in the same order, the tiles' id
    /// order for most layer types.
    fn find(&mut self, tile_id: &OverscaledTileID) -> Option<usize> {
        let index = match self.ids.get(self.next) {
            Some(id) if id == tile_id => self.next,
            _ => self.index_of(tile_id)?,
        };
        self.next = index + 1;
        Some(index)
    }

    fn clear(&mut self) {
        self.ids.clear();
        self.states.clear();
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for state in &mut self.states {
            if let Some(changes) = state.made_from.and_then(|(bucket, _)| changed.get(bucket)) {
                state.changes |= changes;
            }
        }
    }
}

/// The tiles of both `old` and `new`, sorted ids, each with its index in
/// `old` and in `new`, in order.
fn kept_tiles<'a>(
    old: &'a [OverscaledTileID],
    new: &'a [OverscaledTileID],
) -> impl Iterator<Item = (usize, usize)> + 'a {
    let mut old_index = 0;
    new.iter().enumerate().filter_map(move |(new_index, id)| {
        while old.get(old_index).is_some_and(|old_id| old_id < id) {
            old_index += 1;
        }
        (old.get(old_index) == Some(id)).then_some((old_index, new_index))
    })
}

/// A layer's values of the frame's tiles, which the engine keeps, by tile id:
/// what each tile's value was made from and what of its bucket changed
/// since, and the value, a `T` of the layer's type, from when the layer
/// makes one ([`TypedLayer::tile`]).
///
/// [`TypedLayer::tile`]: crate::layer::render_layer::TypedLayer::tile
pub(crate) struct TileMap<T> {
    set: TileSet,
    /// Each tile's `T`, at the tile's index in the set.
    values: Vec<Option<T>>,
}

impl<T> Default for TileMap<T> {
    fn default() -> Self {
        TileMap { set: TileSet::default(), values: Vec::new() }
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
            self.set.moves.apply(&mut self.values, || None);
        }
    }

    /// Whether the map has no tiles.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.set.ids.is_empty()
    }

    /// Drops the entries of the tiles.
    pub(crate) fn clear_tiles(&mut self) {
        self.set.clear();
        self.values.clear();
    }

    /// Moves the tiles' entries to the tiles `wrap_delta` world copies east
    /// of theirs.
    pub(crate) fn shift_wraps(&mut self, wrap_delta: i16) {
        if self.set.shift_wraps(wrap_delta) {
            self.set.moves.apply(&mut self.values, || None);
        }
    }

    /// Adds `changed` to the changes of the tiles whose values were made from
    /// a bucket in it.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.set.mark_changed(changed);
    }

    /// Drops the tiles' values. The tiles' entries stay.
    pub(crate) fn clear(&mut self) {
        self.values.fill_with(|| None);
    }

    /// Whether `tile_id` is one of the frame's tiles of the layer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_tile(&self, tile_id: &OverscaledTileID) -> bool {
        self.set.index_of(tile_id).is_some()
    }

    /// What the layer keeps of `tile_id`, if the tile is one of the frame's
    /// and the layer keeps anything of it.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn get(&self, tile_id: &OverscaledTileID) -> Option<&T> {
        self.values.get(self.set.index_of(tile_id)?)?.as_ref()
    }

    /// What the layer keeps of `tile_id`, if the tile is one of the frame's
    /// and the layer keeps anything of it.
    pub(crate) fn get_mut(&mut self, tile_id: &OverscaledTileID) -> Option<&mut T> {
        self.values.get_mut(self.set.find(tile_id)?)?.as_mut()
    }

    /// Drops what the layer keeps of `tile_id`.
    pub(crate) fn drop_value(&mut self, tile_id: &OverscaledTileID) {
        if let Some(value) = self.set.find(tile_id).and_then(|index| self.values.get_mut(index)) {
            *value = None;
        }
    }

    /// The tiles, in order, each with its entry.
    pub(crate) fn entries(&mut self) -> impl Iterator<Item = (OverscaledTileID, TileEntry<'_, T>)> {
        let TileSet { ids, states, .. } = &mut self.set;
        ids.iter().zip(states).zip(&mut self.values).map(|((&id, state), value)| (id, TileEntry { state, value }))
    }

    /// The entry of `tile_id`, if the tile is one of the frame's.
    pub(crate) fn entry(&mut self, tile_id: &OverscaledTileID) -> Option<TileEntry<'_, T>> {
        let index = self.set.find(tile_id)?;
        Some(TileEntry { state: self.set.states.get_mut(index)?, value: self.values.get_mut(index)? })
    }
}

/// The entry of a tile in a [`TileMap`].
pub(crate) struct TileEntry<'a, T> {
    state: &'a mut TileState,
    /// What the layer keeps of the tile, if anything.
    pub(crate) value: &'a mut Option<T>,
}

impl<T> TileEntry<'_, T> {
    /// What the tile's value was made from, if the engine saw it show the
    /// layer something.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn made_from(&self) -> Option<MadeFrom> {
        self.state.made_from
    }

    /// Records what the tile shows the layer: `made_from`, or `None` for
    /// nothing. The tile's value drops for nothing, and where it was made
    /// from something else, which drops the changes of what it was made
    /// from too.
    pub(crate) fn show(&mut self, made_from: Option<MadeFrom>) {
        match made_from {
            None => *self.value = None,
            Some(made_from) if self.state.made_from != Some(made_from) => {
                if self.state.made_from.is_some() {
                    *self.value = None;
                }
                self.state.made_from = Some(made_from);
                self.state.changes = BucketChanges::default();
            }
            Some(_) => {}
        }
    }

    /// What of the tile's bucket changed since its value last copied it,
    /// which the layer is about to copy again.
    pub(crate) fn take_changes(&mut self) -> BucketChanges {
        std::mem::take(&mut self.state.changes)
    }
}

/// A mesh a draw draws, with what gives it the textures of its tile's
/// atlases where it samples them.
pub(crate) struct MeshMut<'a> {
    pub(crate) mesh: &'a mut Mesh,
    pub(crate) atlases: Option<&'a AtlasBinding>,
}

impl<'a> From<&'a mut Mesh> for MeshMut<'a> {
    /// A mesh that samples no atlases.
    fn from(mesh: &'a mut Mesh) -> Self {
        MeshMut { mesh, atlases: None }
    }
}

/// What a layer keeps of a tile, whose meshes a frame's draws name by a
/// part of the layer's own type.
pub(crate) trait TileObjects {
    /// Which of the tile's meshes a frame's draw draws.
    type Part: Copy;

    /// The tile's mesh `part`.
    fn objects(&mut self, part: Self::Part) -> Option<MeshMut<'_>>;
}

/// A tile's one mesh.
impl TileObjects for Mesh {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<MeshMut<'_>> {
        Some(self.into())
    }
}

/// The mesh of `draw`, `part` of its tile's, as [`LayerDraws::upload`] and
/// [`LayerDraws::render`] find it.
pub(crate) fn tile_objects<'a, T: TileObjects>(
    tiles: &'a mut TileMap<T>,
    draw: &FrameDraw,
    part: T::Part,
) -> Option<MeshMut<'a>> {
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

/// One draw of a frame: the state it draws its mesh with. Its uniform
/// values are the entries at its index in the storage buffers of its
/// [`FrameDraws`].
#[derive(Clone, Copy)]
pub(crate) struct FrameDraw {
    /// The tile whose draw it is, or `None` for a draw of the layer's.
    pub(crate) tile: Option<OverscaledTileID>,
    /// The id that orders the layer's draws of one priority: that of the
    /// draw's mesh ([`Mesh::id`]), which orders them by when the meshes were
    /// made, or one that the layer's draws share ([`LayerDraws::id`]), which
    /// keeps them in the order the layer pushed them.
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
    /// A draw ordered by `id`, of `tile` or of the layer for `None`,
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

    /// The order of the layer's draws: by priority, then by their id.
    #[must_use]
    pub(crate) fn order(&self) -> (DrawPriority, SimpleIdentity) {
        (self.priority, self.id)
    }
}

/// Where a program's blocks of each draw are in the bytes a draw was pushed
/// with, by the ids of their buffers: each block with its offset.
struct ProgramBlocks {
    program: &'static Program,
    at: [Option<(usize, &'static Block)>; BUFFER_IDS],
    /// Whether one of the draws whose buffers a write writes draws with it.
    drawn: bool,
}

impl ProgramBlocks {
    fn of(program: &'static Program) -> ProgramBlocks {
        let mut at = [None; BUFFER_IDS];
        let mut offset = 0;
        for buffer in program.buffers {
            if let BufferKind::Storage { block, .. } | BufferKind::DrawUniform(block) = buffer.kind {
                at[buffer.id] = Some((offset, block));
                offset += block.size;
            }
        }
        ProgramBlocks { program, at, drawn: false }
    }
}

/// A draw as it was pushed: the index of its program's blocks, its
/// projection, and where its blocks start in the pushes' bytes.
struct Pushed {
    program: usize,
    projection: ProjectionUBO,
    blocks: usize,
}

/// A frame's draws as a layer pushed them, by the index it pushed them at,
/// with the bytes of their blocks and of the blocks they share, which the
/// engine writes to the buffers of the frame's draws.
#[derive(Default)]
struct Pushes {
    pushed: Vec<Pushed>,
    /// The bytes of the pushed draws' blocks.
    bytes: Vec<u8>,
    /// The blocks of each draw of the pushed draws' programs.
    programs: Vec<ProgramBlocks>,
    /// The blocks the draws share, each its layout and where its bytes are
    /// in `shared_bytes`.
    shared: Vec<(&'static Block, Range<usize>)>,
    shared_bytes: Vec<u8>,
    /// Where a write of one of the draws' buffers gathers its bytes.
    scratch: Vec<u8>,
}

/// Whether `a` and `b` are the layout of one block.
fn same_block(a: &Block, b: &Block) -> bool {
    std::ptr::eq(a, b) || a.name == b.name
}

impl Pushes {
    fn clear(&mut self) {
        self.pushed.clear();
        self.bytes.clear();
        self.shared.clear();
        self.shared_bytes.clear();
    }

    /// Adds a draw of `program` with `projection`, whose blocks, of the
    /// layouts `layouts`, are the pushes' bytes from `start`.
    fn push(
        &mut self,
        program: &'static Program,
        projection: Option<&ProjectionUBO>,
        start: usize,
        layouts: &'static [&'static Block],
    ) {
        debug_assert!(
            program
                .buffers
                .iter()
                .filter_map(|buffer| match buffer.kind {
                    BufferKind::Storage { block, .. } | BufferKind::DrawUniform(block) => Some(block),
                    _ => None,
                })
                .map(|block| block.name)
                .eq(layouts.iter().map(|block| block.name)),
            "a draw of {} pushed with blocks other than its program's of each draw",
            program.name
        );
        debug_assert!(program.projection != Projection::Own, "a draw of {} with a projection of its own", program.name);
        let index = self.programs.iter().position(|of| std::ptr::eq(of.program, program)).unwrap_or_else(|| {
            self.programs.push(ProgramBlocks::of(program));
            self.programs.len() - 1
        });
        self.pushed.push(Pushed { program: index, projection: projection.copied().unwrap_or_default(), blocks: start });
    }

    /// Makes `bytes` the value of the shared block of the layout `layout`.
    fn block(&mut self, layout: &'static Block, bytes: &[u8]) {
        if let Some((_, range)) = self.shared.iter().find(|(given, _)| same_block(given, layout)) {
            self.shared_bytes[range.clone()].copy_from_slice(bytes);
        } else {
            let start = self.shared_bytes.len();
            self.shared_bytes.extend_from_slice(bytes);
            self.shared.push((layout, start..self.shared_bytes.len()));
        }
    }

    /// Writes to `buffers` the buffers that `draws`, the pushed draws in
    /// drawing order, each with the index it was pushed at, read: at the id
    /// of each of their programs' uniform blocks, the block of its layout
    /// that they share; at the id of each storage buffer of an element per
    /// draw, each draw's block, or zeros for a draw whose program reads
    /// none there; at the id of a uniform block of a draw's own, which one
    /// draw reads, its block; the draws' projections, or a projection of
    /// zeros where none reads one, which every program declares; and the
    /// frame's paint parameters, `paint_params`, where it has its own.
    fn write(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        draws: &[(FrameDraw, usize)],
        paint_params: Option<&GlobalPaintParamsUBO>,
        buffers: &mut DrawBuffers,
    ) {
        let Pushes { pushed, bytes, programs, shared, shared_bytes, scratch } = self;
        for of in programs.iter_mut() {
            of.drawn = false;
        }
        for &(_, index) in draws {
            programs[pushed[index].program].drawn = true;
        }
        let mut written = [false; BUFFER_IDS];
        let (mut per_draw_projection, mut reads_paint_params) = (false, false);
        for of in programs.iter().filter(|of| of.drawn) {
            per_draw_projection |= of.program.projection == Projection::PerDraw;
            reads_paint_params |= of.program.paint_params.is_some();
        }
        // The shared blocks first, then the draws': a backend joins writes
        // to buffers next to each other into one copy, and the shared
        // blocks' buffers, whose size doesn't change, stay where the first
        // frame made them, before the draws' buffers.
        let shared_first = programs.iter().filter(|of| of.drawn).flat_map(|of| {
            let shared = of.program.buffers.iter().filter(|buffer| matches!(buffer.kind, BufferKind::Uniform(_)));
            shared.map(move |buffer| (of, buffer))
        });
        let per_draw = programs.iter().filter(|of| of.drawn).flat_map(|of| {
            let per_draw = of.program.buffers.iter().filter(|buffer| !matches!(buffer.kind, BufferKind::Uniform(_)));
            per_draw.map(move |buffer| (of, buffer))
        });
        for (of, buffer) in shared_first.chain(per_draw) {
            // A mesh binds its vertices read as storage buffers.
            if matches!(buffer.kind, BufferKind::Vertices(_)) || std::mem::replace(&mut written[buffer.id], true) {
                continue;
            }
            match buffer.kind {
                BufferKind::Uniform(block) => {
                    let Some((_, range)) = shared.iter().find(|(given, _)| same_block(given, block)) else {
                        debug_assert!(false, "draws of {} without their uniform block {}", of.program.name, block.name);
                        continue;
                    };
                    buffers.write_uniform(gpu, buffer.id, &shared_bytes[range.clone()], context.device());
                }
                BufferKind::Storage { stride, .. } => {
                    scratch.clear();
                    for &(_, index) in draws {
                        let Pushed { program, blocks, .. } = pushed[index];
                        let end = scratch.len() + stride;
                        if let Some((offset, block)) = programs[program].at[buffer.id] {
                            let at = blocks + offset;
                            scratch.extend_from_slice(&bytes[at..at + block.size]);
                        }
                        scratch.resize(end, 0);
                    }
                    buffers.write_storage(gpu, buffer.id, scratch, context.device());
                }
                BufferKind::DrawUniform(block) => {
                    let mut own = draws.iter().map(|&(_, index)| &pushed[index]).filter(|draw| {
                        programs[draw.program].at[buffer.id].is_some_and(|(_, of)| same_block(of, block))
                    });
                    let Some(&Pushed { program, blocks, .. }) = own.next() else { continue };
                    debug_assert!(own.next().is_none(), "two draws of a block of {}'s own", block.name);
                    if let Some((offset, _)) = programs[program].at[buffer.id] {
                        let at = blocks + offset;
                        buffers.write_uniform(gpu, buffer.id, &bytes[at..at + block.size], context.device());
                    }
                }
                BufferKind::Vertices(_) => {}
            }
        }
        if per_draw_projection {
            scratch.clear();
            for &(_, index) in draws {
                scratch.extend_from_slice(bytemuck::bytes_of(&pushed[index].projection));
            }
            buffers.write_storage(gpu, PROJECTION_ID, scratch, context.device());
        } else if buffers.get(PROJECTION_ID).is_none() {
            // GL's programs declare the block whether they read it or not,
            // so a draw binds it all the same.
            let none = ProjectionUBO::default();
            buffers.write_storage(gpu, PROJECTION_ID, bytemuck::bytes_of(&none), context.device());
        }
        if let Some(paint_params) = paint_params.filter(|_| reads_paint_params) {
            buffers.write_pod(gpu, PAINT_PARAMS_ID, paint_params, context.device());
        }
    }
}

/// Sorts `draws`, each with the index it was pushed at, in
/// [`FrameDraw::order`], and in the order they were pushed among draws of the
/// same order.
fn sort_draws(draws: &mut [(FrameDraw, usize)]) {
    draws.sort_unstable_by_key(|(draw, index)| (draw.order(), *index));
}

/// Makes the draws of `draws`, without their indexes, those of `frame`, and
/// empties `draws`.
fn take_draws(draws: &mut Vec<(FrameDraw, usize)>, frame: &mut Vec<FrameDraw>) {
    frame.clear();
    frame.extend(draws.drain(..).map(|(draw, _)| draw));
}

/// Draws of a frame, and what they share.
pub(crate) struct FrameDraws {
    /// Tells the draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in [`FrameDraw::order`]. A draw's index is the index of its
    /// entries in the storage buffers, which shaders read as `globalIndex`.
    draws: Vec<FrameDraw>,
    /// The buffers the draws bind, by id, and the bindings of the meshes
    /// they draw.
    pub(crate) buffers: DrawBuffers,
    /// How the draws composite onto the frame in the translucent pass, as one
    /// surface, after they draw into a texture of their own; `None` where
    /// they draw straight onto the frame.
    composite: Option<Composite>,
    /// The tiles whose masks the stencil buffer holds when the draws draw.
    /// Draws that don't clip need none, and neither does a layer without
    /// tiles, whose draws are none.
    stencil_tiles: Vec<UnwrappedTileID>,
}

impl Default for FrameDraws {
    /// No draws, with an id of their own.
    fn default() -> Self {
        FrameDraws {
            id: SimpleIdentity::new(),
            draws: Vec::new(),
            buffers: DrawBuffers::default(),
            composite: None,
            stencil_tiles: Vec::new(),
        }
    }
}

impl FrameDraws {
    /// Drops the draws, and the bindings of the meshes that dropped.
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
        self.buffers.sweep();
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
    pub(crate) fn begin(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<DrawPass<'_>> {
        if self.draws.is_empty() || parameters.pass_target().is_none() {
            return None;
        }
        let composited = (self.composite.is_some() && parameters.pass == RenderPass::Translucent)
            .then(|| parameters.begin_composited(gpu, self.id))
            .flatten();
        let drawing = composited.as_ref().is_none_or(Composited::draws);
        let draws = if drawing { &self.draws[..] } else { &[] };
        // A draw for both the opaque and the translucent pass draws in one:
        // the opaque where its layer draws there, and the translucent
        // otherwise. Drawn in both, it would cover itself.
        let both = RenderPass::Opaque | RenderPass::Translucent;
        let drawn_in =
            if parameters.opaque_pass_enabled_for_layer() { RenderPass::Opaque } else { RenderPass::Translucent };
        let skip = if parameters.pass & both != RenderPass::None && parameters.pass != drawn_in {
            both
        } else {
            RenderPass::None
        };
        let stencil_tiles = &self.stencil_tiles;
        let mut pass = DrawPass {
            buffers: &mut self.buffers,
            draws,
            pass: parameters.pass,
            skip,
            volume_stencil: None,
            composited,
        };
        let (volume, volume_clips) =
            pass.draws().fold((false, false), |(volume, clips), PassDraw { draw, .. }| match draw.depth {
                DrawDepth::Volume { .. } => (true, clips || draw.clip),
                _ => (volume, clips),
            });
        if volume {
            pass.volume_stencil = volume_clips.then(|| parameters.stencil_mode_for_3d(gpu));
        } else if drawing && !stencil_tiles.is_empty() {
            parameters.render_tile_clipping_masks(gpu, stencil_tiles);
        }
        Some(pass)
    }

    /// Uploads `mesh`, which all the draws draw, if there are any draws.
    pub(crate) fn upload_one(&self, gpu: &mut dyn Gpu, mesh: &mut Mesh, context: &mut Context) {
        if !self.draws.is_empty() {
            mesh.upload(gpu, context);
        }
    }

    /// Draws the pass's draws, all of which draw `mesh`, into the open pass
    /// of `gpu`, the context's. Returns whether none failed.
    pub(crate) fn render_one(
        &mut self,
        mesh: &mut Mesh,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
    ) -> bool {
        let Some(mut pass) = self.begin(gpu, parameters) else {
            return true;
        };
        let mut drawn = true;
        for draw in pass.draws() {
            drawn &= pass.draw(gpu, parameters, mesh.into(), draw);
        }
        pass.end(gpu, parameters);
        drawn
    }
}

/// The draws of a pass of a [`FrameDraws`], from [`FrameDraws::begin`] to
/// [`DrawPass::end`], which every pass that begins must reach.
#[must_use = "a pass's draws end with `DrawPass::end`"]
pub(crate) struct DrawPass<'a> {
    /// The buffers the draws bind, with the bindings made of them.
    buffers: &'a mut DrawBuffers,
    /// The frame's draws, none where their layer composite's pass didn't
    /// begin.
    draws: &'a [FrameDraw],
    pass: RenderPass,
    /// The passes of the draws that draw in another pass: both the opaque
    /// and the translucent, or none.
    skip: RenderPass,
    /// The stencil of the 3D draws that clip.
    volume_stencil: Option<StencilMode>,
    composited: Option<Composited>,
}

/// A draw of a [`DrawPass`], as [`DrawPass::draws`] yields it.
#[derive(Clone, Copy)]
pub(crate) struct PassDraw<'a> {
    /// The draw's index among the frame's draws.
    index: usize,
    draw: &'a FrameDraw,
}

impl<'a> PassDraw<'a> {
    #[must_use]
    pub(crate) fn draw(self) -> &'a FrameDraw {
        self.draw
    }
}

impl<'a> DrawPass<'a> {
    /// The draws of the pass, in drawing order.
    pub(crate) fn draws(&self) -> impl Iterator<Item = PassDraw<'a>> + use<'a> {
        let (pass, skip) = (self.pass, self.skip);
        let draws = self.draws.iter().enumerate();
        draws
            .filter(move |(_, draw)| draw.pass & pass != RenderPass::None)
            .filter(move |(_, draw)| skip == RenderPass::None || draw.pass & skip != skip)
            .map(|(index, draw)| PassDraw { index, draw })
    }

    /// Draws `mesh`, the mesh of `draw`, one of the pass's. Returns whether
    /// it didn't fail.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        mesh: MeshMut<'_>,
        draw: PassDraw<'_>,
    ) -> bool {
        let MeshMut { mesh, atlases } = mesh;
        let PassDraw { index, draw } = draw;
        if let Some(atlases) = atlases {
            atlases.bind(mesh, parameters);
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
        mesh.draw(gpu, parameters, self.buffers, depth, stencil, index as u32)
    }

    /// Ends the pass's draws: composites what they drew where they drew
    /// into the layer composite's texture.
    pub(crate) fn end(self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(composited) = self.composited {
            parameters.end_composited(gpu, composited);
        }
    }
}

/// A layer's draws of a frame, each with the part of the layer's meshes it
/// draws, of the layer's type `P`.
pub(crate) struct LayerDraws<P = ()> {
    frame: FrameDraws,
    /// Each draw's part, at the draw's index.
    parts: Vec<P>,
    /// The draws that the layer pushes for the next frame, each with the
    /// index it was pushed at, with their parts by that index and their
    /// blocks, until [`finish`](Self::finish) makes them the frame's. The
    /// draws sort without their parts, so that the sort is one function for
    /// all the layers' draws.
    pushed: Vec<(FrameDraw, usize)>,
    pushed_parts: Vec<P>,
    pushes: Pushes,
}

impl<P> Default for LayerDraws<P> {
    fn default() -> Self {
        LayerDraws {
            frame: FrameDraws::default(),
            parts: Vec::new(),
            pushed: Vec::new(),
            pushed_parts: Vec::new(),
            pushes: Pushes::default(),
        }
    }
}

impl<P: Copy> LayerDraws<P> {
    /// The id of the draws ([`FrameDraws`]'), which a layer whose draws all
    /// carry it as theirs ([`FrameDraw::id`]) draws in the order it pushed
    /// them.
    #[must_use]
    pub(crate) fn id(&self) -> SimpleIdentity {
        self.frame.id
    }

    /// The draws, with what they share.
    #[must_use]
    pub(crate) fn frame(&self) -> &FrameDraws {
        &self.frame
    }

    /// The draws, with what they share, to draw them.
    pub(crate) fn frame_mut(&mut self) -> &mut FrameDraws {
        &mut self.frame
    }

    /// See [`FrameDraws::set_composite`].
    pub(crate) fn set_composite(&mut self, composite: Option<Composite>) {
        self.frame.set_composite(composite);
    }

    /// See [`FrameDraws::set_stencil_tiles`].
    pub(crate) fn set_stencil_tiles(&mut self, tiles: impl IntoIterator<Item = UnwrappedTileID>) {
        self.frame.set_stencil_tiles(tiles);
    }

    /// Drops the draws, those pushed too, and the bindings of the meshes
    /// that dropped.
    pub(crate) fn clear(&mut self) {
        self.frame.clear();
        self.parts.clear();
        self.clear_pushed();
    }

    fn clear_pushed(&mut self) {
        self.pushed.clear();
        self.pushed_parts.clear();
        self.pushes.clear();
    }

    /// Adds `draw` of `mesh`, the layer's mesh `part`, to the next frame's,
    /// with `blocks`, the values of the draw's own of the blocks of each
    /// draw that the mesh's program reads, in its order. The draw's
    /// projection, where its program reads one, is zeros.
    pub(crate) fn push<B: DrawBlocks>(&mut self, draw: FrameDraw, part: P, mesh: &Mesh, blocks: B) {
        self.add(draw, part, mesh, None, blocks);
    }

    /// Adds `draw` with `projection`, as [`push`](Self::push) does: the
    /// draw reads it where its program reads a projection of each draw.
    pub(crate) fn push_at<B: DrawBlocks>(
        &mut self,
        draw: FrameDraw,
        part: P,
        mesh: &Mesh,
        projection: &ProjectionUBO,
        blocks: B,
    ) {
        self.add(draw, part, mesh, Some(projection), blocks);
    }

    fn add<B: DrawBlocks>(
        &mut self,
        draw: FrameDraw,
        part: P,
        mesh: &Mesh,
        projection: Option<&ProjectionUBO>,
        blocks: B,
    ) {
        let start = self.pushes.bytes.len();
        blocks.each_bytes(|block| self.pushes.bytes.extend_from_slice(block));
        self.add_at(draw, part, mesh.program(), projection, start, B::BLOCKS);
    }

    /// [`add`](Self::add), with the blocks in the pushes' bytes from `start`
    /// of whatever type they are.
    fn add_at(
        &mut self,
        draw: FrameDraw,
        part: P,
        program: &'static Program,
        projection: Option<&ProjectionUBO>,
        start: usize,
        layouts: &'static [&'static Block],
    ) {
        self.pushed.push((draw, self.pushed_parts.len()));
        self.pushed_parts.push(part);
        self.pushes.push(program, projection, start, layouts);
    }

    /// Makes `block` the value that the next frame's draws read of their
    /// programs' uniform block of its layout, the last given. A draw whose
    /// program has none ignores it.
    pub(crate) fn block<T: UniformBlock>(&mut self, block: &T) -> &mut Self {
        self.pushes.block(T::BLOCK, bytemuck::bytes_of(block));
        self
    }

    /// Makes the pushed draws the frame's, in [`FrameDraw::order`] and in
    /// the order they were pushed among draws of the same order. Where there
    /// are any, it first writes the buffers they read, with the paint
    /// parameters of `parameters`' frame where it has its own (a drape's).
    pub(crate) fn finish(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let paint_params = parameters.own_paint_params();
        self.finish_with(gpu, parameters.context, paint_params.as_ref());
    }

    /// [`finish`](Self::finish), for draws that read `paint_params` of
    /// their own, or the context's global ones for `None`.
    pub(crate) fn finish_with(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        paint_params: Option<&GlobalPaintParamsUBO>,
    ) {
        sort_draws(&mut self.pushed);
        if !self.pushed.is_empty() {
            self.pushes.write(gpu, context, &self.pushed, paint_params, &mut self.frame.buffers);
        }
        self.parts.clear();
        self.parts.extend(self.pushed.iter().map(|&(_, index)| self.pushed_parts[index]));
        take_draws(&mut self.pushed, &mut self.frame.draws);
        self.frame.buffers.sweep();
        self.clear_pushed();
    }

    /// Uploads the meshes of the draws, in drawing order, which `meshes`
    /// finds in `caches`.
    pub(crate) fn upload<C: ?Sized>(
        &self,
        gpu: &mut dyn Gpu,
        caches: &mut C,
        meshes: impl for<'c> Fn(&'c mut C, &FrameDraw, P) -> Option<MeshMut<'c>>,
        context: &mut Context,
    ) {
        for (draw, &part) in self.frame.draws.iter().zip(&self.parts) {
            if let Some(mesh) = meshes(caches, draw, part) {
                mesh.mesh.upload(gpu, context);
            }
        }
    }

    /// Draws the pass's draws, whose meshes `meshes` finds in `caches`, into
    /// the open pass of `gpu`, the context's.
    pub(crate) fn render<C: ?Sized>(
        &mut self,
        caches: &mut C,
        meshes: impl for<'c> Fn(&'c mut C, &FrameDraw, P) -> Option<MeshMut<'c>>,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
    ) {
        let LayerDraws { frame, parts, .. } = self;
        let Some(mut pass) = frame.begin(gpu, parameters) else {
            return;
        };
        for draw in pass.draws() {
            match meshes(caches, draw.draw(), parts[draw.index]) {
                Some(mesh) => {
                    pass.draw(gpu, parameters, mesh, draw);
                }
                None => debug_assert!(false, "a draw of tile {:?} has no mesh", draw.draw().tile),
            }
        }
        pass.end(gpu, parameters);
    }
}

/// One mesh and a frame's draw of it, with the buffers it binds.
pub(crate) struct SingleDraw {
    pub(crate) mesh: Mesh,
    /// How the draw draws.
    draw: FrameDraw,
    draws: LayerDraws,
}

impl SingleDraw {
    /// A draw of `mesh`, as `draw` draws, which draws once its blocks are
    /// written ([`write`](Self::write)).
    #[must_use]
    pub(crate) fn new(mesh: Mesh, draw: FrameDraw) -> SingleDraw {
        SingleDraw { mesh, draw, draws: LayerDraws::default() }
    }

    /// Makes `block` the value the draw reads of its program's uniform
    /// block of its layout, as [`LayerDraws::block`] does, from the next
    /// [`write`](Self::write).
    pub(crate) fn block<T: UniformBlock>(&mut self, block: &T) -> &mut Self {
        self.draws.block(block);
        self
    }

    /// Writes the buffers the draw reads, with `blocks`, its values of its
    /// program's blocks of each draw, as [`LayerDraws::finish`] does.
    pub(crate) fn write<B: DrawBlocks>(&mut self, gpu: &mut dyn Gpu, context: &mut Context, blocks: B) {
        let SingleDraw { mesh, draw, draws } = self;
        draws.push(*draw, (), mesh, blocks);
        draws.finish_with(gpu, context, None);
    }

    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.draws.frame.upload_one(gpu, &mut self.mesh, context);
    }

    /// Draws the draw, if it draws in the pass, into the open pass of `gpu`,
    /// the context's. Returns whether it didn't fail.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        self.draws.frame.render_one(&mut self.mesh, gpu, parameters)
    }
}

/// What the frame draws of a layer in one place in the layers' order and
/// depth.
#[derive(Clone, Copy)]
pub(crate) enum DrawGroup<'a> {
    /// Draws of the layer's meshes.
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

    /// Whether the group draws anything: a custom layer's host may.
    #[must_use]
    pub(crate) fn draws(self) -> bool {
        match self {
            DrawGroup::Draws(frame) => !frame.draws.is_empty(),
            DrawGroup::Custom => true,
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

/// What a frame's draw groups, or a drape's, need of the passes: their
/// number, which sets the passes' depth range; the masks of the tiles they
/// clip to and of the tiles that clear the stencil buffer; and the groups
/// drawn as a whole, each into the layer composite's texture and then onto
/// the frame.
#[derive(Default)]
pub(crate) struct DrawGroups {
    pub(crate) count: usize,
    pub(crate) clip_tiles: Vec<UnwrappedTileID>,
    /// The tiles whose masks clear the stencil buffer, also in
    /// `clip_tiles` ([`PaintParameters::stencil_clear_tiles`]).
    pub(crate) stencil_clear: StencilClear,
    pub(crate) composited: Vec<(SimpleIdentity, Composite)>,
}

impl DrawGroups {
    /// The draw groups that `visit_groups` visits, in drawing order, of
    /// passes whose stencil the tiles that `stencil_clear_tiles` gives for
    /// the tiles they clip to clear
    /// ([`PaintParameters::stencil_clear_tiles`]). Draw groups of one source
    /// clip to the same tiles, which are added once.
    pub(crate) fn of<'a>(
        visit_groups: impl FnOnce(&mut dyn FnMut(DrawGroup<'a>)),
        stencil_clear_tiles: impl FnOnce(&[UnwrappedTileID]) -> StencilClear,
    ) -> DrawGroups {
        let mut groups = DrawGroups::default();
        let mut last: Option<&[UnwrappedTileID]> = None;
        visit_groups(&mut |group| {
            groups.count += 1;
            if let Some(tiles) = group.stencil_tiles()
                && last.is_none_or(|last| last != tiles)
            {
                groups.clip_tiles.extend(tiles.iter().copied());
                last = Some(tiles);
            }
            groups.composited.extend(group.composite());
        });
        groups.stencil_clear = stencil_clear_tiles(&groups.clip_tiles);
        groups.clip_tiles.extend_from_slice(&groups.stencil_clear);
        if !groups.clip_tiles.is_sorted() {
            groups.clip_tiles.sort_unstable();
        }
        groups.clip_tiles.dedup();
        groups
    }

    /// The depth range of the passes' 3D draws, from 0, below the depths of
    /// the groups' 2D draws.
    #[must_use]
    pub(crate) fn depth_range_size(&self) -> f32 {
        1.0 - ((self.count + 2) * PaintParameters::NUM_SUBLAYERS as usize) as f32 * PaintParameters::DEPTH_EPSILON
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
/// the value of the tile it draws, a `T`.
pub(crate) struct TileDraws<T: TileObjects> {
    /// The frame's draws, from the layer's first update with something to
    /// draw.
    frame: Option<LayerDraws<T::Part>>,
}

impl<T: TileObjects> Default for TileDraws<T> {
    fn default() -> Self {
        TileDraws { frame: None }
    }
}

impl<T: TileObjects> TileDraws<T> {
    /// The frame's draws, which the layer has from the first call on: their
    /// group then takes a place in the layers' order and depth.
    pub(crate) fn frame_mut(&mut self) -> &mut LayerDraws<T::Part> {
        self.frame.get_or_insert_default()
    }

    /// Makes the pushed draws the frame's, as [`LayerDraws::finish`] does.
    pub(crate) fn finish(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame_mut().finish(gpu, parameters);
    }
}

impl<T: TileObjects> LayerDrawing<T> for TileDraws<T> {
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if let Some(frame) = &self.frame {
            visit(DrawGroup::Draws(&frame.frame));
        }
    }

    fn begin_frame(&mut self) {
        self.frame_mut().clear();
    }

    fn clear_frame(&mut self) {
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn upload(&mut self, tiles: &mut TileMap<T>, gpu: &mut dyn Gpu, context: &mut Context) {
        if let Some(frame) = &self.frame {
            frame.upload(gpu, tiles, tile_objects, context);
        }
    }

    fn render(
        &mut self,
        tiles: &mut TileMap<T>,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
    ) {
        if let Some(frame) = &mut self.frame {
            frame.render(tiles, tile_objects, gpu, parameters);
            order.step(parameters);
        }
    }
}

/// What a layer type keeps of its drawing besides its tile values, which
/// the engine keeps ([`TileMap`]) of the tile type `T`: its draw groups of
/// the frame, whose draws it uploads and draws in loops of its own, finding
/// the meshes of the tiles' draws in their values, and any meshes of draws
/// of its own.
pub(crate) trait LayerDrawing<T> {
    /// Visits the layer's draw groups of the frame, in drawing order: from
    /// its first update with something to draw, a group of its draws, and
    /// any of draws over them. While the layer is renderable, a group takes
    /// a place in the layers' order and depth, even when it has no draws.
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>));

    /// Drops the frame's draws and the meshes of the drawing's own
    /// draws, as the engine drops the tiles' values.
    fn clear(&mut self) {
        self.clear_frame();
    }

    /// Starts the layer's draws of a frame it has something to draw in, in
    /// the encode, before the layer makes them: empty, keeping the GPU
    /// objects of the drawing's own draws. A drawing whose draw group takes
    /// its place from the layer's first such frame starts the group here.
    fn begin_frame(&mut self);

    /// Drops the frame's draws, keeping the meshes of the drawing's
    /// own draws, for a frame the layer draws nothing in.
    fn clear_frame(&mut self);

    /// Uploads the meshes of the draw groups' draws, those of the
    /// tiles' draws in `tiles`.
    fn upload(&mut self, tiles: &mut TileMap<T>, gpu: &mut dyn Gpu, context: &mut Context);

    /// Draws the draw groups' draws of the pass, those of the tiles' draws
    /// with their objects in `tiles`, into the open pass of `gpu`, the
    /// context's, in `order`, each at its depth.
    fn render(
        &mut self,
        tiles: &mut TileMap<T>,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
    );
}

/// What a drape keeps of a layer, whatever the layer's type, for a
/// [`DrapeDrawing`]: the layer's tile values in the drape and its drawing
/// there.
trait AnyDrawing {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn mark_changed(&mut self, changed: &ChangedBuckets);
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>));
    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context);
    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder);
}

impl<T: 'static, D: LayerDrawing<T> + 'static> AnyDrawing for (TileMap<T>, D) {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.0.mark_changed(changed);
    }

    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        self.1.visit_groups(visit);
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        let (tiles, drawing) = self;
        drawing.upload(tiles, gpu, context);
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        let (tiles, drawing) = self;
        drawing.render(tiles, gpu, parameters, order);
    }
}

/// What a drape keeps of a layer of its stack: the layer's values of the
/// tiles it draws in the drape, which the engine keeps as it keeps those of
/// the frame, and the layer's drawing of its type there. Only the layer
/// updates them ([`RenderLayer::update_drape_draws`]), and the drape uploads
/// and draws them.
///
/// [`RenderLayer::update_drape_draws`]: crate::layer::render_layer::RenderLayer::update_drape_draws
pub(crate) struct DrapeDrawing(Box<dyn AnyDrawing>);

impl DrapeDrawing {
    /// No tile values and an empty `D`.
    pub(crate) fn new<T: 'static, D: LayerDrawing<T> + Default + 'static>() -> DrapeDrawing {
        DrapeDrawing(Box::new((TileMap::<T>::default(), D::default())))
    }

    /// The tile values and the drawing, if they are a `T`'s and a `D`.
    pub(crate) fn downcast_mut<T: 'static, D: 'static>(&mut self) -> Option<&mut (TileMap<T>, D)> {
        self.0.as_any_mut().downcast_mut()
    }

    /// See [`RenderLayer::mark_changed`](crate::layer::render_layer::RenderLayer::mark_changed).
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        self.0.mark_changed(changed);
    }

    /// See [`LayerDrawing::visit_groups`].
    pub(crate) fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        self.0.visit_groups(visit);
    }

    /// See [`LayerDrawing::upload`].
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.0.upload(gpu, context);
    }

    /// See [`LayerDrawing::render`].
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        self.0.render(gpu, parameters, order);
    }
}

#[cfg(test)]
mod tests {
    use super::{BucketChanges, ChangedBuckets, TileMap};
    use crate::draw::identity::SimpleIdentity;
    use crate::tile::OverscaledTileID;

    /// Sets what `tiles` keeps of `tile_id` to `value`, if the tile is one
    /// of the map's; returns whether it is.
    fn set<T>(tiles: &mut TileMap<T>, tile_id: &OverscaledTileID, value: T) -> bool {
        tiles.entry(tile_id).map(|entry| *entry.value = Some(value)).is_some()
    }

    /// The bucket that the value of `tile_id` in `tiles` was made from.
    fn bucket_of<T>(tiles: &mut TileMap<T>, tile_id: &OverscaledTileID) -> Option<SimpleIdentity> {
        Some(tiles.entry(tile_id)?.made_from()?.0)
    }

    /// Records that `tile_id` shows `bucket` in `tiles`; returns whether the
    /// tile is one of the map's.
    fn show<T>(tiles: &mut TileMap<T>, tile_id: &OverscaledTileID, bucket: SimpleIdentity) -> bool {
        tiles.entry(tile_id).map(|mut entry| entry.show(Some((bucket, [0, 0])))).is_some()
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
        assert!(show(&mut tiles, &tile, bucket));
        assert!(show(&mut tiles, &tile, bucket));
        assert!(show(&mut tiles, &other, other_bucket));

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

    /// A tile that shows another bucket, or bucket of another generation,
    /// drops its value and the changes of the bucket before; one that shows
    /// nothing drops its value.
    #[test]
    fn another_bucket_drops_the_value() {
        let tile = OverscaledTileID::with_zxy(1, 0, 1, 1, 0);
        let (bucket, other_bucket) = (SimpleIdentity::new(), SimpleIdentity::new());
        let mut tiles = TileMap::<u32>::default();
        tiles.set_tiles([tile]);
        assert!(set(&mut tiles, &tile, 1));
        assert!(show(&mut tiles, &tile, bucket));
        assert_eq!(Some(&1), tiles.get(&tile));
        let mut changed = ChangedBuckets::default();
        changed.mark(bucket, BucketChanges { paint: true, ..BucketChanges::default() });
        tiles.mark_changed(&changed);
        assert!(show(&mut tiles, &tile, other_bucket));
        assert!(tiles.get(&tile).is_none());
        assert_eq!(Some(BucketChanges::default()), tiles.entry(&tile).map(|mut entry| entry.take_changes()));

        assert!(set(&mut tiles, &tile, 2));
        tiles.entry(&tile).unwrap().show(Some((other_bucket, [1, 0])));
        assert!(tiles.get(&tile).is_none());
        assert!(set(&mut tiles, &tile, 3));
        tiles.entry(&tile).unwrap().show(None);
        assert!(tiles.get(&tile).is_none());
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
        assert!(show(&mut tiles, &ids[2], bucket));
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

    /// Each tile keeps its value and bucket from any set of tiles to any
    /// other.
    #[test]
    fn values_stay_through_every_change() {
        let ids: Vec<OverscaledTileID> = (0..5).map(|x| OverscaledTileID::with_zxy(3, 0, 3, x, 2)).collect();
        let buckets: Vec<SimpleIdentity> = ids.iter().map(|_| SimpleIdentity::new()).collect();
        let subsets = |mask: u32| ids.iter().enumerate().filter(move |(i, _)| mask & (1 << i) != 0).map(|(_, id)| *id);
        let mut tiles = TileMap::<usize>::default();
        for from in 0..32 {
            for to in 0..32 {
                tiles.clear_tiles();
                tiles.set_tiles(subsets(from));
                for (i, id) in ids.iter().enumerate() {
                    show(&mut tiles, id, buckets[i]);
                    set(&mut tiles, id, i);
                }
                tiles.set_tiles(subsets(to).rev());
                let values: Vec<Option<usize>> = ids.iter().map(|id| tiles.get(id).copied()).collect();
                let expected: Vec<Option<usize>> =
                    (0..ids.len()).map(|i| (from & to & (1 << i) != 0).then_some(i)).collect();
                assert_eq!(expected, values, "from {from:05b} to {to:05b}");
                let kept_buckets: Vec<Option<SimpleIdentity>> =
                    ids.iter().map(|id| bucket_of(&mut tiles, id)).collect();
                let expected: Vec<Option<SimpleIdentity>> =
                    (0..ids.len()).map(|i| (from & to & (1 << i) != 0).then_some(buckets[i])).collect();
                assert_eq!(expected, kept_buckets, "from {from:05b} to {to:05b}");
            }
        }
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
        tiles.set_tiles([high, low, tile]);
        assert!(set(&mut tiles, &high, 3));
        assert!(set(&mut tiles, &low, 4));
        assert!(set(&mut tiles, &tile, 5));
        tiles.shift_wraps(1);
        assert_eq!(Some(&3), tiles.get(&tile.unwrap_to(i16::MIN)));
        assert_eq!(Some(&4), tiles.get(&other.unwrap_to(i16::MAX)));
        assert_eq!(Some(&5), tiles.get(&tile.unwrap_to(1)));

        tiles.clear();
        assert!(tiles.has_tile(&other.unwrap_to(i16::MAX)));
        assert!(tiles.get(&other.unwrap_to(i16::MAX)).is_none());
    }
}
