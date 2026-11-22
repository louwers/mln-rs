//! What a layer keeps of its drawing between frames, and the draws of a
//! frame.
//!
//! Each layer type keeps the GPU objects of its draws in the context's
//! store of meshes, through [`Mesh`] handles in types of its own: those of a
//! tile in its value of the tile, which the engine keeps in the layer's
//! [`TileMap`]. Its draw update makes the frame's draws, values
//! ([`FrameDraw`]) that name the mesh each draws, in a [`DrawList`] with
//! their uniform values, which it writes to the buffers of their
//! [`FrameDraws`]. The engine then uploads and draws the meshes the draws
//! name: [`FrameDraws::upload`] and [`FrameDraws::render`]. A layer type
//! that draws one group of its tiles' draws keeps them in a [`TileDraws`].

use mln_gfx::gpu::Gpu;
use mln_gfx::types::DepthMaskType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;

use crate::draw_objects::Mesh;
use crate::gfx::context::Context;
use crate::gfx::store::MeshKey;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::layer_composite::Composite;
use crate::paint_parameters::{Composited, Depth, PaintParameters, Stencil};
use crate::render_pass::RenderPass;
#[cfg(doc)]
use crate::tile_values::TileMap;

/// The order of a layer's draws that its meshes' order doesn't give: lower
/// priorities draw first.
pub type DrawPriority = i64;

/// How a draw tests and writes depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawDepth {
    /// It draws without a depth test.
    Off,
    /// It tests the depth of a sub-layer of the layer's depth, and writes it
    /// as `mask` says.
    Layer {
        /// The sub-layer, of the three of each layer's depth, the lowest
        /// first.
        sub_layer: u8,
        /// Whether the draw writes the depth it tests.
        mask: DepthMaskType,
    },
    /// It tests the depth range of 3D geometry, which the 3D draws of all
    /// layers share, and writes it as `mask` says.
    Volume {
        /// Whether the draw writes the depth it tests.
        mask: DepthMaskType,
    },
}

/// One draw of a frame: the mesh it draws and the state it draws it with.
/// Its uniform values are the entries at its index in the storage buffers of
/// its `FrameDraws`.
#[derive(Clone, Copy, Debug)]
pub struct FrameDraw {
    /// The tile whose draw it is, or `None` for a draw of the layer's.
    pub tile: Option<OverscaledTileID>,
    /// The mesh it draws.
    mesh: MeshKey,
    /// The mesh's [`Mesh::seq`], which orders draws of the same priority.
    seq: u64,
    /// Where the draw is in the layer's order, before its mesh orders it.
    pub priority: DrawPriority,
    /// The passes the draw draws in.
    pub pass: RenderPass,
    /// How the draw tests and writes depth.
    pub depth: DrawDepth,
    /// Whether the draw clips: a draw at the layer's depth to the mask of
    /// its tile, a 3D one to where none of the layer's 3D draws of the pass
    /// drew before it, so no pixel blends twice.
    pub clip: bool,
}

impl FrameDraw {
    /// A draw of `mesh`, of `tile` or of the layer for `None`, in the
    /// translucent pass at the layer's depth, which it reads, without
    /// clipping. The mesh stays the layer's: it must last until the frame
    /// draws.
    #[must_use]
    pub fn new(tile: Option<OverscaledTileID>, mesh: &Mesh) -> FrameDraw {
        FrameDraw {
            tile,
            mesh: mesh.key(),
            seq: mesh.seq(),
            priority: 0,
            pass: RenderPass::TRANSLUCENT,
            depth: DrawDepth::Layer { sub_layer: 0, mask: DepthMaskType::ReadOnly },
            clip: false,
        }
    }

    /// The order of the layer's draws: by priority, then by when their
    /// meshes were made.
    #[must_use]
    pub(crate) fn order(&self) -> (DrawPriority, u64) {
        (self.priority, self.seq)
    }
}

/// The draws an update makes for a frame, each with its uniform values of
/// type `U`, before they become the draws of a `FrameDraws`. The list keeps
/// its memory from frame to frame, so that a frame of no more draws than one
/// before it allocates nothing.
pub struct DrawList<U> {
    /// The draws, each with the index of its values in `uniforms`.
    draws: Vec<(FrameDraw, usize)>,
    uniforms: Vec<U>,
    /// The bytes of a storage buffer's entries, as [`UniformWriter`] writes
    /// them.
    bytes: Vec<u8>,
}

impl<U> Default for DrawList<U> {
    fn default() -> Self {
        DrawList { draws: Vec::new(), uniforms: Vec::new(), bytes: Vec::new() }
    }
}

