//! The encode of a frame's draws.
//!
//! A layer pushes its draws of a frame into its [`Draws`]: each a [`Draw`]
//! of one of its meshes, with the values of the mesh's program's blocks of
//! each draw ([`DrawBlocks`]) and, for a draw of a tile, the tile's
//! [`TileProjection`]. The engine then sorts each [`Group`] by the draws'
//! order and when they were pushed, writes the
//! buffers of each of its programs once for all of the program's draws,
//! places the group in the layers' order and depth, and draws it in the
//! frame's passes: with the
//! masks of the layer's tiles in the stencil where its draws clip, into a
//! texture of its own where it composites, and with no call back into the
//! layer. A group of draws into one of the layer's targets draws before the
//! passes. [`FrameDraws`] are a group's draws as the engine draws them.

use std::any::Any;
use std::ops::Range;

use mln_gfx::gpu::{Gpu, Scissor};
use mln_gfx::shaders::block::{Block, DrawBlocks, UniformBlock};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{BufferKind, PROJECTION_BUFFER, Program, Projection};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use smallvec::SmallVec;

use crate::draw_objects::Mesh;
use crate::gfx::context::Resources;
use crate::gfx::store::MeshKey;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::handles::Target;
use crate::layer::LendAt;
use crate::layer_composite::Composite;
use crate::paint_parameters::{Composited, DepthTest, PaintParameters, Stencil};
use crate::paint_property_binder::MeshPaint;
use crate::render_pass::Pass;
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
    /// all the layer's tiles: lower orders draw first, and draws of one
    /// order in the order they were pushed.
    pub order: i64,
    /// Whether the draw covers what is behind it, so that it may draw in
    /// the opaque pass: it does where its layer's evaluation says the layer
    /// may ([`Evaluation::opaque`](crate::layer::Evaluation::opaque)) and
    /// the layer draws there (front to back, below the lowest layer of 3D
    /// geometry on a flat map), and otherwise in the translucent pass. It
    /// draws once, in one of the two. A draw that isn't opaque draws in the
    /// translucent pass.
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
    program: &'static Program,
    /// The tile whose mask the draw clips to.
    tile: Option<OverscaledTileID>,
    /// The projection block of the tile it was pushed with.
    projection: Option<ProjectionUBO>,
    /// Where its blocks start in the pushes' bytes.
    blocks: usize,
}

