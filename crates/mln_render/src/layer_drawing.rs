//! What a layer that makes its draws itself keeps of its drawing between
//! frames.
//!
//! Such a layer type keeps the GPU objects of its draws in the context's
//! store of meshes, through [`Mesh`] handles in types of its own: those of a
//! tile in its value of the tile, which the engine keeps in the layer's
//! [`TileMap`]. Its draw update makes the frame's draws, values
//! ([`FrameDraw`]) that name the mesh each draws, in a [`DrawList`] with
//! their uniform values, which it writes to the buffers of their
//! [`FrameDraws`]. The engine then uploads and draws the meshes the draws
//! name. A layer type that draws one group of its tiles' draws keeps them in
//! a [`TileDraws`]. A layer type whose drawing is `()` pushes its draws to
//! the engine's [`Draws`](crate::draw::Draws) instead.

use mln_gfx::gpu::Gpu;
use mln_tile_id::UnwrappedTileID;
use mln_util::identity::SimpleIdentity;

use crate::draw_objects::Mesh;
use crate::encode::{Depth, FrameDraw, FrameDraws};
use crate::gfx::context::Context;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::layer_composite::Composite;
use crate::paint_parameters::PaintParameters;
#[cfg(doc)]
use crate::tile_values::TileMap;

/// The order of a layer's draws that its meshes' order doesn't give: lower
/// priorities draw first.
pub type DrawPriority = i64;

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
    pub fn new(mesh: Mesh, depth: Depth) -> SingleDraw {
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

/// The drawing of a layer type that pushes its draws to the engine's
/// [`Draws`](crate::draw::Draws) ([`DrawContext::draws`](crate::layer::DrawContext::draws)),
/// which keeps nothing.
impl LayerDrawing for () {
    fn visit_groups<'a>(&'a self, _visit: &mut dyn FnMut(DrawGroup<'a>)) {}

    fn clear(&mut self) {}

    fn begin_frame(&mut self) {}

    fn clear_frame(&mut self) {}

    fn upload(&mut self, _gpu: &mut dyn Gpu, _context: &mut Context) {}

    fn render(&mut self, _gpu: &mut dyn Gpu, _parameters: &mut PaintParameters<'_>, _order: DrawOrder) {}
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
