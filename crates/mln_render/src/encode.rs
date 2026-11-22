//! The encode of a frame's draws.
//!
//! A layer pushes its draws of a frame into its [`Draws`]: each a [`Draw`]
//! of one of its meshes, with the values of the mesh's program's blocks of
//! each draw ([`DrawBlocks`]) and, for a draw of a tile, the tile's
//! [`TileProjection`]. The engine then sorts each [`Group`] by the draws'
//! order, the age of their meshes and when they were pushed, writes each of
//! its programs' buffers once for all of its draws, places the group in the
//! layers' order and depth, and draws it in the frame's passes: with the
//! masks of the layer's tiles in the stencil where its draws clip, into a
//! texture of its own where it composites, and with no call back into the
//! layer. [`FrameDraws`] are a group's draws as the engine draws them.

use std::ops::Range;

use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::bindings::FIRST_VERTEX_BUFFER_ID;
use mln_gfx::shaders::block::{Block, DrawBlocks, UniformBlock};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{BufferKind, PROJECTION_BUFFER, Program, Projection};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;

use crate::draw_objects::Mesh;
use crate::gfx::context::Resources;
use crate::gfx::store::MeshKey;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::layer_composite::Composite;
use crate::paint_parameters::{Composited, DepthTest, PaintParameters, Stencil};
use crate::render_pass::RenderPass;
use crate::tile_matrix::{TileProjection, bind_no_projection};

/// How a draw tests and writes depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// No depth test: the draw draws over what drew before it.
    Off,
    /// A test against one of the three depths of the layer's place in the
    /// layers' order, `sub_layer` 0, 1 or 2, 0 the nearest, which the draw
    /// writes where `write`. A draw tests depth this way only where its
    /// layer draws opaque draws in the opaque pass: below the lowest layer
    /// of 3D geometry on a flat map. Elsewhere it draws as [`Off`](Depth::Off)
    /// does.
    Layer {
        /// The depth of the three.
        sub_layer: u8,
        /// Whether the draw writes the depth it tests.
        write: bool,
    },
    /// A test against the depth of 3D geometry, which the 3D draws of all
    /// layers share, which the draw writes where `write`. With `once`, no
    /// pixel draws twice among the `once` draws of the draw's group in a
    /// pass, so that a translucent surface behind another of the same layer
    /// doesn't show through.
    Volume {
        /// Whether the draw writes the depth it tests.
        write: bool,
        /// Whether a pixel draws once among the group's `once` draws.
        once: bool,
    },
}

/// One draw of a mesh: its place among its group's draws and the state it
/// draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// Where the draw is among its group's draws, which may hold draws of
    /// all the layer's tiles: lower orders draw first. Draws of one order
    /// draw in the order their meshes were made, and draws of one mesh in
    /// the order they were pushed.
    pub order: i64,
    /// Whether the draw draws in the opaque pass where its layer does
    /// (front to back, below the lowest layer of 3D geometry on a flat
    /// map), and otherwise in the translucent pass: it draws once, in one of
    /// the two. A draw that isn't draws in the translucent pass. Only a
    /// layer whose evaluation says it may draw there
    /// ([`Evaluation::opaque`](crate::layer::Evaluation::opaque)) pushes
    /// such a draw.
    pub opaque: bool,
    /// How the draw tests and writes depth.
    pub depth: Depth,
    /// Whether the draw draws only within its tile, where the stencil holds
    /// the tile's mask: a draw of [`Depth::Off`] or [`Depth::Layer`] pushed
    /// with the projection of one of its layer's tiles
    /// ([`FrameTile::project`](crate::layer::FrameTile::project)). The
    /// group then draws the masks of all its layer's tiles first.
    pub clip: bool,
}

impl Draw {
    /// A draw of order 0 in the translucent pass that tests, and doesn't
    /// write, the nearest depth of its layer, and doesn't clip.
    pub const DEFAULT: Draw =
        Draw { order: 0, opaque: false, depth: Depth::Layer { sub_layer: 0, write: false }, clip: false };
}

impl Default for Draw {
    fn default() -> Self {
        Draw::DEFAULT
    }
}

/// A draw as it was pushed, before its group sorts it.
struct Pushed {
    draw: Draw,
    /// Its index among the group's draws as they were pushed.
    index: u32,
    mesh: MeshKey,
    /// When its mesh was made.
    seq: u64,
    program: &'static Program,
    /// The tile whose mask the draw clips to.
    tile: Option<OverscaledTileID>,
    /// The projection block of the tile it was pushed with.
    projection: Option<ProjectionUBO>,
    /// Where its blocks start in the group's bytes.
    blocks: usize,
}