impl<U> DrawList<U> {
    /// Drops the list's draws.
    pub fn clear(&mut self) {
        self.draws.clear();
        self.uniforms.clear();
    }

    /// Adds `draw`, with its uniform values.
    pub fn push(&mut self, draw: FrameDraw, uniforms: U) {
        self.draws.push((draw, self.uniforms.len()));
        self.uniforms.push(uniforms);
    }

    /// Makes the list's draws `frame`'s, in order of their priority, then of
    /// their meshes' `seq`, then of when they were pushed, and empties the
    /// list. Where there are any, `write` first writes the buffers they read
    /// to `frame`'s. A draw binds a buffer that a write replaced again when
    /// it draws.
    pub fn finish(
        &mut self,
        frame: &mut FrameDraws,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        write: impl FnOnce(&mut UniformWriter<'_, U>),
    ) {
        sort_draws(&mut self.draws);
        if !self.draws.is_empty() {
            write(&mut UniformWriter {
                draws: &self.draws,
                uniforms: &self.uniforms,
                bytes: &mut self.bytes,
                buffers: &mut frame.uniform_buffers,
                gpu,
                context,
            });
        }
        take_draws(&mut self.draws, &mut frame.draws);
        self.uniforms.clear();
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

/// Writes the buffers that a frame's draws read, each draw's entries at its
/// index in drawing order.
pub struct UniformWriter<'a, U> {
    draws: &'a [(FrameDraw, usize)],
    uniforms: &'a [U],
    bytes: &'a mut Vec<u8>,
    buffers: &'a mut UniformBufferArray,
    gpu: &'a mut dyn Gpu,
    context: &'a mut Context,
}

impl<U> UniformWriter<'_, U> {
    /// Writes each draw's `entry` of its uniform values to the storage
    /// buffer `id`.
    pub fn storage<P: bytemuck::Pod>(&mut self, id: usize, entry: impl Fn(&U) -> P) {
        self.bytes.clear();
        for &(_, index) in self.draws {
            self.bytes.extend_from_slice(bytemuck::bytes_of(&entry(&self.uniforms[index])));
        }
        let _ = self.buffers.write_storage(self.gpu, id, self.bytes, self.context.device());
    }

    /// Writes `value`, which all the draws read, to the uniform buffer `id`.
    pub fn uniform<P: bytemuck::Pod>(&mut self, id: usize, value: &P) {
        let _ = self.buffers.write_pod(self.gpu, id, value, self.context.device());
    }
}

/// Draws of a frame, and what they share.
pub struct FrameDraws {
    /// Tells the draws apart in the layer composite.
    id: SimpleIdentity,
    /// The draws, in [`FrameDraw::order`]. A draw's index is the index of its
    /// entries in the storage buffers, which shaders read as `drawIndex`.
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
    /// Writes `value` to the uniform buffer `id` that the draws bind. A draw
    /// binds a buffer that the write replaced again when it draws.
    pub fn write_pod<T: bytemuck::Pod>(&mut self, gpu: &mut dyn Gpu, id: usize, value: &T, context: &mut Context) {
        let _ = self.uniform_buffers.write_pod(gpu, id, value, context.device());
    }

    /// Gives the draws, which draw without a projection, a projection block
    /// of zeros if they have none. GL's programs declare the block whether
    /// they read it or not, so a draw binds it all the same.
    pub fn bind_no_projection(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        crate::tile_matrix::bind_no_projection(gpu, &mut self.uniform_buffers, context);
    }

    /// Adds `draw`, which draws after the others.
    pub fn push(&mut self, draw: FrameDraw) {
        self.draws.push(draw);
    }

    /// Drops the draws.
    pub fn clear(&mut self) {
        self.draws.clear();
    }

    /// Makes the draws composite onto the frame as `composite` says, or
    /// draw straight onto it for `None`.
    pub fn set_composite(&mut self, composite: Option<Composite>) {
        self.composite = composite;
    }

    /// Makes `tiles` those whose masks the stencil buffer holds when the
    /// draws draw.
    pub fn set_stencil_tiles(&mut self, tiles: impl IntoIterator<Item = UnwrappedTileID>) {
        self.stencil_tiles.clear();
        self.stencil_tiles.extend(tiles);
    }

    /// How the draws composite onto the frame, with the id that tells them
    /// apart in the layer composite, if there are any and they composite.
    #[must_use]
    pub(crate) fn composited(&self) -> Option<(SimpleIdentity, Composite)> {
        self.composite.filter(|_| !self.draws.is_empty()).map(|composite| (self.id, composite))
    }