/// A group's draws of a frame as they were pushed, with the bytes of their
/// blocks and of the blocks they share.
#[derive(Default)]
struct Pushes {
    pushed: Vec<Pushed>,
    /// The bytes of the pushed draws' blocks.
    bytes: Vec<u8>,
    /// The blocks that all the draws share, each its layout and where its
    /// bytes are in `block_bytes`.
    blocks: Vec<(&'static Block, Range<usize>)>,
    block_bytes: Vec<u8>,
    /// Where a write of one of the draws' buffers gathers its bytes.
    scratch: Vec<u8>,
    /// The paint values of the first pushed mesh of each program that has
    /// any, which the engine writes into the blocks its draws share.
    paints: SmallVec<[(&'static Program, MeshPaint); 2]>,
}

/// A layer's draws of a frame in one place in the layers' order and depth,
/// which the engine sorts and draws in each of the frame's passes.
pub struct Group {
    /// The draws, as the engine draws them, once it sorted them.
    pub(crate) frame: FrameDraws,
    pushes: Pushes,
    /// Whether the group's draws may draw in the opaque pass.
    opaque: bool,
    /// Whether the group can composite with the frame's depth.
    frame_depth: bool,
    /// Whether the group draws into a target, where it doesn't composite.
    into_target: bool,
    /// Whether the group draws into a named target, where it composites
    /// with no blend that reads the backdrop.
    named: bool,
    /// Whether the frame refused the group a composite of a blend that reads
    /// the backdrop, in a named target.
    refused_backdrop: bool,
    /// The camera's zoom, at which the draws mix their paint values' zoom
    /// stops.
    zoom: f32,
}

impl Group {
    pub(crate) fn new() -> Group {
        Group {
            frame: FrameDraws::default(),
            pushes: Pushes::default(),
            opaque: false,
            frame_depth: false,
            into_target: false,
            named: false,
            refused_backdrop: false,
            zoom: 0.0,
        }
    }

    /// Adds a draw of `mesh`, not of a tile, with `blocks`, the values of
    /// the draw's own of its program's storage buffers of an element per
    /// draw. Its program's projection block, if it reads one
    /// ([`Projection::PerDraw`]), is zeros.
    ///
    /// The members of the blocks that a mesh's paint values are the
    /// engine's ([`MeshDesc::paint`](crate::draw::MeshDesc::paint)): the
    /// factor `<name>_t` that mixes each of its program's properties at two
    /// zooms, in `blocks`, and each property's value where it is the same
    /// for every feature, `<name>`, in the group's [`block`](Self::block)s.
    /// The engine writes them, over what the layer gave.
    pub fn push<B: DrawBlocks>(&mut self, mesh: &Mesh<B>, draw: Draw, blocks: B) -> &mut Self {
        self.add(mesh, None, draw, blocks)
    }

    /// Adds a draw of `mesh` projected as `at` says, with `blocks`, as
    /// [`push`](Self::push) does: the engine writes `at` as the projection
    /// block of the draw, for a program that reads one
    /// ([`Projection::PerDraw`]), and `at`'s Mercator matrix
    /// ([`TileProjection::fallback_matrix`]) as the member `matrix` of
    /// `blocks`, a `mat4x4<f32>`, where they have one. The draw clips to
    /// `at`'s tile where it clips.
    pub fn push_at<B: DrawBlocks>(&mut self, mesh: &Mesh<B>, at: &TileProjection, draw: Draw, blocks: B) -> &mut Self {
        self.add(mesh, Some(at), draw, blocks)
    }

    /// Makes `block` the value that the group's draws read of their
    /// programs' uniform block of its type in the frame, the last given. A
    /// draw of a program without one ignores it.
    pub fn block<T: UniformBlock>(&mut self, block: &T) -> &mut Self {
        let Pushes { blocks, block_bytes, .. } = &mut self.pushes;
        let bytes = bytemuck::bytes_of(block);
        if let Some((_, range)) = blocks.iter().find(|(given, _)| std::ptr::eq(*given, T::BLOCK)) {
            block_bytes[range.clone()].copy_from_slice(bytes);
        } else {
            let start = block_bytes.len();
            block_bytes.extend_from_slice(bytes);
            blocks.push((T::BLOCK, start..block_bytes.len()));
        }
        self
    }

    /// Makes the group's draws draw into a texture of their own in the
    /// translucent pass, which then composites onto the frame as
    /// `composite` says. Returns false, changing nothing, for a group of
    /// draws into a target ([`Draws::target`]), for a composite with the
    /// frame's depth ([`Composite::frame_depth`]) where the frame can't draw
    /// its depth together with that texture, and for a blend that reads
    /// what is behind the layer (`multiply`, `overlay`, `color`,
    /// `luminosity`) in a group that the style draws into a named target
    /// ([`NamedTarget`](crate::draw::NamedTarget)).
    pub fn composite(&mut self, composite: Composite) -> bool {
        if self.into_target || (composite.frame_depth && !self.frame_depth) {
            return false;
        }
        if self.named && composite.reads_backdrop() {
            self.refused_backdrop = true;
            return false;
        }
        self.frame.composite = Some(composite);
        true
    }

    fn add<B: DrawBlocks>(&mut self, mesh: &Mesh<B>, at: Option<&TileProjection>, draw: Draw, blocks: B) -> &mut Self {
        let program = mesh.program();
        debug_assert!(
            !draw.clip
                || (matches!(draw.depth, Depth::Off | Depth::Layer { .. }) && at.is_some_and(|at| at.tile.is_some())),
            "a draw of {} that clips without a tile, or in 3D",
            program.name
        );
        debug_assert!(
            !matches!(draw.depth, Depth::Layer { sub_layer, .. } if sub_layer > 2),
            "a draw of {} at a sub-layer past 2",
            program.name
        );
        let Pushes { pushed, bytes, paints, .. } = &mut self.pushes;
        let start = bytes.len();
        blocks.each_bytes(|block| bytes.extend_from_slice(block));
        if let Some(at) = at {
            for &matrix in mesh.matrices() {
                bytes[start + matrix..start + matrix + 64].copy_from_slice(bytemuck::bytes_of(&at.fallback_matrix));
            }
        }
        if let Some(mesh_paint) = mesh.paint() {
            mesh_paint.write_mixes(self.zoom, &mut bytes[start..]);
            if !paints.iter().any(|(of, _)| std::ptr::eq(*of, program)) {
                paints.push((program, mesh_paint.clone()));
            }
        }
        pushed.push(Pushed {
            draw,
            index: pushed.len() as u32,
            mesh: mesh.key(),
            program,
            tile: at.and_then(|at| at.tile),
            projection: at.map(|at| at.block),
            blocks: start,
        });
        self
    }

    /// Starts the group's draws of a frame at the camera's `zoom`: none, in
    /// no composite, which may draw in the opaque pass where `opaque`, and
    /// composite with the frame's depth where `frame_depth`.
    pub(crate) fn begin(&mut self, zoom: f32, opaque: bool, frame_depth: bool) {
        self.clear();
        self.frame.composite = None;
        self.zoom = zoom;
        self.opaque = opaque;
        self.frame_depth = frame_depth;
        self.named = false;
        self.refused_backdrop = false;
    }

    /// Makes the group of the frame draw into a named target, where it
    /// composites with no blend that reads the backdrop, nor with the
    /// frame's depth.
    fn route(&mut self) {
        self.named = true;
        self.frame_depth = false;
    }

    /// Drops the frame's draws.
    pub(crate) fn clear(&mut self) {
        self.frame.draws.clear();
        self.pushes.clear();
    }

    /// Makes the pushed draws the frame's, in their order, and, where there
    /// are any, writes the buffers they read, with the uniform values of
    /// `paint`, the layer's evaluated paint values. Draws that clip clip to
    /// the masks of `tiles`.
    pub(crate) fn finish(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        paint: &dyn Any,
        tiles: impl IntoIterator<Item = UnwrappedTileID>,
    ) {
        let Group { frame, pushes, opaque, .. } = self;
        pushes.pushed.sort_unstable_by_key(|pushed| (pushed.draw.order, pushed.index));
        frame.draws.clear();
        let mut counts: SmallVec<[u32; 4]> = SmallVec::new();
        for pushed in &pushes.pushed {
            let run = frame.run(pushed.program);
            counts.resize(counts.len().max(run + 1), 0);
            let element = counts[run];
            counts[run] += 1;
            frame.draws.push(FrameDraw {
                tile: pushed.tile,
                mesh: pushed.mesh,
                run: run as u16,
                element,
                opaque: pushed.draw.opaque && *opaque,
                depth: pushed.draw.depth,
                clip: pushed.draw.clip,
            });
        }
        frame.stencil_tiles.clear();
        if pushes.pushed.iter().any(|pushed| pushed.draw.clip) {
            frame.stencil_tiles.extend(tiles);
        }
        for (run, Run { program, buffers }) in frame.runs.iter_mut().enumerate() {
            if counts.get(run).is_some_and(|&count| count > 0) {
                pushes.write(gpu, context, program, buffers, paint);
            }
        }
        pushes.clear();
    }

    /// Uploads the meshes of the frame's draws.
    pub(crate) fn upload(&self, gpu: &mut dyn Gpu, context: &mut Resources) {
        self.frame.upload(gpu, context);
    }

    /// Draws `mesh` into the open pass of `gpu`, the context's, with `depth`
    /// and `stencil`, reading the buffers of the frame's draws of its
    /// program as their draw at `element` among them.
    pub(crate) fn draw_mesh<B>(
        &self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        mesh: &Mesh<B>,
        element: u32,
        depth: DepthTest,
        stencil: Stencil,
    ) {
        let Some(run) = self.frame.runs.iter().find(|run| std::ptr::eq(run.program, mesh.program())) else { return };
        let Some(mut objects) = parameters.context.take_mesh(mesh.key()) else { return };
        objects.draw(gpu, parameters, &run.buffers, depth, stencil, element);
        parameters.context.restore_mesh(mesh.key(), objects);
    }

    /// Draws the pass's draws of the frame into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render(gpu, parameters);
    }
}

/// The blocks of each draw of `program`, in its order, each with its
/// buffer's id and the bytes between two draws' elements of a storage
/// buffer, or `None` for a block of a draw's own.
fn blocks_of_each_draw(program: &Program) -> impl Iterator<Item = (&'static Block, usize, Option<usize>)> + '_ {
    program.buffers.iter().filter_map(|buffer| match buffer.kind {
        BufferKind::Storage { block } => Some((block, buffer.id, Some(block.size))),
        BufferKind::DrawUniform(block) => Some((block, buffer.id, None)),
        _ => None,
    })
}

impl Pushes {
    fn clear(&mut self) {
        self.pushed.clear();
        self.bytes.clear();
        self.blocks.clear();
        self.block_bytes.clear();
        self.paints.clear();
    }

    /// Writes to `buffers` the buffers that the draws of `program`, in
    /// their order, read: each shared block at the id of the program's
    /// uniform block of its layout, with the uniform values of `paint`, the
    /// layer's evaluated paint values, as the program's meshes' paint says,
    /// the draws' blocks at the ids of the program's storage buffers, an
    /// element per draw, and their projection blocks, or a projection block
    /// of zeros where the program reads none of each draw.
    fn write(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        program: &'static Program,
        buffers: &mut UniformBufferArray,
        paint: &dyn Any,
    ) {
        let Pushes { pushed, bytes, blocks, block_bytes, scratch, paints } = self;
        let mesh_paint = paints.iter().find(|(of, _)| std::ptr::eq(*of, program)).map(|(_, paint)| paint);
        for buffer in program.buffers {
            let BufferKind::Uniform(of) = buffer.kind else { continue };
            let Some((block, range)) = blocks.iter().find(|(block, _)| block.name == of.name) else {
                debug_assert!(false, "a group whose draws of {} read a uniform block it has no value of", program.name);
                continue;
            };
            scratch.clear();
            scratch.extend_from_slice(&block_bytes[range.clone()]);
            if let Some(mesh_paint) = mesh_paint {
                mesh_paint.write_uniforms(paint, program, block, scratch);
            }
            buffers.write_uniform(gpu, buffer.id, scratch, context.device());
        }

        let drawn = || pushed.iter().filter(|pushed| std::ptr::eq(pushed.program, program));
        let mut start = 0;
        for (block, id, stride) in blocks_of_each_draw(program) {
            if let Some(stride) = stride {
                scratch.clear();
                for pushed in drawn() {
                    let at = pushed.blocks + start;
                    scratch.extend_from_slice(&bytes[at..at + block.size]);
                    scratch.resize(scratch.len() + stride - block.size, 0);
                }
                buffers.write_storage(gpu, id, scratch, context.device());
            }
            start += block.size;
        }

        // A draw of a program with blocks of its own writes them to its
        // mesh's buffers, so its mesh draws once.
        let own = program.projection == Projection::Own;
        if own || blocks_of_each_draw(program).any(|(_, _, stride)| stride.is_none()) {
            debug_assert!(
                drawn().enumerate().all(|(i, pushed)| drawn().take(i).all(|before| before.mesh != pushed.mesh)),
                "a mesh of {} with blocks of its own that draws twice",
                program.name
            );
            for pushed in drawn() {
                let Some(mut objects) = context.take_mesh(pushed.mesh) else { continue };
                let mut start = pushed.blocks;
                for (block, id, stride) in blocks_of_each_draw(program) {
                    if stride.is_none() {
                        objects.write_own(gpu, id, &bytes[start..start + block.size], context.device());
                    }
                    start += block.size;
                }
                if own {
                    let projection = pushed.projection.unwrap_or_default();
                    objects.write_own(gpu, PROJECTION_BUFFER.id, bytemuck::bytes_of(&projection), context.device());
                }
                context.restore_mesh(pushed.mesh, objects);
            }
        }

        if program.projection == Projection::PerDraw {
            scratch.clear();
            for pushed in drawn() {
                scratch.extend_from_slice(bytemuck::bytes_of(&pushed.projection.unwrap_or_default()));
            }
            buffers.write_storage(gpu, PROJECTION_BUFFER.id, scratch, context.device());
        } else {
            bind_no_projection(gpu, buffers, context);
        }
    }
}

/// A layer's draws of a frame, in groups: the layer's [`main`](Draws::main)
/// group in its place in the layers' order, its [`overlay`](Draws::overlay)
/// above it, and the draws into each of its [`target`](Draws::target)s
/// before the frame's passes, with the backend's commands that the layer
/// asks the frame to [`lend`](Draws::lend) it. The engine keeps a layer's
/// groups from frame to frame, with the memory of their draws: the main
/// group and the overlay each keep their place in the layers' order and
/// depth while the layer renders, in frames they draw nothing in too, the
/// overlay from the first frame that asks for it.
pub struct Draws {
    main: Group,
    overlay: Option<Group>,
    /// The groups of the targets that the frame draws into, in the order it
    /// first asked for them, then spare groups of the last frame's.
    targets: Vec<(Target, Group)>,
    /// How many of `targets` the frame asked for.
    targets_used: usize,
    /// Whether the frame lends the layer the backend's commands before its
    /// passes, and in its translucent pass.
    lends: [bool; 2],
    /// The frame, as `begin` started the main group's.
    frame: (f32, bool, bool),
    /// Whether the main group and the overlay draw into a named target.
    routed: bool,
}

impl Draws {
    pub(crate) fn new() -> Draws {
        Draws {
            main: Group::new(),
            overlay: None,
            targets: Vec::new(),
            targets_used: 0,
            lends: [false; 2],
            frame: (0.0, false, false),
            routed: false,
        }
    }

    /// The group of the layer's draws in its place in the layers' order.
    pub fn main(&mut self) -> &mut Group {
        &mut self.main
    }

    /// The group of the layer's draws over its main group's: in a place of
    /// its own in the layers' order, right above the layer's, which it takes
    /// in the first frame that asks for it.
    pub fn overlay(&mut self) -> &mut Group {
        let (zoom, opaque, frame_depth) = self.frame;
        let routed = self.routed;
        self.overlay.get_or_insert_with(|| {
            let mut overlay = Group::new();
            overlay.begin(zoom, opaque, frame_depth);
            if routed {
                overlay.route();
            }
            overlay
        })
    }

    /// The group of the layer's draws into `target`, which draws into it
    /// before the frame's passes, once they cleared it: in each of the
    /// frame's passes, at its own depth, with no masks and in no composite.
    /// The frame draws into the targets of a layer in the order it first
    /// asked for their groups, and clears each that it asked for, whether or
    /// not it draws into it. The layers' targets draw bottom to top, but a
    /// layer's after the named targets it reads
    /// ([`UpdateCx::target`](crate::layer::UpdateCx::target)).
    pub fn target(&mut self, target: &Target) -> &mut Group {
        let asked = self.targets[..self.targets_used].iter().position(|(asked, _)| asked == target);
        let index = asked.unwrap_or_else(|| self.begin_target(target));
        &mut self.targets[index].1
    }

    /// Asks the frame to lend the layer the backend's commands `at` its
    /// time ([`Layer::lend`](crate::layer::Layer::lend)): in the translucent
    /// pass, right after the layer's main group draws, at its depth.
    pub fn lend(&mut self, at: LendAt) -> &mut Self {
        self.lends[at as usize] = true;
        self
    }

    /// Whether the frame lends the layer the backend's commands `at` its
    /// time.
    pub(crate) fn lends(&self, at: LendAt) -> bool {
        self.lends[at as usize]
    }

    /// Starts the frame's group of `target`, in a spare group if there is
    /// one, and returns its index.
    fn begin_target(&mut self, target: &Target) -> usize {
        let index = self.targets_used;
        self.targets_used += 1;
        if index == self.targets.len() {
            self.targets.push((target.clone(), Group::new()));
        } else {
            self.targets[index].0 = target.clone();
        }
        let (zoom, opaque, _) = self.frame;
        let group = &mut self.targets[index].1;
        group.begin(zoom, opaque, false);
        group.into_target = true;
        index
    }

    /// Starts the layer's draws of a frame, as [`Group::begin`] does.
    pub(crate) fn begin(&mut self, zoom: f32, opaque: bool, frame_depth: bool) {
        self.frame = (zoom, opaque, frame_depth);
        self.routed = false;
        self.main.begin(zoom, opaque, frame_depth);
        if let Some(overlay) = &mut self.overlay {
            overlay.begin(zoom, opaque, frame_depth);
        }
        self.targets_used = 0;
        self.lends = [false; 2];
    }

    /// Makes the main group and the overlay of the frame draw into a named
    /// target, as [`Group::route`] says.
    pub(crate) fn route(&mut self) {
        self.routed = true;
        self.main.route();
        if let Some(overlay) = &mut self.overlay {
            overlay.route();
        }
    }

    /// Whether the frame refused a group of the layer a composite of a
    /// blend that reads the backdrop, in a named target.
    pub(crate) fn refused_backdrop(&self) -> bool {
        self.groups().any(|group| group.refused_backdrop)
    }

    /// Drops the frame's draws.
    pub(crate) fn clear(&mut self) {
        self.main.clear();
        if let Some(overlay) = &mut self.overlay {
            overlay.clear();
        }
        self.targets.clear();
        self.targets_used = 0;
        self.lends = [false; 2];
    }

    /// Makes the pushed draws the frame's, as [`Group::finish`] does.
    pub(crate) fn finish(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        paint: &dyn Any,
        tiles: impl IntoIterator<Item = UnwrappedTileID> + Clone,
    ) {
        self.main.finish(gpu, context, paint, tiles.clone());
        if let Some(overlay) = &mut self.overlay {
            overlay.finish(gpu, context, paint, tiles);
        }
        self.targets.truncate(self.targets_used);
        for (_, group) in &mut self.targets {
            debug_assert!(
                !group.pushes.pushed.iter().any(|pushed| pushed.draw.clip),
                "a draw into a target that clips"
            );
            group.finish(gpu, context, paint, []);
        }
    }

    /// The main group.
    pub(crate) fn main_group(&self) -> &Group {
        &self.main
    }

    /// The groups in the layers' order, bottom to top.
    pub(crate) fn groups(&self) -> impl DoubleEndedIterator<Item = &Group> {
        std::iter::once(&self.main).chain(&self.overlay)
    }

    /// The groups of the frame's targets, in the order they draw.
    pub(crate) fn target_groups(&self) -> impl Iterator<Item = &Group> {
        self.targets.iter().map(|(_, group)| group)
    }

    /// Draws the groups of the frame's targets into them through `gpu`,
    /// each in a pass of its own that clears the target first, outside the
    /// frame's passes.
    pub(crate) fn render_targets(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        for (target, group) in &self.targets {
            let size = match parameters.context.target_pass(gpu, target, None) {
                Ok(pass) => parameters.begin_pass(gpu, pass).map(|()| pass.size),
                Err(error) => Err(error),
            };
            let size = match size {
                Ok(size) => size,
                Err(error) => {
                    Log::error(Event::Render, format!("Beginning a target's pass failed: {error}"));
                    continue;
                }
            };
            let frame_scissor = std::mem::replace(
                &mut parameters.scissor,
                Scissor { x: 0, y: 0, width: size.width, height: size.height },
            );
            for pass in [Pass::Opaque, Pass::Translucent] {
                parameters.pass = pass;
                parameters.depth_range_size =
                    1.0 - (3 * PaintParameters::NUM_SUBLAYERS) as f32 * PaintParameters::DEPTH_EPSILON;
                parameters.current_layer = 0;
                group.render(gpu, parameters);
            }
            if let Err(error) = parameters.end_pass(gpu) {
                Log::error(Event::Render, format!("Ending a target's pass failed: {error}"));
            }
            parameters.scissor = frame_scissor;
        }
    }
}

/// One draw of a frame: the mesh it draws and the state it draws it with.
/// Its uniform values are the entries at its `element` in the storage
/// buffers of its program's [`Run`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameDraw {
    /// The tile whose mask it clips to.
    tile: Option<OverscaledTileID>,
    /// The mesh it draws.
    mesh: MeshKey,
    /// The index of its program's run among its `FrameDraws`'.
    run: u16,
    /// The index of its entries among those of its run, which its shaders
    /// read as `drawIndex`.
    element: u32,
    /// Whether it draws in the opaque pass where its layer does.
    opaque: bool,
    /// How the draw tests and writes depth.
    depth: Depth,
    /// Whether the draw clips to the mask of its tile.
    clip: bool,
}

/// The buffers that a group's draws of one program bind, by id, each with
/// an element per draw of the program where it holds one per draw: the
/// elements of the programs of a group differ in size.
struct Run {
    program: &'static Program,
    buffers: UniformBufferArray,
}

/// A group's draws of a frame, as the engine draws them, and what they
/// share.
pub(crate) struct FrameDraws {
    /// Tells the draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in drawing order.
    draws: Vec<FrameDraw>,
    /// The buffers of the draws of each program that the group has drawn
    /// with, kept from frame to frame.
    runs: Vec<Run>,
    /// How the draws composite onto the frame in the translucent pass, as one
    /// surface, after they draw into a texture of their own; `None` where
    /// they draw straight onto the frame.
    composite: Option<Composite>,
    /// The tiles whose masks the stencil buffer holds when the draws draw:
    /// none where no draw clips.
    stencil_tiles: Vec<UnwrappedTileID>,
}

impl Default for FrameDraws {
    /// No draws, with an id of their own.
    fn default() -> Self {
        FrameDraws {
            id: SimpleIdentity::new(),
            draws: Vec::new(),
            runs: Vec::new(),
            composite: None,
            stencil_tiles: Vec::new(),
        }
    }
}

impl FrameDraws {
    /// The index of the run of `program`'s draws, made if there is none.
    fn run(&mut self, program: &'static Program) -> usize {
        self.runs.iter().position(|run| std::ptr::eq(run.program, program)).unwrap_or_else(|| {
            self.runs.push(Run { program, buffers: UniformBufferArray::new() });
            self.runs.len() - 1
        })
    }

    /// The tiles whose masks the stencil buffer holds when the draws draw;
    /// `None` where no draw clips.
    #[must_use]
    pub(crate) fn stencil_tiles(&self) -> Option<&[UnwrappedTileID]> {
        Some(&self.stencil_tiles[..]).filter(|tiles| !tiles.is_empty())
    }

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

    /// Draws the pass's draws into the open pass of `gpu`, the context's:
    /// into the layer composite's texture in the translucent pass where they
    /// composite, and onto the frame otherwise. Where none of the pass's
    /// draws is 3D, the masks of the tiles that those that clip clip to go
    /// into the stencil first; 3D draws that draw `once` share one stencil
    /// value of their own.
    pub(crate) fn render(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.draws.is_empty() || parameters.pass_target().is_none() {
            return;
        }
        let composited = (self.composite.is_some() && parameters.pass == Pass::Translucent)
            .then(|| parameters.begin_composited(gpu, self.id))
            .flatten();
        let drawing = composited.as_ref().is_none_or(Composited::draws);
        let pass = parameters.pass;
        // An opaque draw draws in one pass: the opaque where its layer draws
        // there, and the translucent otherwise. Drawn in both, it would
        // cover itself, which the overdraw inspector shows.
        let opaque_in = if parameters.opaque_pass_enabled_for_layer() { Pass::Opaque } else { Pass::Translucent };
        let in_pass = |draw: &FrameDraw| pass == if draw.opaque { opaque_in } else { Pass::Translucent };
        let draws = if drawing { &self.draws[..] } else { &[] };
        let draws = || draws.iter().filter(|draw| in_pass(draw));
        let (volume, volume_once) = draws().fold((false, false), |(volume, once), draw| match draw.depth {
            Depth::Volume { once: draw_once, .. } => (true, once || draw_once),
            _ => (volume, once),
        });
        let volume_stencil = if volume {
            volume_once.then(|| parameters.stencil_for_3d(gpu))
        } else {
            if drawing && !self.stencil_tiles.is_empty() {
                parameters.render_tile_clipping_masks(gpu, &self.stencil_tiles);
            }
            None
        };
        for draw in draws() {
            self.draw(gpu, parameters, draw, volume_stencil);
        }
        if let Some(composited) = composited {
            parameters.end_composited(gpu, composited);
        }
    }

    /// Draws `draw` with the GPU objects of its mesh and the buffers of its
    /// run; a 3D draw that draws `once` with `volume_stencil`.
    fn draw(
        &self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        draw: &FrameDraw,
        volume_stencil: Option<Stencil>,
    ) {
        let Some(mut objects) = parameters.context.take_mesh(draw.mesh) else {
            debug_assert!(false, "a draw of tile {:?} names a mesh that went", draw.tile);
            return;
        };
        let tile_stencil = || match draw.tile.filter(|_| draw.clip) {
            Some(tile) => parameters.stencil_for_clipping(&tile.to_unwrapped()),
            None => Stencil::DISABLED,
        };
        let (depth, stencil) = match draw.depth {
            Depth::Off => (DepthTest::DISABLED, tile_stencil()),
            Depth::Layer { sub_layer, write } => (parameters.depth_for_sublayer(sub_layer, write), tile_stencil()),
            Depth::Volume { write, once } => {
                (parameters.depth_for_3d(write), volume_stencil.filter(|_| once).unwrap_or(Stencil::DISABLED))
            }
        };
        let buffers = &self.runs[usize::from(draw.run)].buffers;
        objects.draw(gpu, parameters, buffers, depth, stencil, draw.element);
        parameters.context.restore_mesh(draw.mesh, objects);
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

#[cfg(test)]
mod tests {
    use mln_gfx::shaders::layer_ubo::ProjectionUBO;
    use mln_gfx::types::TextureChannelDataType;
    use mln_tile_id::{CanonicalTileID, OverscaledTileID};
    use mln_util::Color;
    use mln_util::size::Size;

    use super::*;
    use crate::mesh_desc::MeshDesc;
    use crate::shaders::programs;

    /// A mesh of a triangle in `context`, made after those made before.
    fn mesh(context: &mut Resources) -> Mesh {
        let vertices = context.buffer::<[i16; 2]>(&[[0, 0], [1, 0], [0, 1]]);
        let triangles = context.index_buffer(&[0, 1, 2]);
        let mut desc = MeshDesc::new(&programs::GLOBE_DEPTH, &triangles);
        desc.vertices(&vertices);
        context.mesh(&desc)
    }

    /// The projection of the tile `id`, which draws can clip to.
    fn at(id: OverscaledTileID) -> TileProjection {
        TileProjection {
            matrix: [0.0; 16],
            fallback_matrix: [0.0; 16],
            translation: [0.0; 2],
            pixels_to_tile_units: 1.0,
            globe_extrude_scale: 0.0,
            block: ProjectionUBO::default(),
            tile: Some(id),
        }
    }

    #[test]
    fn a_group_draws_by_order_then_as_pushed() {
        crate::tests::with_context(|gpu, context| {
            let (older, newer) = (mesh(context), mesh(context));
            assert!(older.seq() < newer.seq());
            let mut group = Group::new();
            group.begin(0.0, false, false);
            group
                .push(&newer, Draw::DEFAULT, ())
                .push(&older, Draw { order: 1, ..Draw::DEFAULT }, ())
                .push(&older, Draw::DEFAULT, ())
                .push(&newer, Draw { order: -1, ..Draw::DEFAULT }, ());
            group.finish(gpu, context, &(), []);
            let drawn: Vec<MeshKey> = group.frame.draws.iter().map(|draw| draw.mesh).collect();
            assert_eq!(drawn, [newer.key(), newer.key(), older.key(), older.key()]);
        });
    }

    #[test]
    fn a_draw_reads_its_elements_among_those_of_its_programs_draws() {
        crate::tests::with_context(|gpu, context| {
            let depth = mesh(context);
            let vertices = context.buffer::<[i16; 2]>(&[[0, 0], [1, 0], [0, 1]]);
            let triangles = context.index_buffer(&[0, 1, 2]);
            let mut desc = MeshDesc::new(&programs::TILE_DEBUG, &triangles);
            desc.vertices(&vertices);
            let debug: Mesh = context.mesh(&desc);
            let mut group = Group::new();
            group.begin(0.0, false, false);
            group.block(&crate::shaders::ubo::TileDebugPropsUBO::default());
            group.push(&depth, Draw::DEFAULT, ()).push(&debug, Draw::DEFAULT, ()).push(&depth, Draw::DEFAULT, ());
            group.finish(gpu, context, &(), []);
            let read: Vec<(u16, u32)> = group.frame.draws.iter().map(|draw| (draw.run, draw.element)).collect();
            assert_eq!(read, [(0, 0), (1, 0), (0, 1)]);
            let programs: Vec<&str> = group.frame.runs.iter().map(|run| run.program.name).collect();
            assert_eq!(programs, [programs::GLOBE_DEPTH.name, programs::TILE_DEBUG.name]);
        });
    }

    #[test]
    fn an_opaque_draw_is_opaque_only_where_its_layer_may_draw_opaque() {
        crate::tests::with_context(|gpu, context| {
            let mesh = mesh(context);
            let mut group = Group::new();
            for layer_opaque in [false, true] {
                group.begin(0.0, layer_opaque, false);
                group.push(&mesh, Draw { opaque: true, ..Draw::DEFAULT }, ()).push(&mesh, Draw::DEFAULT, ());
                group.finish(gpu, context, &(), []);
                let opaque: Vec<bool> = group.frame.draws.iter().map(|draw| draw.opaque).collect();
                assert_eq!(opaque, [layer_opaque, false]);
            }
        });
    }

    #[test]
    fn a_group_masks_its_layers_tiles_only_where_a_draw_clips() {
        crate::tests::with_context(|gpu, context| {
            let mesh = mesh(context);
            let tile = |x| OverscaledTileID::new(1, 0, CanonicalTileID::new(1, x, 0));
            let id = tile(0);
            let tiles = [id.to_unwrapped(), tile(1).to_unwrapped()];
            let mut group = Group::new();
            group.begin(0.0, false, false);
            group.push_at(&mesh, &at(id), Draw::DEFAULT, ());
            group.finish(gpu, context, &(), tiles);
            assert_eq!(group.frame.stencil_tiles(), None);

            group.begin(0.0, false, false);
            group.push_at(&mesh, &at(id), Draw { clip: true, ..Draw::DEFAULT }, ());
            group.finish(gpu, context, &(), tiles);
            assert_eq!(group.frame.stencil_tiles(), Some(&tiles[..]));
            assert_eq!(group.frame.draws[0].tile, Some(id));
        });
    }

    #[test]
    fn a_layers_targets_draw_as_the_frame_first_asked_for_them_apart_from_its_groups() {
        crate::tests::with_context(|gpu, context| {
            let mesh = mesh(context);
            let [first, second] =
                [0, 1].map(|_| context.target(Size::new(4, 4), TextureChannelDataType::UnsignedByte, Color::black()));
            let mut draws = Draws::new();
            draws.begin(0.0, false, false);
            draws.target(&second).push(&mesh, Draw::DEFAULT, ());
            draws.target(&first);
            draws.target(&second).push(&mesh, Draw::DEFAULT, ());
            draws.finish(gpu, context, &(), []);
            let drawn: Vec<(bool, usize)> =
                draws.targets.iter().map(|(target, group)| (*target == second, group.frame.draws.len())).collect();
            assert_eq!(drawn, [(true, 2), (false, 0)]);
            assert_eq!(1, draws.groups().count());

            // A frame draws into the targets it asks for alone.
            draws.begin(0.0, false, false);
            draws.target(&first);
            draws.finish(gpu, context, &(), []);
            let drawn: Vec<bool> = draws.targets.iter().map(|(target, _)| *target == first).collect();
            assert_eq!(drawn, [true]);
        });
    }

    #[test]
    fn a_target_keeps_its_texture_through_a_resize() {
        let mut context = Resources::detached();
        let target = context.target(Size::new(4, 4), TextureChannelDataType::HalfFloat, Color::black());
        let texture = target.texture().clone();
        context.resize(&target, Size::new(8, 2));
        assert_eq!(*target.texture(), texture);
        assert_eq!(Size::new(8, 2), context.size(target.texture()));
        // The detached resources' device draws no half floats.
        let format = context.stored_texture(&target.texture().0).expect("the texture").format();
        assert_eq!(mln_gfx::gpu::TextureFormat::Rgba8Unorm, format);
    }

    #[test]
    fn a_groups_last_value_of_a_block_is_the_one_it_writes() {
        let mut group = Group::new();
        let view = |pixel_ratio| mln_gfx::shaders::layer_ubo::ViewUBO { pixel_ratio, ..Default::default() };
        group.block(&view(1.0)).block(&view(2.0));
        let Pushes { blocks, block_bytes, .. } = &group.pushes;
        assert_eq!(blocks.len(), 1);
        assert_eq!(bytemuck::from_bytes::<mln_gfx::shaders::layer_ubo::ViewUBO>(&block_bytes[..]).pixel_ratio, 2.0);
    }
}