/// A layer's draws of a frame in one place in the layers' order and depth,
/// which the engine sorts and draws in each of the frame's passes.
pub struct Group {
    /// The draws, as the engine draws them, once it sorted them.
    pub(crate) frame: FrameDraws,
    /// The draws pushed for the frame.
    pushed: Vec<Pushed>,
    /// The bytes of the pushed draws' blocks.
    bytes: Vec<u8>,
    /// The blocks that all the draws share, each its layout and where its
    /// bytes are in `block_bytes`.
    blocks: Vec<(&'static Block, Range<usize>)>,
    block_bytes: Vec<u8>,
    /// Where a write of one of the draws' buffers gathers its bytes.
    scratch: Vec<u8>,
    /// Whether the group's draws may draw in the opaque pass.
    opaque: bool,
    /// Whether the group can composite with the frame's depth.
    frame_depth: bool,
}

impl Group {
    pub(crate) fn new() -> Group {
        Group {
            frame: FrameDraws::default(),
            pushed: Vec::new(),
            bytes: Vec::new(),
            blocks: Vec::new(),
            block_bytes: Vec::new(),
            scratch: Vec::new(),
            opaque: false,
            frame_depth: false,
        }
    }

    /// Adds a draw of `mesh`, not of a tile, with `blocks`, the values of
    /// the draw's own of its program's storage buffers of an element per
    /// draw. Its program's projection block, if it reads one
    /// ([`Projection::PerDraw`]), is zeros.
    pub fn push<B: DrawBlocks>(&mut self, mesh: &Mesh<B>, draw: Draw, blocks: B) -> &mut Self {
        self.add(mesh, None, draw, blocks)
    }

    /// Adds a draw of `mesh` projected as `at` says, with `blocks`, as
    /// [`push`](Self::push) does: the engine writes `at` as the projection
    /// block of the draw, for a program that reads one
    /// ([`Projection::PerDraw`]), and the draw clips to `at`'s tile where it
    /// clips.
    pub fn push_at<B: DrawBlocks>(&mut self, mesh: &Mesh<B>, at: &TileProjection, draw: Draw, blocks: B) -> &mut Self {
        self.add(mesh, Some(at), draw, blocks)
    }

    /// Makes `block` the value that the group's draws read of their
    /// programs' uniform block of its type, written once a frame. A draw of
    /// a program without one ignores it.
    pub fn block<T: UniformBlock>(&mut self, block: &T) -> &mut Self {
        let start = self.block_bytes.len();
        self.block_bytes.extend_from_slice(bytemuck::bytes_of(block));
        self.blocks.push((T::BLOCK, start..self.block_bytes.len()));
        self
    }

    /// Makes the group's draws draw into a texture of their own in the
    /// translucent pass, which then composites onto the frame as
    /// `composite` says. Returns false, changing nothing, for a composite
    /// with the frame's depth ([`Composite::frame_depth`]) where the frame
    /// can't draw its depth together with that texture.
    pub fn composite(&mut self, composite: Composite) -> bool {
        if composite.frame_depth && !self.frame_depth {
            return false;
        }
        self.frame.composite = Some(composite);
        true
    }

    fn add<B: DrawBlocks>(&mut self, mesh: &Mesh<B>, at: Option<&TileProjection>, draw: Draw, blocks: B) -> &mut Self {
        let program = mesh.program();
        debug_assert!(
            blocks_of_each_draw(program).map(|buffer| buffer.0.name).eq(B::BLOCKS.iter().map(|block| block.name)),
            "a draw of {} with other blocks than its program's of each draw",
            program.name
        );
        debug_assert!(
            !program.buffers.iter().any(|buffer| matches!(buffer.kind, BufferKind::DrawUniform(_))),
            "a draw of {}, whose blocks of each draw are its mesh's own",
            program.name
        );
        debug_assert!(!draw.opaque || self.opaque, "an opaque draw of a layer whose evaluation draws none");
        debug_assert!(
            !draw.clip
                || (matches!(draw.depth, Depth::Off | Depth::Layer { .. }) && at.is_some_and(|at| at.tile.is_some())),
            "a draw of {} that clips without a tile, or in 3D",
            program.name
        );
        let start = self.bytes.len();
        blocks.each_bytes(|bytes| self.bytes.extend_from_slice(bytes));
        debug_assert_eq!(
            self.bytes.len() - start,
            B::BLOCKS.iter().map(|block| block.size).sum::<usize>(),
            "blocks of other sizes than their layouts"
        );
        self.pushed.push(Pushed {
            draw,
            index: self.pushed.len() as u32,
            mesh: mesh.key(),
            seq: mesh.seq(),
            program,
            tile: at.and_then(|at| at.tile),
            projection: at.map(|at| at.block),
            blocks: start,
        });
        self
    }