    /// Uploads the meshes of the draws, in drawing order.
    pub fn upload(&self, gpu: &mut dyn Gpu, context: &mut Context) {
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
    pub fn render(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
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
    /// tiles that those that clip clip to; 3D draws that clip share one
    /// stencil value of their own. `None` where there is nothing to draw
    /// into or there are no draws; [`DrawPass::end`] ends what this begins.
    pub(crate) fn begin(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<DrawPass<'_>> {
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
        let (volume, volume_clips) =
            pass.draws().fold((false, false), |(volume, clips), PassDraw { draw, .. }| match draw.depth {
                DrawDepth::Volume { .. } => (true, clips || draw.clip),
                _ => (volume, clips),
            });
        if volume {
            pass.volume_stencil = volume_clips.then(|| parameters.stencil_for_3d(gpu));
        } else if drawing && !self.stencil_tiles.is_empty() {
            parameters.render_tile_clipping_masks(gpu, &self.stencil_tiles);
        }
        Some(pass)
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
    /// The passes of the draws that draw in another pass: both the opaque
    /// and the translucent, or none.
    skip: RenderPass,
    /// The stencil of the 3D draws that clip.
    volume_stencil: Option<Stencil>,
    composited: Option<Composited>,
}

/// A draw of a [`DrawPass`], as [`DrawPass::draws`] yields it.
#[derive(Clone, Copy)]
pub(crate) struct PassDraw<'a> {
    /// The draw's index among the frame's draws.
    index: usize,
    draw: &'a FrameDraw,
}

impl<'a> DrawPass<'a> {
    /// The draws of the pass, in drawing order.
    pub(crate) fn draws(&self) -> impl Iterator<Item = PassDraw<'a>> + use<'a> {
        let (pass, skip) = (self.pass, self.skip);
        let draws = self.draws.iter().enumerate();
        draws
            .filter(move |(_, draw)| draw.pass & pass != RenderPass::NONE)
            .filter(move |(_, draw)| skip == RenderPass::NONE || draw.pass & skip != skip)
            .map(|(index, draw)| PassDraw { index, draw })
    }

    /// Draws `draw`, one of the pass's, with the GPU objects of its mesh.
    pub(crate) fn draw(&self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, draw: PassDraw<'_>) {
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
            DrawDepth::Off => (Depth::DISABLED, tile_stencil()),
            DrawDepth::Layer { sub_layer, mask } => (parameters.depth_for_sublayer(sub_layer, mask), tile_stencil()),
            DrawDepth::Volume { mask } => {
                (parameters.depth_for_3d(mask), self.volume_stencil.filter(|_| draw.clip).unwrap_or(Stencil::DISABLED))
            }
        };
        objects.draw(gpu, parameters, &self.frame.uniform_buffers, depth, stencil, index as u32);
        parameters.context.restore_mesh(draw.mesh, objects);
    }

    /// Ends the pass's draws: composites what they drew where they drew
    /// into the layer composite's texture.
    pub(crate) fn end(self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(composited) = self.composited {
            parameters.end_composited(gpu, composited);
        }
    }
}

/// One mesh and a frame's draw of it, with the buffers it binds.
pub struct SingleDraw {
    /// Keeps the mesh the draw draws.
    _mesh: Mesh,
    pub(crate) frame: FrameDraws,
}

impl SingleDraw {
    /// One draw of `mesh`, of the layer, in the translucent pass at `depth`,
    /// without buffers.
    #[must_use]
    pub fn new(mesh: Mesh, depth: DrawDepth) -> SingleDraw {
        let mut frame = FrameDraws::default();
        let mut draw = FrameDraw::new(None, &mesh);
        draw.depth = depth;
        frame.push(draw);
        SingleDraw { _mesh: mesh, frame }
    }

    /// Uploads the draw's mesh.
    pub fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.frame.upload(gpu, context);
    }

    /// Binds a projection block that projects nothing, as
    /// [`FrameDraws::bind_no_projection`] does.
    pub fn bind_no_projection(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.frame.bind_no_projection(gpu, context);
    }

    /// Writes `value` to the frame's uniform buffer `id`, as
    /// [`FrameDraws::write_pod`] does.
    pub fn write_pod<T: bytemuck::Pod>(&mut self, gpu: &mut dyn Gpu, id: usize, value: &T, context: &mut Context) {
        self.frame.write_pod(gpu, id, value, context);
    }

    /// Draws the draw, if it draws in the pass, into the open pass of `gpu`,
    /// the context's.
    pub fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render(gpu, parameters);
    }
}

