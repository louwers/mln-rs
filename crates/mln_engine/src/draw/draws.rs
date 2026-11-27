//! A layer's draws of a frame, which the engine holds, writes, uploads and
//! draws.
//!
//! Each frame, a layer pushes its draws into its [`Draws`]' main
//! [`Group`]: each a [`Draw`] of one of its [`Mesh`]es, placed in the
//! group's order, with how it tests depth and whether it clips to its tile,
//! with the values of its program's blocks of each draw ([`DrawBlocks`]),
//! and, for a draw of a tile, the tile's projection. The blocks the group's
//! draws share go in once ([`Group::block`]), and the group may composite
//! onto the frame as one surface ([`Group::composite`]). A layer can also
//! push draws into textures of its own ([`Draws::target`]), which its draws
//! of the frame sample, and have the frame lend it the backend's commands
//! at its time in the frame ([`Draws::lend`]). The engine then
//! sorts the draws, writes the buffers they read once for the group, with
//! a drape's paint parameters where the frame is a drape's, uploads their
//! meshes, places the group in the layers' order and depth, and draws it in
//! the frame's passes, with the masks of the layer's tiles in the stencil
//! where its draws clip. A layer sees its frame as a [`LayerFrame`]: the
//! map's, or a drape's.

use std::any::Any;
use std::ops::Range;
use std::rc::Rc;

use mln_gfx::Size;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gpu::Gpu;
use mln_gfx::shaders::bindings::{BUFFER_IDS, PAINT_PARAMS_ID, PROJECTION_ID};
use mln_gfx::shaders::block::{Block, DrawBlocks, UniformBlock};
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::{BufferKind, Program, Projection};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::{CullFaceWindingType, DepthMaskType};

use crate::camera::{FrameProjection, ProjectionData, View, ViewportMode};
use crate::draw::after_gpu::AfterGpu;
use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::draw::objects::{DrawBuffers, Mesh};
use crate::draw::target::Target;
use crate::render::layer_composite::Composite;
use crate::render::light::EvaluatedLight;
use crate::render::paint_parameters::{Composited, PaintParameters, StencilClear, TransformParameters};
use crate::render::pass::RenderPass;
use crate::render::terrain::FrameGround;
use crate::tile::{OverscaledTileID, UnwrappedTileID};

/// What a layer's frame draws into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawsInto {
    /// The map.
    Map,
    /// A drape of a terrain tile, whose shading the terrain's surface adds
    /// where `lit` ([`SurfaceLight`](crate::render::terrain::SurfaceLight)).
    Drape { lit: bool },
}

/// The frame a layer makes its draws for, as it reads it: the camera, the
/// target's size and scale, the light and ground, and what it draws into.
#[derive(Clone)]
pub struct LayerFrame<'a> {
    pub view: &'a View,
    pub(crate) transform_params: &'a TransformParameters,
    /// Whether a gesture or an animation moves the camera: draws favor speed
    /// over crispness then.
    pub camera_moving: bool,
    pub light: &'a EvaluatedLight,
    pub pixel_ratio: f32,
    pub renderable_size: Size,
    pub pixels_to_gl_units: [f32; 2],
    /// Whether the frame draws terrain, under which 2D layers draw without
    /// depth.
    pub terrain: bool,
    /// Whether a group drawn as a whole ([`Group::composite`]) can draw
    /// with the frame's depth and stencil: the frame has them, and a pass can
    /// draw into them together with the layer composite's texture.
    pub composites_with_frame_depth: bool,
    /// The ground that the layers that stand on terrain stand on, where the
    /// frame has terrain and such layers.
    pub ground: Option<Rc<FrameGround>>,
    pub(crate) into: DrawsInto,
}