    /// Starts the group's draws of a frame: none, in no composite, which
    /// may draw in the opaque pass where `opaque`, and composite with the
    /// frame's depth where `frame_depth`.
    pub(crate) fn begin(&mut self, opaque: bool, frame_depth: bool) {
        self.clear();
        self.frame.composite = None;
        self.opaque = opaque;
        self.frame_depth = frame_depth;
    }

    /// Drops the frame's draws.
    pub(crate) fn clear(&mut self) {
        self.frame.draws.clear();
        self.pushed.clear();
        self.bytes.clear();
        self.blocks.clear();
        self.block_bytes.clear();
    }

    /// Makes the pushed draws the frame's, in their order, and, where there
    /// are any, writes the buffers they read: the blocks they share, their
    /// blocks of each draw, at the index of each draw, and their projection
    /// blocks. Draws that clip clip to the masks of `tiles`.
    pub(crate) fn finish(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        tiles: impl IntoIterator<Item = UnwrappedTileID>,
    ) {
        let Group { frame, pushed, bytes, blocks, block_bytes, scratch, .. } = self;
        pushed.sort_unstable_by_key(|pushed| (pushed.draw.order, pushed.seq, pushed.index));
        frame.draws.clear();
        frame.draws.extend(pushed.iter().map(|pushed| FrameDraw {
            tile: pushed.tile,
            mesh: pushed.mesh,
            pass: if pushed.draw.opaque {
                RenderPass::OPAQUE | RenderPass::TRANSLUCENT
            } else {
                RenderPass::TRANSLUCENT
            },
            depth: pushed.draw.depth,
            clip: pushed.draw.clip,
        }));
        frame.stencil_tiles.clear();
        if pushed.iter().any(|pushed| pushed.draw.clip) {
            frame.stencil_tiles.extend(tiles);
        }
        if !pushed.is_empty() {
            write_blocks(gpu, context, &mut frame.uniform_buffers, pushed, bytes, blocks, block_bytes, scratch);
        }
        pushed.clear();
        bytes.clear();
        blocks.clear();
        block_bytes.clear();
    }

    /// Uploads the meshes of the frame's draws.
    pub(crate) fn upload(&self, gpu: &mut dyn Gpu, context: &mut Resources) {
        self.frame.upload(gpu, context);
    }

    /// Draws the pass's draws of the frame into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render(gpu, parameters);
    }
}

