//! What a layer keeps of its drawing between frames, and the draws of a
//! frame.
//!
//! Each layer type keeps the GPU objects of its draws in types of its own,
//! those of a tile in its value of the tile, which the engine keeps in the
//! layer's [`TileMap`]. Its draw update makes the frame's draws, values
//! ([`FrameDraw`]) in a [`DrawList`] with their uniform values and the part
//! of the layer's GPU objects each binds, which it writes to the buffers of
//! their [`LayerDraws`]. The layer type then uploads and draws them in
//! loops of its own: [`FrameDraws::begin`] begins a pass's draws, and
//! [`DrawPass::draw`] draws one draw's objects. A layer type that draws one
//! group of its tiles' draws keeps them in a [`TileDraws`], whose tiles name
//! the objects of their draws by a part of their own type
//! ([`TileObjects`]).

use mln_gfx::gpu::Gpu;
use mln_gfx::types::DepthMaskType;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::identity::SimpleIdentity;

use crate::draw_objects::DrawObjects;
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::layer_composite::Composite;
use crate::paint_parameters::{Composited, Depth, PaintParameters, Stencil};
use crate::render_pass::RenderPass;
use crate::tile_values::TileMap;

/// The order of a layer's draws that its draws' ids don't give: lower
/// priorities draw first.
pub type DrawPriority = i64;

/// The GPU objects of one draw a layer keeps, with the id that orders the
/// draws by when their objects were made.
pub struct KeptDraw {
    pub(crate) id: SimpleIdentity,
    /// The GPU objects.
    pub objects: DrawObjects,
}

impl KeptDraw {
    /// The draw of `objects`, ordered after the draws made before it.
    #[must_use]
    pub fn new(objects: DrawObjects) -> KeptDraw {
        KeptDraw { id: SimpleIdentity::new(), objects }
    }

    /// A draw of the objects, which `tile` holds, or the layer for `None`
    /// (see [`FrameDraw::new`]).
    #[must_use]
    pub fn frame_draw(&self, tile: Option<OverscaledTileID>) -> FrameDraw {
        FrameDraw::new(tile, self.id)
    }
}

/// The GPU objects of a draw, with what gives them the textures of their
/// tile's atlases where they sample them.
pub struct DrawObjectsMut<'a> {
    /// The GPU objects.
    pub objects: &'a mut DrawObjects,
    /// What gives them their tile's atlases' textures, `None` for objects
    /// that sample none.
    pub atlases: Option<&'a AtlasBinding>,
}

impl<'a> From<&'a mut DrawObjects> for DrawObjectsMut<'a> {
    /// GPU objects that sample no atlases.
    fn from(objects: &'a mut DrawObjects) -> Self {
        DrawObjectsMut { objects, atlases: None }
    }
}

/// What a layer keeps of a tile, whose draws a frame's draws name by a part
/// of the layer's own type.
pub trait TileObjects {
    /// Which of the tile's draws a frame's draw draws.
    type Part: Copy;

    /// The GPU objects of the tile's draw `part`.
    fn objects(&mut self, part: Self::Part) -> Option<DrawObjectsMut<'_>>;

    /// Drops the bindings of the tile's draws ([`DrawObjects::drop_bindings`]).
    fn drop_bindings(&mut self);
}

impl TileObjects for KeptDraw {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<DrawObjectsMut<'_>> {
        Some((&mut self.objects).into())
    }

    fn drop_bindings(&mut self) {
        self.objects.drop_bindings();
    }
}

/// Makes the draws of `list` those of `layer`, as `DrawList::finish`
/// does, and drops the bindings of the draws of `tiles` where that replaced
/// a buffer they bind.
pub fn finish_tiles<U, T: TileObjects>(
    list: &mut DrawList<U, T::Part>,
    layer: &mut LayerDraws<T::Part>,
    tiles: &mut TileMap<T>,
    gpu: &mut dyn Gpu,
    context: &mut Context,
    write: impl FnOnce(&mut UniformWriter<'_, U>),
) {
    if list.finish(layer, gpu, context, write) {
        tiles.values_mut().for_each(T::drop_bindings);
    }
}

/// The GPU objects of `draw`, `part` of its tile's, as
/// [`LayerDraws::upload`] and [`LayerDraws::render`] find them.
pub fn tile_objects<'a, T: TileObjects>(
    tiles: &'a mut TileMap<T>,
    draw: &FrameDraw,
    part: T::Part,
) -> Option<DrawObjectsMut<'a>> {
    tiles.find_mut(&draw.tile?)?.objects(part)
}

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