/// What the frame draws of a layer in one place in the layers' order and
/// depth.
#[derive(Clone, Copy)]
pub enum DrawGroup<'a> {
    /// Draws of the layer's meshes.
    Draws(&'a FrameDraws),
    /// What the layer draws with objects of its own, in its drawing's
    /// [`render`](LayerDrawing::render), such as what a custom layer's host
    /// draws.
    Own,
}

impl<'a> DrawGroup<'a> {
    /// The tiles whose masks the stencil buffer holds when the group draws;
    /// `None` for a group that needs no masks.
    #[must_use]
    pub(crate) fn stencil_tiles(self) -> Option<&'a [UnwrappedTileID]> {
        match self {
            DrawGroup::Draws(frame) => Some(&frame.stencil_tiles[..]).filter(|tiles| !tiles.is_empty()),
            DrawGroup::Own => None,
        }
    }

    /// How the group composites onto the frame, with the id that tells it
    /// apart in the layer composite, if it draws anything and composites.
    #[must_use]
    pub(crate) fn composite(self) -> Option<(SimpleIdentity, Composite)> {
        match self {
            DrawGroup::Draws(frame) => frame.composited(),
            DrawGroup::Own => None,
        }
    }
}

/// The order the frame draws the layers' draw groups in, each at a depth
/// of its own, which [`DrawOrder::step`] moves the encode to after the
/// group draws.
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
    /// Moves the encode `parameters` to the depth of the group after the one
    /// it is at, as [`LayerDrawing::render`] does after each of its groups.
    pub fn step(self, parameters: &mut PaintParameters<'_>) {
        match self {
            DrawOrder::BottomUp => parameters.current_layer = parameters.current_layer.saturating_sub(1),
            DrawOrder::TopDown => parameters.current_layer += 1,
        }
    }
}

/// The drawing of a layer that draws one group of draws, whose meshes its
/// values of its tiles keep.
#[derive(Default)]
pub struct TileDraws {
    /// The frame's draws, from the layer's first update with something to
    /// draw.
    frame: Option<FrameDraws>,
}

impl TileDraws {
    /// The frame's draws, which the layer has from the first call on: their
    /// group then takes a place in the layers' order and depth.
    pub fn frame_mut(&mut self) -> &mut FrameDraws {
        self.frame.get_or_insert_default()
    }

    /// Makes the draws of `list` the frame's, as [`DrawList::finish`] does.
    pub fn finish<U>(
        &mut self,
        list: &mut DrawList<U>,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        write: impl FnOnce(&mut UniformWriter<'_, U>),
    ) {
        list.finish(self.frame_mut(), gpu, context, write);
    }
}

impl LayerDrawing for TileDraws {
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if let Some(frame) = &self.frame {
            visit(DrawGroup::Draws(frame));
        }
    }

    fn clear(&mut self) {
        self.clear_frame();
    }

    fn begin_frame(&mut self) {
        self.frame_mut().clear();
    }

    fn clear_frame(&mut self) {
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if let Some(frame) = &self.frame {
            frame.upload(gpu, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(frame) = &self.frame {
            frame.render(gpu, parameters);
            order.step(parameters);
        }
    }
}

/// What a layer type keeps of its drawing besides its values of its tiles,
/// which the engine keeps: the meshes of its other draws, in types of its
/// own, and its draw groups of the frame.
pub trait LayerDrawing {
    /// Visits the layer's draw groups of the frame, in drawing order: from
    /// its first update with something to draw, a group of its draws, and
    /// any of draws over them. While the layer is renderable, a group takes
    /// a place in the layers' order and depth, even when it has no draws.
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>));

    /// Drops the frame's draws, and the meshes of the draws that the layer
    /// keeps besides its values of its tiles, which the engine drops with
    /// them.
    fn clear(&mut self);

    /// Starts the layer's draws of a frame it has something to draw in, in
    /// the encode, before the layer makes them: empty, keeping the meshes of
    /// the tiles' draws. A drawing whose draw group takes its place from the
    /// layer's first such frame starts the group here.
    fn begin_frame(&mut self);

    /// Drops the frame's draws, keeping the meshes of the tiles' draws, for
    /// a frame the layer draws nothing in.
    fn clear_frame(&mut self);

    /// Uploads the meshes of the draw groups' draws.
    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context);

    /// Draws the draw groups' draws of the pass into the open pass of `gpu`,
    /// the context's, in `order`, each at its depth. After each group that
    /// [`visit_groups`](LayerDrawing::visit_groups) visits, drawn in the pass
    /// or not, it steps the depth once ([`DrawOrder::step`]), so that the
    /// groups after it draw at theirs.
    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder);
}