/// The blocks of each draw of `program`, each with its buffer's id and the
/// bytes between two draws' elements.
fn blocks_of_each_draw(program: &Program) -> impl Iterator<Item = (&'static Block, usize, usize)> + '_ {
    program.buffers.iter().filter_map(|buffer| match buffer.kind {
        BufferKind::Storage { block, stride } => Some((block, buffer.id, stride)),
        _ => None,
    })
}

/// Writes to `buffers` the buffers that `pushed`, a group's draws in their
/// order, read: each of `blocks`, whose bytes are in `block_bytes`, at the
/// id of the uniform block of its layout in the draws' programs, the
/// draws' blocks in `bytes` at the ids of their programs' storage buffers,
/// an element per draw, and their projection blocks, or a projection block
/// of zeros where no draw's program reads one of each draw.
#[expect(clippy::too_many_arguments, reason = "the GPU, the buffers and the parts of a group they're written from")]
fn write_blocks(
    gpu: &mut dyn Gpu,
    context: &mut Resources,
    buffers: &mut UniformBufferArray,
    pushed: &[Pushed],
    bytes: &[u8],
    blocks: &[(&'static Block, Range<usize>)],
    block_bytes: &[u8],
    scratch: &mut Vec<u8>,
) {
    let uniform_id = |program: &Program, block: &Block| {
        program.buffers.iter().find_map(|buffer| match buffer.kind {
            BufferKind::Uniform(of) if of.name == block.name => Some(buffer.id),
            _ => None,
        })
    };
    for (block, range) in blocks {
        if let Some(id) = pushed.iter().find_map(|pushed| uniform_id(pushed.program, block)) {
            buffers.write_uniform(gpu, id, &block_bytes[range.clone()], context.device());
        }
    }
    debug_assert!(
        pushed.iter().all(|pushed| pushed.program.buffers.iter().all(|buffer| match buffer.kind {
            BufferKind::Uniform(of) => blocks.iter().any(|(block, _)| block.name == of.name),
            _ => true,
        })),
        "a group whose draws read a uniform block that it has no value of"
    );

    let mut ids = [false; FIRST_VERTEX_BUFFER_ID];
    for pushed in pushed {
        for (_, id, _) in blocks_of_each_draw(pushed.program) {
            ids[id] = true;
        }
    }
    for id in (0..FIRST_VERTEX_BUFFER_ID).filter(|&id| ids[id]) {
        let stride = pushed
            .iter()
            .find_map(|pushed| blocks_of_each_draw(pushed.program).find(|&(_, at, _)| at == id))
            .map_or(0, |(_, _, stride)| stride);
        scratch.clear();
        for pushed in pushed {
            let mut start = pushed.blocks;
            let mut element = None;
            for (block, at, of) in blocks_of_each_draw(pushed.program) {
                if at == id {
                    debug_assert_eq!(of, stride, "programs of one group whose elements at {id} differ in size");
                    element = Some(&bytes[start..start + block.size]);
                }
                start += block.size;
            }
            let at = scratch.len();
            scratch.extend_from_slice(element.unwrap_or_default());
            scratch.resize(at + stride, 0);
        }
        buffers.write_storage(gpu, id, scratch, context.device());
    }

    if pushed.iter().any(|pushed| pushed.program.projection == Projection::PerDraw) {
        scratch.clear();
        for pushed in pushed {
            let block = pushed.projection.filter(|_| pushed.program.projection == Projection::PerDraw);
            scratch.extend_from_slice(bytemuck::bytes_of(&block.unwrap_or_default()));
        }
        buffers.write_storage(gpu, PROJECTION_BUFFER.id, scratch, context.device());
    } else {
        bind_no_projection(gpu, buffers, context);
    }
}

/// A layer's draws of a frame, in groups: the layer's [`main`](Draws::main)
/// group in its place in the layers' order. The engine keeps a layer's
/// groups from frame to frame, with the memory of their draws.
pub struct Draws {
    /// The main group, from the layer's first frame with something to draw:
    /// it then keeps its place in the layers' order and depth while the
    /// layer renders, in frames it draws nothing in too.
    main: Option<Group>,
}

impl Draws {
    pub(crate) fn new() -> Draws {
        Draws { main: None }
    }

    /// The group of the layer's draws in its place in the layers' order.
    pub fn main(&mut self) -> &mut Group {
        self.main.get_or_insert_with(Group::new)
    }

    /// Starts the layer's draws of a frame it has something to draw in, as
    /// [`Group::begin`] does.
    pub(crate) fn begin(&mut self, opaque: bool, frame_depth: bool) {
        self.main().begin(opaque, frame_depth);
    }

    /// Drops the frame's draws.
    pub(crate) fn clear(&mut self) {
        if let Some(main) = &mut self.main {
            main.clear();
        }
    }

    /// Makes the pushed draws the frame's, as [`Group::finish`] does.
    pub(crate) fn finish(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        tiles: impl IntoIterator<Item = UnwrappedTileID>,
    ) {
        if let Some(main) = &mut self.main {
            main.finish(gpu, context, tiles);
        }
    }

    /// The groups, in drawing order.
    pub(crate) fn groups(&self) -> impl Iterator<Item = &Group> {
        self.main.iter()
    }
}

/// One draw of a frame: the mesh it draws and the state it draws it with.
/// Its uniform values are the entries at its index in the storage buffers of
/// its `FrameDraws`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameDraw {
    /// The tile whose draw it is, or `None` for a draw of the layer's.
    pub(crate) tile: Option<OverscaledTileID>,
    /// The mesh it draws.
    mesh: MeshKey,
    /// The passes the draw draws in.
    pub(crate) pass: RenderPass,
    /// How the draw tests and writes depth.
    pub(crate) depth: Depth,
    /// Whether the draw clips to the mask of its tile, at the layer's depth
    /// or without a depth test.
    pub(crate) clip: bool,
}

/// Draws of a frame, and what they share.
pub(crate) struct FrameDraws {
    /// Tells the draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in drawing order. A draw's index is the index of its
    /// entries in the storage buffers, which shaders read as `drawIndex`.
    pub(crate) draws: Vec<FrameDraw>,
    /// The buffers the draws bind, by id.
    pub(crate) uniform_buffers: UniformBufferArray,
    /// How the draws composite onto the frame in the translucent pass, as one
    /// surface, after they draw into a texture of their own; `None` where
    /// they draw straight onto the frame.
    composite: Option<Composite>,
    /// The tiles whose masks the stencil buffer holds when the draws draw.
    /// Draws that don't clip need none, and neither does a layer without
    /// tiles, whose draws are none.
    pub(crate) stencil_tiles: Vec<UnwrappedTileID>,
}

impl Default for FrameDraws {
    /// No draws, with an id of their own.
    fn default() -> Self {
        FrameDraws {
            id: SimpleIdentity::new(),
            draws: Vec::new(),
            uniform_buffers: UniformBufferArray::new(),
            composite: None,
            stencil_tiles: Vec::new(),
        }
    }
}

impl FrameDraws {
    /// How the draws composite onto the frame, with the id that tells them
    /// apart in the layer composite, if there are any and they composite.
    #[must_use]
    pub(crate) fn composited(&self) -> Option<(SimpleIdentity, Composite)> {
        self.composite.filter(|_| !self.draws.is_empty()).map(|composite| (self.id, composite))
    }

    /// Uploads the meshes of the draws, in drawing order.
    pub(crate) fn upload(&self, gpu: &mut dyn Gpu, context: &mut Resources) {
        for draw in &self.draws {
            match context.take_mesh(draw.mesh) {
                Some(mut objects) => {
                    objects.upload(gpu, context);
                    context.restore_mesh(draw.mesh, objects);
                }
                None => debug_assert!(false, "a draw of tile {:?} names a mesh that went", draw.tile),
            }
        }
    }

    /// Draws the pass's draws into the open pass of `gpu`, the context's.
    pub(crate) fn render(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(pass) = self.begin(gpu, parameters) else {
            return;
        };
        for draw in pass.draws() {
            pass.draw(gpu, parameters, draw);
        }
        pass.end(gpu, parameters);
    }

    /// Begins drawing the draws of `parameters`' pass into the open pass of
    /// `gpu`, the context's: into the layer composite's texture in the
    /// translucent pass where they composite, and onto the frame otherwise.
    /// Where none of the pass's draws is 3D, this draws the masks of the
    /// tiles that those that clip clip to; 3D draws that draw `once` share
    /// one stencil value of their own. `None` where there is nothing to draw
    /// into or there are no draws; [`DrawPass::end`] ends what this begins.
    fn begin(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<DrawPass<'_>> {
        if self.draws.is_empty() || parameters.pass_target().is_none() {
            return None;
        }
        let composited = (self.composite.is_some() && parameters.pass == RenderPass::TRANSLUCENT)
            .then(|| parameters.begin_composited(gpu, self.id))
            .flatten();
        let drawing = composited.as_ref().is_none_or(Composited::draws);
        let draws = if drawing { &self.draws[..] } else { &[] };
        // A draw for both the opaque and the translucent pass draws in one:
        // the opaque where its layer draws there, and the translucent
        // otherwise. Drawn in both, it would cover itself, which the overdraw
        // inspector shows.
        let both = RenderPass::OPAQUE | RenderPass::TRANSLUCENT;
        let drawn_in =
            if parameters.opaque_pass_enabled_for_layer() { RenderPass::OPAQUE } else { RenderPass::TRANSLUCENT };
        let skip = if parameters.pass & both != RenderPass::NONE && parameters.pass != drawn_in {
            both
        } else {
            RenderPass::NONE
        };
        let mut pass = DrawPass { frame: self, draws, pass: parameters.pass, skip, volume_stencil: None, composited };
        let (volume, volume_once) =
            pass.draws().fold((false, false), |(volume, once), PassDraw { draw, .. }| match draw.depth {
                Depth::Volume { once: draw_once, .. } => (true, once || draw_once),
                _ => (volume, once),
            });
        if volume {
            pass.volume_stencil = volume_once.then(|| parameters.stencil_for_3d(gpu));
        } else if drawing && !self.stencil_tiles.is_empty() {
            parameters.render_tile_clipping_masks(gpu, &self.stencil_tiles);
        }
        Some(pass)
    }
}

/// The draws of a pass of a [`FrameDraws`], from [`FrameDraws::begin`] to
/// [`DrawPass::end`], which every pass that begins must reach.
#[must_use = "a pass's draws end with `DrawPass::end`"]
struct DrawPass<'a> {
    frame: &'a FrameDraws,
    /// The frame's draws, none where their layer composite's pass didn't
    /// begin.
    draws: &'a [FrameDraw],
    pass: RenderPass,
    /// The passes of the draws that draw in another pass: both the opaque
    /// and the translucent, or none.
    skip: RenderPass,
    /// The stencil of the 3D draws that draw `once`.
    volume_stencil: Option<Stencil>,
    composited: Option<Composited>,
}

/// A draw of a [`DrawPass`], as [`DrawPass::draws`] yields it.
#[derive(Clone, Copy)]
struct PassDraw<'a> {
    /// The draw's index among the frame's draws.
    index: usize,
    draw: &'a FrameDraw,
}