/// One draw of a frame: the state it draws its GPU objects with. Its uniform
/// values are the entries at its index in the storage buffers of its
/// `FrameDraws`.
pub struct FrameDraw {
    /// The tile whose draw it is, or `None` for a draw of the layer's.
    pub tile: Option<OverscaledTileID>,
    /// The id of the draw's GPU objects, which orders the draws by when
    /// their objects were made.
    pub id: SimpleIdentity,
    /// Where the draw is in the layer's order, before the id orders it.
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
    /// A draw of the GPU objects `id`, of `tile` or of the layer for `None`,
    /// in the translucent pass at the layer's depth, which it reads, without
    /// clipping.
    #[must_use]
    pub fn new(tile: Option<OverscaledTileID>, id: SimpleIdentity) -> FrameDraw {
        FrameDraw {
            tile,
            id,
            priority: 0,
            pass: RenderPass::TRANSLUCENT,
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
/// before they become the draws of a `LayerDraws`. The list keeps its
/// memory from frame to frame, so that a frame of no more draws than one
/// before it allocates nothing.
pub struct DrawList<U, P = ()> {
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
    /// Adds `draw`, of a layer whose draws have no parts.
    pub fn push(&mut self, draw: FrameDraw, uniforms: U) {
        self.push_part(draw, (), uniforms);
    }
}

impl<U, P: Copy> DrawList<U, P> {
    /// Drops the list's draws.
    pub fn clear(&mut self) {
        self.draws.clear();
        self.uniforms.clear();
        self.parts.clear();
    }

    /// Adds `draw`, of the GPU objects of the layer's `part`.
    pub fn push_part(&mut self, draw: FrameDraw, part: P, uniforms: U) {
        self.draws.push((draw, self.uniforms.len()));
        self.uniforms.push(uniforms);
        self.parts.push(part);
    }

    /// Makes the list's draws `layer`'s, in order of their priority, then of
    /// their GPU objects' ids, then of when they were pushed, and empties the
    /// list. Where there are any, `write` first writes the buffers they read
    /// to `layer`'s. Returns whether that replaced one of `layer`'s buffers,
    /// whose owner then drops the bindings of the draws that bind them
    /// ([`DrawObjects::drop_bindings`]).
    #[must_use]
    pub fn finish(
        &mut self,
        layer: &mut LayerDraws<P>,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        write: impl FnOnce(&mut UniformWriter<'_, U>),
    ) -> bool {
        sort_draws(&mut self.draws);
        let mut writer = UniformWriter {
            draws: &self.draws,
            uniforms: &self.uniforms,
            bytes: &mut self.bytes,
            buffers: &mut layer.frame.uniform_buffers,
            gpu,
            context,
            replaced: false,
        };
        if !self.draws.is_empty() {
            write(&mut writer);
        }
        let replaced = writer.replaced;
        layer.parts.clear();
        layer.parts.extend(self.draws.iter().map(|&(_, index)| self.parts[index]));
        take_draws(&mut self.draws, &mut layer.frame.draws);
        self.uniforms.clear();
        self.parts.clear();
        replaced
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
    /// Whether a write replaced a buffer.
    replaced: bool,
}

impl<U> UniformWriter<'_, U> {
    /// Writes each draw's `entry` of its uniform values to the storage
    /// buffer `id`.
    pub fn storage<P: bytemuck::Pod>(&mut self, id: usize, entry: impl Fn(&U) -> P) {
        self.bytes.clear();
        for &(_, index) in self.draws {
            self.bytes.extend_from_slice(bytemuck::bytes_of(&entry(&self.uniforms[index])));
        }
        self.replaced |= self.buffers.write_storage(self.gpu, id, self.bytes, self.context.device());
    }

    /// Writes `value`, which all the draws read, to the uniform buffer `id`.
    pub fn uniform<P: bytemuck::Pod>(&mut self, id: usize, value: &P) {
        self.replaced |= self.buffers.write_pod(self.gpu, id, value, self.context.device());
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
    /// Writes `value` to the uniform buffer `id` that the draws bind.
    /// Whether that replaced the buffer, which drops the bindings of the
    /// draws that bound it.
    pub fn write_pod<T: bytemuck::Pod>(
        &mut self,
        gpu: &mut dyn Gpu,
        id: usize,
        value: &T,
        context: &mut Context,
    ) -> bool {
        self.uniform_buffers.write_pod(gpu, id, value, context.device())
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

    /// Uploads `objects`, which all the draws bind, if there are any draws.
    pub fn upload_one(&self, gpu: &mut dyn Gpu, objects: &mut DrawObjects, context: &mut Context) {
        if !self.draws.is_empty() {
            objects.upload(gpu, context);
        }
    }

    /// Draws the pass's draws, all of which bind `objects`, into the open
    /// pass of `gpu`, the context's.
    pub fn render_one(&self, objects: &mut DrawObjects, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(pass) = self.begin(gpu, parameters) else {
            return;
        };
        for draw in pass.draws() {
            pass.draw(gpu, parameters, objects.into(), draw);
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
            .filter(move |(_, draw)| draw.pass & pass != RenderPass::NONE)
            .filter(move |(_, draw)| skip == RenderPass::NONE || draw.pass & skip != skip)
            .map(|(index, draw)| PassDraw { index, draw })
    }

    /// Draws `objects`, the GPU objects of `draw`, one of the pass's.
    pub(crate) fn draw(
        &self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        objects: DrawObjectsMut<'_>,
        draw: PassDraw<'_>,
    ) {
        let DrawObjectsMut { objects, atlases } = objects;
        let PassDraw { index, draw } = draw;
        if let Some(atlases) = atlases {
            atlases.bind(objects, parameters);
        }
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
pub struct LayerDraws<P = ()> {
    frame: FrameDraws,
    /// Each draw's part, at the draw's index.
    parts: Vec<P>,
}

impl<P> Default for LayerDraws<P> {
    fn default() -> Self {
        LayerDraws { frame: FrameDraws::default(), parts: Vec::new() }
    }
}

impl<P: Copy> LayerDraws<P> {
    /// The draws, with what they share.
    #[must_use]
    pub fn frame(&self) -> &FrameDraws {
        &self.frame
    }

    /// Makes the draws composite onto the frame as `composite` says, or draw
    /// straight onto it for `None`.
    pub fn set_composite(&mut self, composite: Option<Composite>) {
        self.frame.set_composite(composite);
    }

    /// Makes `tiles` those whose masks the stencil buffer holds when the
    /// draws draw.
    pub fn set_stencil_tiles(&mut self, tiles: impl IntoIterator<Item = UnwrappedTileID>) {
        self.frame.set_stencil_tiles(tiles);
    }

    /// Drops the draws.
    pub fn clear(&mut self) {
        self.frame.draws.clear();
        self.parts.clear();
    }

    /// Uploads the GPU objects of the draws, in drawing order, which
    /// `objects` finds in `caches`.
    pub fn upload<C: ?Sized>(
        &self,
        gpu: &mut dyn Gpu,
        caches: &mut C,
        objects: impl for<'c> Fn(&'c mut C, &FrameDraw, P) -> Option<DrawObjectsMut<'c>>,
        context: &mut Context,
    ) {
        for (draw, &part) in self.frame.draws.iter().zip(&self.parts) {
            if let Some(objects) = objects(caches, draw, part) {
                objects.objects.upload(gpu, context);
            }
        }
    }

    /// Draws the pass's draws, whose GPU objects `objects` finds in
    /// `caches`, into the open pass of `gpu`, the context's.
    pub fn render<C: ?Sized>(
        &self,
        caches: &mut C,
        objects: impl for<'c> Fn(&'c mut C, &FrameDraw, P) -> Option<DrawObjectsMut<'c>>,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
    ) {
        let Some(pass) = self.frame.begin(gpu, parameters) else {
            return;
        };
        for draw in pass.draws() {
            match objects(caches, draw.draw(), self.parts[draw.index]) {
                Some(objects) => pass.draw(gpu, parameters, objects, draw),
                None => debug_assert!(false, "a draw of tile {:?} has no GPU objects", draw.draw().tile),
            }
        }
        pass.end(gpu, parameters);
    }
}

/// One draw's GPU objects and a frame's draws of them, with the buffers they
/// bind.
pub struct SingleDraw {
    pub(crate) objects: DrawObjects,
    pub(crate) frame: FrameDraws,
}

impl SingleDraw {
    /// One draw of `objects`, as `draw` draws, without buffers.
    #[must_use]
    pub fn new(objects: DrawObjects, draw: FrameDraw) -> SingleDraw {
        let mut frame = FrameDraws::default();
        frame.push(draw);
        SingleDraw { objects, frame }
    }

    /// Uploads the draw's GPU objects, if the frame draws it.
    pub fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.frame.upload_one(gpu, &mut self.objects, context);
    }

    /// Binds a projection block that projects nothing, as
    /// [`FrameDraws::bind_no_projection`] does.
    pub fn bind_no_projection(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.frame.bind_no_projection(gpu, context);
    }

    /// Writes `value` to the frame's uniform buffer `id`, as
    /// [`FrameDraws::write_pod`] does, dropping the draw's bindings where that
    /// replaced the buffer.
    pub fn write_pod<T: bytemuck::Pod>(&mut self, gpu: &mut dyn Gpu, id: usize, value: &T, context: &mut Context) {
        if self.frame.write_pod(gpu, id, value, context) {
            self.objects.drop_bindings();
        }
    }

    /// Draws the draw, if it draws in the pass, into the open pass of `gpu`,
    /// the context's.
    pub fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        self.frame.render_one(&mut self.objects, gpu, parameters);
    }
}

/// What the frame draws of a layer in one place in the layers' order and
/// depth.
#[derive(Clone, Copy)]
pub enum DrawGroup<'a> {
    /// Draws of the layer's GPU objects.
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

/// The drawing of a layer that draws one group of draws, each of a part of
/// the tile it draws, whose value of a tile is a `T`.
pub struct TileDraws<T: TileObjects> {
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
    pub fn frame_mut(&mut self) -> &mut LayerDraws<T::Part> {
        self.frame.get_or_insert_default()
    }

    /// Makes the draws of `list` the frame's, as [`DrawList::finish`] does,
    /// and drops the bindings of the draws of `tiles`, the layer's values of
    /// its tiles, where that replaced a buffer they bind.
    pub fn finish<U>(
        &mut self,
        list: &mut DrawList<U, T::Part>,
        tiles: &mut TileMap<T>,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        write: impl FnOnce(&mut UniformWriter<'_, U>),
    ) {
        finish_tiles(list, self.frame.get_or_insert_default(), tiles, gpu, context, write);
    }
}

impl<T: TileObjects> LayerDrawing<T> for TileDraws<T> {
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if let Some(frame) = &self.frame {
            visit(DrawGroup::Draws(&frame.frame));
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
        if let Some(frame) = &self.frame {
            frame.render(tiles, tile_objects, gpu, parameters);
            order.step(parameters);
        }
    }
}

/// What a layer type keeps of its drawing besides its values of its tiles,
/// each a `Tile`, which the engine keeps: the GPU objects of its other
/// draws, in types of its own, and its draw groups of the frame, whose
/// draws it uploads and draws in loops of its own.
pub trait LayerDrawing<Tile> {
    /// Visits the layer's draw groups of the frame, in drawing order: from
    /// its first update with something to draw, a group of its draws, and
    /// any of draws over them. While the layer is renderable, a group takes
    /// a place in the layers' order and depth, even when it has no draws.
    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>));

    /// Drops the frame's draws, and the GPU objects of the draws that the
    /// layer keeps besides its values of its tiles, which the engine drops
    /// with them.
    fn clear(&mut self);

    /// Starts the layer's draws of a frame it has something to draw in, in
    /// the encode, before the layer makes them: empty, keeping the GPU
    /// objects of the tiles' draws. A drawing whose draw group takes its
    /// place from the layer's first such frame starts the group here.
    fn begin_frame(&mut self);

    /// Drops the frame's draws, keeping the GPU objects of the tiles' draws,
    /// for a frame the layer draws nothing in.
    fn clear_frame(&mut self);

    /// Uploads the GPU objects of the draw groups' draws, those of its
    /// tiles' draws in `tiles`.
    fn upload(&mut self, tiles: &mut TileMap<Tile>, gpu: &mut dyn Gpu, context: &mut Context);

    /// Draws the draw groups' draws of the pass, those of its tiles' draws
    /// with the GPU objects in `tiles`, into the open pass of `gpu`, the
    /// context's, in `order`, each at its depth. After each group that
    /// [`visit_groups`](LayerDrawing::visit_groups) visits, drawn in the pass
    /// or not, it steps the depth once ([`DrawOrder::step`]), so that the
    /// groups after it draw at theirs.
    fn render(
        &mut self,
        tiles: &mut TileMap<Tile>,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
    );
}
