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

use std::any::Any;
use std::ops::Range;

use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::bindings::FIRST_VERTEX_BUFFER_ID;
use mln_gfx::shaders::block::{Block, DrawBlocks, UniformBlock};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::{BufferKind, PROJECTION_BUFFER, Program, Projection};
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;
use smallvec::SmallVec;

use crate::draw_objects::Mesh;
use crate::gfx::context::Resources;
use crate::gfx::store::MeshKey;
use crate::gfx::uniform_buffer::UniformBufferArray;
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
    /// all the layer's tiles: lower orders draw first. Draws of one order
    /// draw in the order their meshes were made, and draws of one mesh in
    /// the order they were pushed.
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
    /// When its mesh was made.
    seq: u64,
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
    /// The first pushed mesh's paint values that the engine writes into
    /// the shared blocks, with its program.
    paint: Option<(MeshPaint, &'static Program)>,
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
    /// The camera's zoom, at which the draws mix their paint values' zoom
    /// stops.
    zoom: f32,
}

impl Group {
    pub(crate) fn new() -> Group {
        Group { frame: FrameDraws::default(), pushes: Pushes::default(), opaque: false, frame_depth: false, zoom: 0.0 }
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
    /// ([`Projection::PerDraw`]), and the draw clips to `at`'s tile where it
    /// clips.
    pub fn push_at<B: DrawBlocks>(&mut self, mesh: &Mesh<B>, at: &TileProjection, draw: Draw, blocks: B) -> &mut Self {
        self.add(mesh, Some(at), draw, blocks)
    }

    /// Makes `block` the value that the group's draws read of their
    /// programs' uniform block of its type in the frame, the last given. A
    /// draw of a program without one ignores it.
    pub fn block<T: UniformBlock>(&mut self, block: &T) -> &mut Self {
        let Pushes { blocks, block_bytes, .. } = &mut self.pushes;
        let bytes = bytemuck::bytes_of(block);
        match blocks.iter().find(|(given, _)| std::ptr::eq(*given, T::BLOCK)) {
            Some((_, range)) => block_bytes[range.clone()].copy_from_slice(bytes),
            None => {
                let start = block_bytes.len();
                block_bytes.extend_from_slice(bytes);
                blocks.push((T::BLOCK, start..block_bytes.len()));
            }
        }
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
        let Pushes { pushed, bytes, paint, .. } = &mut self.pushes;
        let start = bytes.len();
        blocks.each_bytes(|block| bytes.extend_from_slice(block));
        if let Some(mesh_paint) = mesh.paint() {
            mesh_paint.write_mixes(self.zoom, &mut bytes[start..]);
            paint.get_or_insert_with(|| (mesh_paint.clone(), program));
        }
        pushed.push(Pushed {
            draw,
            index: pushed.len() as u32,
            mesh: mesh.key(),
            seq: mesh.seq(),
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
        pushes.pushed.sort_unstable_by_key(|pushed| (pushed.draw.order, pushed.seq, pushed.index));
        frame.draws.clear();
        frame.draws.extend(pushes.pushed.iter().map(|pushed| FrameDraw {
            tile: pushed.tile,
            mesh: pushed.mesh,
            opaque: pushed.draw.opaque && *opaque,
            depth: pushed.draw.depth,
            clip: pushed.draw.clip,
        }));
        frame.stencil_tiles.clear();
        if pushes.pushed.iter().any(|pushed| pushed.draw.clip) {
            frame.stencil_tiles.extend(tiles);
        }
        if !pushes.pushed.is_empty() {
            pushes.write_uniforms(paint);
            pushes.write(gpu, context, &mut frame.uniform_buffers);
        }
        pushes.clear();
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

impl Pushes {
    fn clear(&mut self) {
        self.pushed.clear();
        self.bytes.clear();
        self.blocks.clear();
        self.block_bytes.clear();
        self.paint = None;
    }

    /// Writes the uniform values of `paint`, a layer's evaluated paint
    /// values, into the shared blocks, as the pushed meshes' paint says.
    fn write_uniforms(&mut self, paint: &dyn Any) {
        let Pushes { blocks, block_bytes, paint: Some((mesh_paint, program)), .. } = self else { return };
        for (block, range) in blocks.iter() {
            mesh_paint.write_uniforms(paint, program, block, &mut block_bytes[range.clone()]);
        }
    }

    /// Writes to `buffers` the buffers that the draws, in their order,
    /// read: each shared block at the ids of the uniform blocks of its
    /// layout in the draws' programs, the draws' blocks at the ids of their
    /// programs' storage buffers, an element per draw, and their projection
    /// blocks, or a projection block of zeros where no draw's program reads
    /// one of each draw.
    fn write(&mut self, gpu: &mut dyn Gpu, context: &mut Resources, buffers: &mut UniformBufferArray) {
        let Pushes { pushed, bytes, blocks, block_bytes, scratch, .. } = self;
        let mut programs: SmallVec<[&'static Program; 4]> = SmallVec::new();
        for pushed in pushed.iter() {
            if !programs.iter().any(|program| std::ptr::eq(*program, pushed.program)) {
                programs.push(pushed.program);
            }
        }
        let mut written = [false; FIRST_VERTEX_BUFFER_ID];
        for (block, range) in blocks.iter() {
            for buffer in programs.iter().flat_map(|program| program.buffers) {
                if let BufferKind::Uniform(of) = buffer.kind
                    && of.name == block.name
                    && !std::mem::replace(&mut written[buffer.id], true)
                {
                    buffers.write_uniform(gpu, buffer.id, &block_bytes[range.clone()], context.device());
                }
            }
        }
        debug_assert!(
            programs.iter().all(|program| program.buffers.iter().all(|buffer| match buffer.kind {
                BufferKind::Uniform(of) => blocks.iter().any(|(block, _)| block.name == of.name),
                _ => true,
            })),
            "a group whose draws read a uniform block that it has no value of"
        );

        let mut strides = [None; FIRST_VERTEX_BUFFER_ID];
        for program in &programs {
            for (_, id, stride) in blocks_of_each_draw(program) {
                debug_assert!(!written[id], "a group whose programs read buffer {id} as a uniform and per draw");
                debug_assert!(
                    strides[id].is_none_or(|other| other == stride),
                    "programs of one group whose elements at {id} differ in size"
                );
                strides[id].get_or_insert(stride);
            }
        }
        for (id, stride) in strides.iter().enumerate().filter_map(|(id, stride)| Some((id, (*stride)?))) {
            scratch.clear();
            for pushed in pushed.iter() {
                let mut start = pushed.blocks;
                let mut element = None;
                for (block, at, _) in blocks_of_each_draw(pushed.program) {
                    if at == id {
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

        if programs.iter().any(|program| program.projection == Projection::PerDraw) {
            scratch.clear();
            for pushed in pushed.iter() {
                let block = pushed.projection.filter(|_| pushed.program.projection == Projection::PerDraw);
                scratch.extend_from_slice(bytemuck::bytes_of(&block.unwrap_or_default()));
            }
            buffers.write_storage(gpu, PROJECTION_BUFFER.id, scratch, context.device());
        } else {
            bind_no_projection(gpu, buffers, context);
        }
    }
}

/// A layer's draws of a frame, in groups: the layer's [`main`](Draws::main)
/// group in its place in the layers' order. The engine keeps a layer's
/// groups from frame to frame, with the memory of their draws: the main
/// group keeps its place in the layers' order and depth while the layer
/// renders, in frames it draws nothing in too.
pub struct Draws {
    main: Group,
}

impl Draws {
    pub(crate) fn new() -> Draws {
        Draws { main: Group::new() }
    }

    /// The group of the layer's draws in its place in the layers' order.
    pub fn main(&mut self) -> &mut Group {
        &mut self.main
    }

    /// Starts the layer's draws of a frame, as [`Group::begin`] does.
    pub(crate) fn begin(&mut self, zoom: f32, opaque: bool, frame_depth: bool) {
        self.main.begin(zoom, opaque, frame_depth);
    }

    /// Drops the frame's draws.
    pub(crate) fn clear(&mut self) {
        self.main.clear();
    }

    /// Makes the pushed draws the frame's, as [`Group::finish`] does.
    pub(crate) fn finish(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        paint: &dyn Any,
        tiles: impl IntoIterator<Item = UnwrappedTileID>,
    ) {
        self.main.finish(gpu, context, paint, tiles);
    }

    /// The groups, in drawing order.
    pub(crate) fn groups(&self) -> impl Iterator<Item = &Group> {
        std::iter::once(&self.main)
    }
}

/// One draw of a frame: the mesh it draws and the state it draws it with.
/// Its uniform values are the entries at its index in the storage buffers of
/// its `FrameDraws`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameDraw {
    /// The tile whose mask it clips to.
    tile: Option<OverscaledTileID>,
    /// The mesh it draws.
    mesh: MeshKey,
    /// Whether it draws in the opaque pass where its layer does.
    opaque: bool,
    /// How the draw tests and writes depth.
    depth: Depth,
    /// Whether the draw clips to the mask of its tile.
    clip: bool,
}

/// A group's draws of a frame, as the engine draws them, and what they
/// share.
pub(crate) struct FrameDraws {
    /// Tells the draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in drawing order. A draw's index is the index of its
    /// entries in the storage buffers, which shaders read as `drawIndex`.
    draws: Vec<FrameDraw>,
    /// The buffers the draws bind, by id.
    uniform_buffers: UniformBufferArray,
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
            uniform_buffers: UniformBufferArray::new(),
            composite: None,
            stencil_tiles: Vec::new(),
        }
    }
}

impl FrameDraws {
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
        let draws = || draws.iter().enumerate().filter(|(_, draw)| in_pass(draw));
        let (volume, volume_once) = draws().fold((false, false), |(volume, once), (_, draw)| match draw.depth {
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
        for (index, draw) in draws() {
            self.draw(gpu, parameters, index, draw, volume_stencil);
        }
        if let Some(composited) = composited {
            parameters.end_composited(gpu, composited);
        }
    }

    /// Draws `draw`, at `index` among the draws, with the GPU objects of its
    /// mesh; a 3D draw that draws `once` with `volume_stencil`.
    fn draw(
        &self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        index: usize,
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
        objects.draw(gpu, parameters, &self.uniform_buffers, depth, stencil, index as u32);
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
    use mln_tile_id::{CanonicalTileID, OverscaledTileID};

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
    fn a_group_draws_by_order_then_by_the_age_of_meshes_then_as_pushed() {
        crate::tests::with_context(|gpu, context| {
            let (older, newer) = (mesh(context), mesh(context));
            let mut group = Group::new();
            group.begin(0.0, false, false);
            group
                .push(&newer, Draw::DEFAULT, ())
                .push(&older, Draw { order: 1, ..Draw::DEFAULT }, ())
                .push(&older, Draw::DEFAULT, ())
                .push(&newer, Draw { order: -1, ..Draw::DEFAULT }, ());
            group.finish(gpu, context, &(), []);
            let drawn: Vec<MeshKey> = group.frame.draws.iter().map(|draw| draw.mesh).collect();
            assert_eq!(drawn, [newer.key(), older.key(), newer.key(), older.key()]);
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
    fn a_groups_last_value_of_a_block_is_the_one_it_writes() {
        let mut group = Group::new();
        let view = |pixel_ratio| mln_gfx::shaders::layer_ubo::ViewUBO { pixel_ratio, ..Default::default() };
        group.block(&view(1.0)).block(&view(2.0));
        let Pushes { blocks, block_bytes, .. } = &group.pushes;
        assert_eq!(blocks.len(), 1);
        assert_eq!(bytemuck::from_bytes::<mln_gfx::shaders::layer_ubo::ViewUBO>(&block_bytes[..]).pixel_ratio, 2.0);
    }
}