impl LayerFrame<'_> {
    /// The frame's camera.
    #[must_use]
    pub fn view(&self) -> &View {
        self.view
    }

    /// Whether a gesture or an animation moves the camera: draws favor
    /// speed over crispness then.
    #[must_use]
    pub fn camera_moving(&self) -> bool {
        self.camera_moving
    }

    /// The device pixels of the frame's target per logical pixel.
    #[must_use]
    pub fn pixel_ratio(&self) -> f32 {
        self.pixel_ratio
    }

    /// The size of the frame's target, in device pixels.
    #[must_use]
    pub fn size(&self) -> Size {
        self.renderable_size
    }

    /// Whether the frame is a drape whose shading the terrain's surface
    /// adds: a hillshade draws its slopes there, unlit.
    #[must_use]
    pub fn drape_lit(&self) -> bool {
        matches!(self.into, DrawsInto::Drape { lit: true })
    }

    /// See `TransformParameters::projection_data_for_tile`.
    #[must_use]
    pub fn projection_data_for_tile(&self, tile_id: &UnwrappedTileID, aligned: bool) -> ProjectionData {
        self.transform_params.projection_data_for_tile(tile_id, aligned)
    }

    /// Culling of the faces turned away from the camera, whose triangles
    /// wind clockwise on screen, or counterclockwise where a viewport
    /// flipped upside down turns every triangle round. The globe's
    /// projection draws the right way up in either mode.
    #[must_use]
    pub fn cull_back_faces(&self) -> CullFaceMode {
        let turned = self.view.viewport_mode() == ViewportMode::FlippedY && !self.view.is_globe();
        let winding = if turned { CullFaceWindingType::Clockwise } else { CullFaceWindingType::CounterClockwise };
        CullFaceMode { winding, ..CullFaceMode::back_ccw() }
    }
}

/// How a draw tests and writes depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// It draws without a depth test.
    Off,
    /// It tests the depth of a sub-layer of the layer's depth, and writes it
    /// as `mask` says.
    Layer { sub_layer: u8, mask: DepthMaskType },
    /// It tests the depth range of 3D geometry, which the 3D draws of all
    /// layers share, and writes it as `mask` says.
    Volume { mask: DepthMaskType },
}

/// How a draw of a mesh draws: its place among its group's draws, its pass,
/// its depth and whether it clips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// Where the draw is among its group's draws: lower orders draw first,
    /// and draws of one order in the order they were pushed. A layer that
    /// draws its meshes in the order they were made gives each draw its
    /// mesh's [`seq`](Mesh::seq).
    pub order: i64,
    /// Whether the draw may draw in the opaque pass, where its layer draws
    /// there, rather than in the translucent pass. It draws in one of the
    /// two.
    pub opaque: bool,
    pub depth: Depth,
    /// Whether the draw clips: a draw at the layer's depth to the mask of
    /// its tile ([`Group::push_at`]), a 3D one to where none of the group's
    /// 3D draws of the pass drew before it, so no pixel blends twice. A
    /// group with a draw that clips draws the masks of its layer's tiles.
    pub clip: bool,
}

impl Draw {
    /// A draw of order 0 in the translucent pass that tests, and doesn't
    /// write, the nearest depth of its layer, and doesn't clip.
    pub const DEFAULT: Draw = Draw {
        order: 0,
        opaque: false,
        depth: Depth::Layer { sub_layer: 0, mask: DepthMaskType::ReadOnly },
        clip: false,
    };

    /// The passes the draw draws in.
    fn passes(self) -> RenderPass {
        if self.opaque { RenderPass::Opaque | RenderPass::Translucent } else { RenderPass::Translucent }
    }
}

impl Default for Draw {
    fn default() -> Self {
        Draw::DEFAULT
    }
}

/// One of a group's draws as the engine draws it. Its uniform values are
/// the entries at its index in the storage buffers of its group.
struct GroupDraw {
    mesh: Mesh,
    /// The tile whose mask it clips to.
    tile: Option<OverscaledTileID>,
    pass: RenderPass,
    depth: Depth,
    clip: bool,
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
/// engine writes to the buffers of the group's draws.
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