impl<'a> DrawPass<'a> {
    /// The draws of the pass, in drawing order.
    fn draws(&self) -> impl Iterator<Item = PassDraw<'a>> + use<'a> {
        let (pass, skip) = (self.pass, self.skip);
        let draws = self.draws.iter().enumerate();
        draws
            .filter(move |(_, draw)| draw.pass & pass != RenderPass::NONE)
            .filter(move |(_, draw)| skip == RenderPass::NONE || draw.pass & skip != skip)
            .map(|(index, draw)| PassDraw { index, draw })
    }

    /// Draws `draw`, one of the pass's, with the GPU objects of its mesh.
    fn draw(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, draw: PassDraw<'_>) {
        let PassDraw { index, draw } = draw;
        let Some(mut objects) = parameters.context.take_mesh(draw.mesh) else {
            debug_assert!(false, "a draw of tile {:?} names a mesh that went", draw.tile);
            return;
        };
        objects.bind_atlases(parameters);
        let tile_stencil = || match draw.tile.filter(|_| draw.clip) {
            Some(tile) => parameters.stencil_for_clipping(&tile.to_unwrapped()),
            None => Stencil::DISABLED,
        };
        let (depth, stencil) = match draw.depth {
            Depth::Off => (DepthTest::DISABLED, tile_stencil()),
            Depth::Layer { sub_layer, write } => (parameters.depth_for_sublayer(sub_layer, write), tile_stencil()),
            Depth::Volume { write, once } => {
                (parameters.depth_for_3d(write), self.volume_stencil.filter(|_| once).unwrap_or(Stencil::DISABLED))
            }
        };
        objects.draw(gpu, parameters, &self.frame.uniform_buffers, depth, stencil, index as u32);
        parameters.context.restore_mesh(draw.mesh, objects);
    }