    /// Writes to `buffers` the buffers that the pushed draws at `order`'s
    /// indexes, in drawing order, read: at the id of each of their
    /// programs' uniform blocks, the block of its layout that they share; at
    /// the id of each storage buffer of an element per draw, each draw's
    /// block, or zeros for a draw whose program reads none there; at the id
    /// of a uniform block of a draw's own, which one draw reads, its block;
    /// the draws' projections, or a projection of zeros where none reads
    /// one, which every program declares; and the frame's paint parameters,
    /// `paint_params`, where it has its own.
    fn write(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        order: &[(i64, usize)],
        paint_params: Option<&GlobalPaintParamsUBO>,
        buffers: &mut DrawBuffers,
    ) {
        let Pushes { pushed, bytes, programs, shared, shared_bytes, scratch } = self;
        for of in programs.iter_mut() {
            of.drawn = false;
        }
        for &(_, index) in order {
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
                    for &(_, index) in order {
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
                    let mut own = order.iter().map(|&(_, index)| &pushed[index]).filter(|draw| {
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
            for &(_, index) in order {
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

/// A layer's draws of a frame in one place in the layers' order and depth,
/// which the engine writes, uploads and draws in each of the frame's
/// passes, and what they share.
pub struct Group {
    /// Tells the group apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in their order. A draw's index is the index of its
    /// entries in the storage buffers, which shaders read as `globalIndex`.
    draws: Vec<GroupDraw>,
    /// The buffers the draws bind, by id, and the bindings of the meshes
    /// they draw.
    buffers: DrawBuffers,
    /// How the draws composite onto the frame in the translucent pass, as one
    /// surface, after they draw into a texture of their own; `None` where
    /// they draw straight onto the frame.
    composite: Option<Composite>,
    /// The tiles whose masks the stencil buffer holds when the draws draw:
    /// none where no draw clips.
    stencil_tiles: Vec<UnwrappedTileID>,
    /// The draws pushed for the next frame, by the index they were pushed
    /// at, until [`finish`](Self::finish) makes them the frame's.
    pending: Vec<Option<GroupDraw>>,
    /// The pushed draws' orders, each with the draw's index in `pending`.
    order: Vec<(i64, usize)>,
    pushes: Pushes,
}

impl Default for Group {
    /// No draws, with an id of their own.
    fn default() -> Self {
        Group {
            id: SimpleIdentity::new(),
            draws: Vec::new(),
            buffers: DrawBuffers::default(),
            composite: None,
            stencil_tiles: Vec::new(),
            pending: Vec::new(),
            order: Vec::new(),
            pushes: Pushes::default(),
        }
    }
}

impl Group {
    /// Adds a draw of `mesh`, not of a tile, to the next frame's, with
    /// `blocks`, the values of the draw's own of the blocks of each draw
    /// that the mesh's program reads, in its order. The draw's projection,
    /// where its program reads one, is zeros.
    pub fn push<B: DrawBlocks>(&mut self, mesh: &Mesh, draw: Draw, blocks: B) -> &mut Self {
        self.add(mesh, None, None, draw, blocks)
    }

    /// Adds a draw of `mesh` of the tile `tile` with `projection`, as
    /// [`push`](Self::push) does: the draw reads `projection` where its
    /// program reads a projection of each draw, and clips to `tile` where it
    /// clips.
    pub fn push_at<B: DrawBlocks>(
        &mut self,
        mesh: &Mesh,
        tile: OverscaledTileID,
        projection: &ProjectionUBO,
        draw: Draw,
        blocks: B,
    ) -> &mut Self {
        self.add(mesh, Some(tile), Some(projection), draw, blocks)
    }

    fn add<B: DrawBlocks>(
        &mut self,
        mesh: &Mesh,
        tile: Option<OverscaledTileID>,
        projection: Option<&ProjectionUBO>,
        draw: Draw,
        blocks: B,
    ) -> &mut Self {
        debug_assert!(
            !draw.clip || tile.is_some() || matches!(draw.depth, Depth::Volume { .. }),
            "a draw that clips without a tile"
        );
        let start = self.pushes.bytes.len();
        blocks.each_bytes(|block| self.pushes.bytes.extend_from_slice(block));
        self.pushes.push(mesh.program(), projection, start, B::BLOCKS);
        self.order.push((draw.order, self.pending.len()));
        self.pending.push(Some(GroupDraw {
            mesh: mesh.clone(),
            tile,
            pass: draw.passes(),
            depth: draw.depth,
            clip: draw.clip,
        }));
        self
    }

    /// Makes `block` the value that the next frame's draws read of their
    /// programs' uniform block of its layout, the last given. A draw whose
    /// program has none ignores it.
    pub fn block<T: UniformBlock>(&mut self, block: &T) -> &mut Self {
        self.pushes.block(T::BLOCK, bytemuck::bytes_of(block));
        self
    }

    /// Makes the draws composite onto the frame as `composite` says, or
    /// draw straight onto it for `None`.
    pub fn composite(&mut self, composite: Option<Composite>) -> &mut Self {
        self.composite = composite;
        self
    }

    /// Drops the draws, those pushed too, and the bindings of the meshes
    /// that dropped.
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
        self.clear_pushed();
        self.buffers.sweep();
    }

    fn clear_pushed(&mut self) {
        self.pending.clear();
        self.order.clear();
        self.pushes.clear();
    }

    /// Makes the pushed draws the frame's, in their order and in the order
    /// they were pushed among draws of the same order. Where there are any,
    /// it first writes the buffers they read, with `paint_params` where the
    /// frame has paint parameters of its own (a drape's). Where a draw
    /// clips, the group clips to the masks of `tiles`.
    pub(crate) fn finish(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        paint_params: Option<&GlobalPaintParamsUBO>,
        tiles: impl IntoIterator<Item = UnwrappedTileID>,
    ) {
        self.order.sort_unstable();
        if !self.order.is_empty() {
            self.pushes.write(gpu, context, &self.order, paint_params, &mut self.buffers);
        }
        let Group { draws, pending, order, stencil_tiles, .. } = self;
        draws.clear();
        draws.extend(order.iter().filter_map(|&(_, index)| pending[index].take()));
        stencil_tiles.clear();
        if draws.iter().any(|draw| draw.clip) {
            stencil_tiles.extend(tiles);
        }
        self.buffers.sweep();
        self.clear_pushed();
    }

    /// The tiles whose masks the stencil buffer holds when the group draws;
    /// `None` for a group that needs no masks.
    #[must_use]
    pub(crate) fn stencil_tiles(&self) -> Option<&[UnwrappedTileID]> {
        Some(&self.stencil_tiles[..]).filter(|tiles| !tiles.is_empty())
    }

    /// Whether the group draws anything.
    #[must_use]
    pub(crate) fn draws(&self) -> bool {
        !self.draws.is_empty()
    }

    /// How the draws composite onto the frame, with the id that tells them
    /// apart in the layer composite, if there are any and they composite.
    #[must_use]
    pub(crate) fn composited(&self) -> Option<(SimpleIdentity, Composite)> {
        self.composite.filter(|_| !self.draws.is_empty()).map(|composite| (self.id, composite))
    }

    /// The meshes of the draws, in drawing order.
    #[cfg(test)]
    pub(crate) fn meshes(&self) -> impl Iterator<Item = &Mesh> {
        self.draws.iter().map(|draw| &draw.mesh)
    }

    /// Uploads the meshes of the draws, in drawing order.
    pub(crate) fn upload(&self, gpu: &mut dyn Gpu, context: &mut Context) {
        for draw in &self.draws {
            draw.mesh.upload(gpu, context);
        }
    }

    /// Draws the draws of `parameters`' pass into the open pass of `gpu`,
    /// the context's: into the layer composite's texture in the translucent
    /// pass where they composite, and onto the frame otherwise. Where none
    /// of the pass's draws is 3D, this first draws the masks of the tiles
    /// that those that clip clip to; 3D draws that clip share one stencil
    /// value of their own. Returns whether no draw failed.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        if self.draws.is_empty() || parameters.pass_target().is_none() {
            return true;
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
        let pass = parameters.pass;
        let skip = if pass & both != RenderPass::None && pass != drawn_in { both } else { RenderPass::None };
        let in_pass = |draw: &&GroupDraw| {
            draw.pass & pass != RenderPass::None && (skip == RenderPass::None || draw.pass & skip != skip)
        };
        let (volume, volume_clips) =
            draws.iter().filter(in_pass).fold((false, false), |(volume, clips), draw| match draw.depth {
                Depth::Volume { .. } => (true, clips || draw.clip),
                _ => (volume, clips),
            });
        let mut volume_stencil = None;
        if volume {
            volume_stencil = volume_clips.then(|| parameters.stencil_mode_for_3d(gpu));
        } else if drawing && !self.stencil_tiles.is_empty() {
            parameters.render_tile_clipping_masks(gpu, &self.stencil_tiles);
        }
        let mut drawn = true;
        for (index, draw) in draws.iter().enumerate().filter(|(_, draw)| in_pass(draw)) {
            let tile_stencil = || match draw.tile.filter(|_| draw.clip) {
                Some(tile) => parameters.stencil_mode_for_clipping(&tile.to_unwrapped()),
                None => StencilMode::disabled(),
            };
            let (depth, stencil) = match draw.depth {
                Depth::Off => (DepthMode::disabled(), tile_stencil()),
                Depth::Layer { sub_layer, mask } => {
                    (parameters.depth_mode_for_sublayer(sub_layer, mask), tile_stencil())
                }
                Depth::Volume { mask } => (
                    parameters.depth_mode_for_3d(mask),
                    volume_stencil.filter(|_| draw.clip).unwrap_or_else(StencilMode::disabled),
                ),
            };
            let Some(mut objects) = parameters.context.take_mesh(&draw.mesh) else {
                debug_assert!(false, "a draw of a mesh the context doesn't have");
                continue;
            };
            drawn &= objects.draw(gpu, parameters, &mut self.buffers, depth, stencil, index as u32);
            parameters.context.put_mesh(&draw.mesh, objects);
        }
        if let Some(composited) = composited {
            parameters.end_composited(gpu, composited);
        }
        drawn
    }
}

/// When a frame lends a layer the backend's commands ([`Draws::lend`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LendAt {
    /// Before the frame's passes, after the layer drew into its targets
    /// ([`Draws::target`]): the frame's commands outside a pass.
    BeforePasses,
    /// In the frame's translucent pass, at the layer's place in the layers'
    /// order and depth, after the layer's draws there: the open pass.
    Pass,
}

/// What a frame lends a layer
/// ([`Layer::lend`](crate::layer::Layer::lend)).
pub struct Lent<'a> {
    /// The backend's commands, as its `Gpu` lends them
    /// ([`Gpu::lend`]): before the passes `mln_vulkan::CustomLayerEncoder`,
    /// `mln_mtl::CustomLayerEncoder`, `mln_gl::CustomLayerEncoder`,
    /// `mln_webgpu::CustomLayerEncoder` or `mln_d3d12::CustomLayerEncoder`,
    /// and in a pass the same crate's `CustomLayerPass`, whose docs say what
    /// the layer may change.
    pub raw: &'a mut dyn Any,
    /// The frame.
    pub frame: &'a LayerFrame<'a>,
    /// Where the frame draws the map, and the depth it writes there.
    pub projection: &'a FrameProjection,
    /// The depth that the frame's 2D layers write at the layer's place in
    /// the layers' order, in a pass; `None` before the passes.
    pub layer_depth: Option<f64>,
    /// Where the layer queues what waits for the GPU to finish what it
    /// drew with the commands, such as the freeing of what it made with
    /// them. A clone stays usable for as long as the layer.
    pub after_gpu: &'a AfterGpu,
}

/// A layer's draws of a frame, the map's or a drape's, which the engine
/// keeps between frames: its main group, which takes a place in the layers'
/// order and depth from the first frame that the layer has something to
/// draw in, even with no draws; a group of draws into each target of the
/// layer's that the frame draws into ([`target`](Self::target)); and where
/// the frame lends the layer the backend's commands ([`lend`](Self::lend)).
#[derive(Default)]
pub struct Draws {
    main: Group,
    /// The groups of the draws into the layer's targets, each with its
    /// target: the frame's, in the order the layer first pushed into them,
    /// then those of the last frame that the frame hasn't drawn into yet,
    /// which keep their buffers for it until the draws are written.
    targets: Vec<(Target, Group)>,
    /// How many of `targets` are the frame's.
    frame_targets: usize,
    /// Whether the frame lends the layer the backend's commands, at each
    /// [`LendAt`].
    lends: [bool; 2],
    /// Whether a frame started the draws, from which the main group takes
    /// its place.
    started: bool,
}

impl Draws {
    /// The layer's main group, which draws in its place in the layers' order
    /// and depth.
    pub fn main(&mut self) -> &mut Group {
        &mut self.main
    }

    /// The group of the frame's draws into `target`, a target of the
    /// layer's own. Once the frame's draws are written, the engine clears
    /// the target and draws the group into it, before the frame's passes
    /// or, in a drape, the drape's: in the opaque pass and then the
    /// translucent one, without depth from the frame, and without clipping
    /// to tiles. A target the frame asks for draws, cleared, even without
    /// draws. The frame's draws sample what it drew.
    pub fn target(&mut self, target: &Target) -> &mut Group {
        let frame = self.frame_targets;
        let at = match self.targets.iter().position(|(given, _)| given == target) {
            Some(at) if at < frame => return &mut self.targets[at].1,
            Some(at) => at,
            None => {
                self.targets.push((target.clone(), Group::default()));
                self.targets.len() - 1
            }
        };
        self.targets.swap(at, frame);
        self.frame_targets += 1;
        &mut self.targets[frame].1
    }

    /// Makes the frame lend the layer the backend's commands `at` its time
    /// in the frame
    /// ([`Layer::lend`](crate::layer::Layer::lend)).
    pub fn lend(&mut self, at: LendAt) -> &mut Self {
        self.lends[at as usize] = true;
        self
    }

    /// Whether the frame lends the layer the backend's commands `at` its
    /// time in the frame.
    #[must_use]
    pub(crate) fn lends(&self, at: LendAt) -> bool {
        self.lends[at as usize]
    }

    /// Starts the draws of a frame the layer has something to draw in: none
    /// yet, and the main group from now on takes its place.
    pub(crate) fn begin(&mut self) {
        self.started = true;
        self.reset();
    }

    /// Drops the frame's draws, and the groups of the draws into targets.
    /// The main group keeps its place.
    pub(crate) fn clear(&mut self) {
        self.reset();
        self.targets.clear();
    }

    /// Drops the frame's draws and lends.
    fn reset(&mut self) {
        for (_, group) in &mut self.targets {
            group.clear();
        }
        self.frame_targets = 0;
        self.lends = [false; 2];
        self.main.clear();
    }

    /// See [`Group::finish`], for every group: the draws into the frame's
    /// targets, which don't clip, first, whose targets then hold what the
    /// draws draw ([`Context::declare_target`]). The groups of the targets
    /// the frame doesn't draw into go.
    pub(crate) fn finish(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        paint_params: Option<&GlobalPaintParamsUBO>,
        tiles: impl IntoIterator<Item = UnwrappedTileID>,
    ) {
        self.finish_targets(gpu, context, paint_params);
        self.main.finish(gpu, context, paint_params, tiles);
    }

    /// The targets' part of [`finish`](Self::finish), which doesn't depend
    /// on the type of the tiles.
    fn finish_targets(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        paint_params: Option<&GlobalPaintParamsUBO>,
    ) {
        self.targets.truncate(self.frame_targets);
        for (target, group) in &mut self.targets {
            debug_assert!(group.composite.is_none(), "draws into a target that composite");
            group.finish(gpu, context, paint_params, []);
            context.declare_target(target);
        }
    }

    /// Visits the groups that take a place in the layers' order and depth,
    /// in drawing order.
    pub(crate) fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(&'a Group)) {
        if self.started {
            visit(&self.main);
        }
    }

    /// Uploads the meshes of the groups' draws: those into the frame's
    /// targets, then the main group's.
    pub(crate) fn upload(&self, gpu: &mut dyn Gpu, context: &mut Context) {
        if self.started {
            for (_, group) in &self.targets[..self.frame_targets] {
                group.upload(gpu, context);
            }
            self.main.upload(gpu, context);
        }
    }

    /// Clears each of the frame's targets and draws the draws into it
    /// ([`Target::render`]), outside any pass of `gpu`, the context's.
    pub(crate) fn render_targets(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        for (target, group) in &mut self.targets[..self.frame_targets] {
            target.render(gpu, parameters, |gpu, parameters| group.render(gpu, parameters));
        }
    }

    /// Draws the pass's draws of the groups into the open pass of `gpu`,
    /// the context's, each at its depth, in `order`; `in_pass`, if any, runs
    /// where the main group draws, after it, before the depth steps on.
    pub(crate) fn render(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
        in_pass: Option<&mut InPass<'_>>,
    ) {
        if self.started {
            self.main.render(gpu, parameters);
            if let Some(in_pass) = in_pass {
                in_pass(gpu, parameters);
            }
            order.step(parameters);
        }
    }
}

/// What [`Draws::render`] runs where the main group draws, in the open pass.
pub(crate) type InPass<'a> = dyn FnMut(&mut dyn Gpu, &mut PaintParameters<'_>) + 'a;

/// One mesh and a frame's draw of it, with the buffers it binds: a draw of
/// the engine's own.
pub(crate) struct SingleDraw {
    pub(crate) mesh: Mesh,
    /// How the draw draws.
    draw: Draw,
    group: Group,
}

impl SingleDraw {
    /// A draw of `mesh`, as `draw` draws, which draws once its blocks are
    /// written ([`write`](Self::write)).
    #[must_use]
    pub(crate) fn new(mesh: Mesh, draw: Draw) -> SingleDraw {
        SingleDraw { mesh, draw, group: Group::default() }
    }

    /// Makes `block` the value the draw reads of its program's uniform
    /// block of its layout, as [`Group::block`] does, from the next
    /// [`write`](Self::write).
    pub(crate) fn block<T: UniformBlock>(&mut self, block: &T) -> &mut Self {
        self.group.block(block);
        self
    }

    /// Writes the buffers the draw reads, with `blocks`, its values of its
    /// program's blocks of each draw, as [`Group::finish`] does.
    pub(crate) fn write<B: DrawBlocks>(&mut self, gpu: &mut dyn Gpu, context: &mut Context, blocks: B) {
        let SingleDraw { mesh, draw, group } = self;
        group.push(mesh, *draw, blocks);
        group.finish(gpu, context, None, []);
    }

    /// Uploads the mesh, if the draw's blocks were written.
    pub(crate) fn upload(&self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.group.upload(gpu, context);
    }

    /// Draws the draw, if it draws in the pass, into the open pass of `gpu`,
    /// the context's. Returns whether it didn't fail.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        self.group.render(gpu, parameters)
    }
}

/// What a frame's groups, or a drape's, need of the passes: their number,
/// which sets the passes' depth range; the masks of the tiles they clip to
/// and of the tiles that clear the stencil buffer; and the groups drawn as
/// a whole, each into the layer composite's texture and then onto the
/// frame.
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
    /// The groups that `visit_groups` visits, in drawing order, of passes
    /// whose stencil the tiles that `stencil_clear_tiles` gives for the
    /// tiles they clip to clear ([`PaintParameters::stencil_clear_tiles`]).
    /// Groups of one source clip to the same tiles, which are added once.
    pub(crate) fn of<'a>(
        visit_groups: impl FnOnce(&mut dyn FnMut(&'a Group)),
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
            groups.composited.extend(group.composited());
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

/// The order the frame draws the layers' groups in, each at the depth of
/// [`PaintParameters::current_layer`], which [`DrawOrder::step`] moves to
/// the next group's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawOrder {
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