    /// Ends the pass's draws: composites what they drew where they drew
    /// into the layer composite's texture.
    fn end(self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(composited) = self.composited {
            parameters.end_composited(gpu, composited);
        }
    }
}

/// What the frame draws of a layer in one place in the layers' order and
/// depth.
#[derive(Clone, Copy)]
pub(crate) enum DrawGroup<'a> {
    /// Draws of the layer's meshes.
    Draws(&'a FrameDraws),
}

impl<'a> DrawGroup<'a> {
    /// The tiles whose masks the stencil buffer holds when the group draws;
    /// `None` for a group that needs no masks.
    #[must_use]
    pub(crate) fn stencil_tiles(self) -> Option<&'a [UnwrappedTileID]> {
        match self {
            DrawGroup::Draws(frame) => Some(&frame.stencil_tiles[..]).filter(|tiles| !tiles.is_empty()),
        }
    }

    /// How the group composites onto the frame, with the id that tells it
    /// apart in the layer composite, if it draws anything and composites.
    #[must_use]
    pub(crate) fn composite(self) -> Option<(SimpleIdentity, Composite)> {
        match self {
            DrawGroup::Draws(frame) => frame.composited(),
        }
    }
}

/// The order the frame draws the layers' draw groups in, each at a depth
/// of its own, which [`DrawOrder::step`] moves the encode to after the
/// group draws.
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
    /// Moves the encode `parameters` to the depth of the group after the one
    /// it is at.
    pub(crate) fn step(self, parameters: &mut PaintParameters<'_>) {
        match self {
            DrawOrder::BottomUp => parameters.current_layer = parameters.current_layer.saturating_sub(1),
            DrawOrder::TopDown => parameters.current_layer += 1,
        }
    }
}
