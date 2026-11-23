//! The GPU objects of a draw, which the renderer keeps between frames and
//! makes again only when what they are made of changes.
//!
//! The context keeps each draw's objects in its store of meshes, which their
//! maker holds through a [`Mesh`] handle and a frame's draws name. A draw
//! owns what is its own, its own indexes and attribute data, and holds what
//! it shares with other draws, its textures and the indexes and vertices of
//! buckets and static geometry, through handles of the context's stores
//! (`gfx::store`), which keep them while the draw lasts.

use std::marker::PhantomData;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::gfx_types::DrawModeType;
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferId, DepthState, Draw, DynamicState, FixedState, GeometryDesc,
    GeometryId, Gpu, GpuError, PipelineId, PipelineKey, SamplerId, Scissor, ShaderId, ShaderKey, StencilState,
    TargetFormats, TextureId, VertexAttribute as GpuVertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::{FIRST_VERTEX_BUFFER_ID, MAX_TEXTURES};
use mln_gfx::shaders::block::{DrawBlocks, MemberType};
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::segment::SegmentBase;
use mln_util::logging::{Event, Log};
use smallvec::SmallVec;

use crate::gfx::buffer_store::{SharedBuffer, StoredBuffer};
use crate::gfx::context::Resources;
use crate::gfx::owned::{Held, Owned};
use crate::gfx::resources::Device;
use crate::gfx::store::{MeshKey, Shared, SharedTexture};
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::gfx::upload::{self, AttributeBindingArray};
use crate::gfx::vertex_attribute::{Uniforms, VertexAttributeArray};
use crate::handles::Sampler;
use crate::paint_parameters::{DepthTest, DrawTarget, PaintParameters, Stencil};
use crate::paint_property_binder::MeshPaint;

/// The textures a draw binds, by texture id, each with the sampler it is
/// sampled with.
pub(crate) type Textures = [Option<(SharedTexture, Sampler)>; MAX_TEXTURES];

/// A handle of a draw's GPU objects in the context's store of meshes
/// ([`Resources::mesh`](crate::draw::Resources::mesh)). The objects stay while the handle does, and go
/// once it drops and the GPU is done with them. A draw names the objects
/// it draws by their mesh ([`Group::push`](crate::draw::Group::push)), with
/// `B`, the values of the draw's own for its program's storage buffers of an
/// element per draw ([`DrawBlocks`](mln_gfx::shaders::block::DrawBlocks)).
pub struct Mesh<B = ()> {
    handle: Shared<MeshKey>,
    seq: u64,
    program: &'static Program,
    /// What the engine writes of the mesh's paint values into its draws'
    /// blocks.
    paint: Option<MeshPaint>,
    /// Where the members `matrix` of the draws' blocks are in their bytes.
    matrices: SmallVec<[usize; 1]>,
    blocks: PhantomData<fn(B)>,
}

impl<B: DrawBlocks> Mesh<B> {
    pub(crate) fn new(
        handle: Shared<MeshKey>,
        seq: u64,
        program: &'static Program,
        paint: Option<MeshPaint>,
    ) -> Mesh<B> {
        let mut matrices = SmallVec::new();
        let mut start = 0;
        for block in B::BLOCKS {
            let matrix = block.members.iter().find(|member| member.name == "matrix" && member.ty == MemberType::Mat4);
            matrices.extend(matrix.map(|member| start + member.offset));
            start += block.size;
        }
        Mesh { handle, seq, program, paint, matrices, blocks: PhantomData }
    }
}

impl<B> Mesh<B> {
    /// When the mesh was made among its renderer's meshes: a mesh made
    /// later has a greater one. A draw of the mesh that orders by it
    /// ([`Draw::order`](crate::draw::Draw::order)) draws over the draws of
    /// the meshes made before it.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// What the engine writes of the mesh's paint values into its draws'
    /// blocks.
    pub(crate) fn paint(&self) -> Option<&MeshPaint> {
        self.paint.as_ref()
    }

    /// Where the members `matrix` of the draws' blocks are in their bytes.
    pub(crate) fn matrices(&self) -> &[usize] {
        &self.matrices
    }

    pub(crate) fn key(&self) -> MeshKey {
        self.handle.key()
    }

    /// The program the mesh draws with.
    pub(crate) fn program(&self) -> &'static Program {
        self.program
    }
}

impl<B> std::fmt::Debug for Mesh<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mesh")
            .field("key", &self.handle.key())
            .field("seq", &self.seq)
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

/// The most vertex buffers a draw binds, WebGPU's default limit.
const MAX_VERTEX_BUFFERS: usize = 8;

/// What a draw draws: its vertices, and the primitives its segments draw of
/// them.
pub(crate) struct DrawGeometry {
    /// The vertex attributes, by attribute index.
    pub vertices: VertexAttributeArray,
    /// How many vertices the attributes hold, which an attribute the
    /// shader reads and the draw lacks gets default values for.
    pub vertex_count: usize,
    /// The primitive the segments draw.
    pub mode: DrawMode,
    /// The `u16` indexes of the primitives' vertices, a buffer of the
    /// context's store.
    pub indexes: SharedBuffer,
    /// The ranges of the indexes and vertices each draw call draws.
    pub segments: Vec<SegmentBase>,
    /// The attributes of each instance, for a draw of instances, as many
    /// as the fewest any of them holds.
    pub instances: Option<VertexAttributeArray>,
}

/// The most entries bindings have.
const MAX_ENTRIES: usize = FIRST_VERTEX_BUFFER_ID + MAX_PULLED + MAX_TEXTURES;

/// The entries of bindings, on the stack: at most a buffer at each uniform
/// buffer id and each vertex buffer id, and a texture at each texture id.
pub(crate) struct Entries {
    entries: [BindGroupEntry; MAX_ENTRIES],
    len: usize,
}

impl Entries {
    pub(crate) fn new() -> Entries {
        let none = BindGroupEntry::Texture { id: 0, texture: TextureId::default(), sampler: SamplerId::default() };
        Entries { entries: [none; MAX_ENTRIES], len: 0 }
    }

    pub(crate) fn push(&mut self, entry: BindGroupEntry) {
        self.entries[self.len] = entry;
        self.len += 1;
    }

    pub(crate) fn as_slice(&self) -> &[BindGroupEntry] {
        &self.entries[..self.len]
    }
}

/// A vertex buffer a draw binds, what keeps it where others share it, and
/// how its pipeline reads it.
struct VertexSlot {
    buffer: BufferId,
    held: Option<Held>,
    offset: u64,
    layout: VertexBufferLayout,
}

/// What a draw's bindings were made of: the epochs of the global uniform
/// buffers, of those of its frame's draws and of its own, the texture and
/// sampler it bound at each texture id, and the vertex buffers it reads as
/// storage buffers.
#[derive(Clone, Copy, PartialEq)]
struct BindingsStamp {
    global: u64,
    frame: u64,
    own: u64,
    textures: [Option<(TextureId, SamplerId)>; MAX_TEXTURES],
    pulled: [Option<BufferId>; MAX_PULLED],
}

/// The most vertex buffers a program reads as storage buffers.
const MAX_PULLED: usize = mln_gfx::shaders::bindings::MAX_VERTEX_BUFFERS;

/// What, besides the draw's own state, a draw's pipeline depends on.
#[derive(Clone, Copy, PartialEq)]
struct PipelineVariant {
    primitive: DrawModeType,
    depth: DepthState,
    stencil: StencilState,
    targets: TargetFormats,
}

/// Geometry drawn with one program, and the GPU objects that draw it: its
/// vertices, indexes and segments, the buffers and textures it binds, the
/// color and cull modes it draws with, and the shader, pipelines, bindings
/// and geometry it made of them on the context's `Gpu`, which it makes again
/// when what they are made of changes, the frame's projection included.
pub(crate) struct DrawObjects {
    program: &'static Program,
    /// The program's properties that its variant reads from their uniforms.
    uniforms: Uniforms,
    /// The projection it draws in, `None` for the frame's.
    projection: Option<ProjectionVariant>,
    /// The key of the variant of the program it last drew with, and that
    /// variant's projection.
    shader: Option<(ProjectionVariant, ShaderKey)>,
    vertex_attributes: VertexAttributeArray,
    /// The attributes of each instance, for a draw of instances.
    instance_attributes: Option<VertexAttributeArray>,
    /// The textures it binds, by texture id.
    textures: Textures,
    /// The buffers of its program's blocks of a draw's own, which its draw
    /// writes, and the bytes last written to each, by id.
    own_buffers: UniformBufferArray,
    own_bytes: SmallVec<[(usize, Vec<u8>); 2]>,
    /// The vertex buffers its program reads as storage buffers, each at its
    /// buffer's id, with the GPU buffer of its last upload.
    pulled: SmallVec<[(usize, SharedBuffer, Option<BufferId>); 2]>,
    color_mode: ColorMode,
    cull_face_mode: CullFaceMode,

    mode: DrawMode,
    segments: Vec<SegmentBase>,
    indexes: SharedBuffer,
    vertex_count: usize,
    attribute_bindings: AttributeBindingArray,
    instance_bindings: AttributeBindingArray,
    /// The vertex buffers of the bindings, `None` until a draw after either
    /// changes computes them.
    vertex_slots: Option<Vec<VertexSlot>>,
    /// The context's shader of the draw's, once it drew.
    shader_id: Option<ShaderId>,
    /// The pipelines it drew with, for its shader, `pipeline_layouts`, color
    /// and cull modes as they are.
    pipelines: Vec<(PipelineVariant, PipelineId)>,
    /// The vertex buffer layouts of the vertex slots the pipelines were made
    /// for.
    pipeline_layouts: Vec<VertexBufferLayout>,
    /// What it bound at its last draw, and what that was made of.
    bindings: Option<(Owned<BindingsId>, BindingsStamp)>,
    /// The geometry of its vertex slots and the index buffer it was made
    /// with.
    geometry: Option<(Owned<GeometryId>, BufferId)>,
}

impl DrawObjects {
    /// The objects of a draw of `geometry` with the variant of `program`
    /// that reads `uniforms` from their uniforms, in `projection` or the
    /// frame's, in `color_mode` and `cull_face_mode`, binding `textures`.
    #[must_use]
    pub(crate) fn new(
        program: &'static Program,
        uniforms: Uniforms,
        projection: Option<ProjectionVariant>,
        color_mode: ColorMode,
        cull_face_mode: CullFaceMode,
        geometry: DrawGeometry,
        textures: Textures,
    ) -> Self {
        let DrawGeometry { vertices, vertex_count, mode, indexes, segments, instances } = geometry;
        DrawObjects {
            program,
            uniforms,
            projection,
            shader: None,
            vertex_attributes: vertices,
            instance_attributes: instances,
            textures,
            own_buffers: UniformBufferArray::new(),
            own_bytes: SmallVec::new(),
            pulled: SmallVec::new(),
            color_mode,
            cull_face_mode,
            mode,
            segments,
            indexes,
            vertex_count,
            attribute_bindings: Vec::new(),
            instance_bindings: Vec::new(),
            vertex_slots: None,
            shader_id: None,
            pipelines: Vec::new(),
            pipeline_layouts: Vec::new(),
            bindings: None,
            geometry: None,
        }
    }

    /// The program the objects draw with.
    pub(crate) fn program(&self) -> &'static Program {
        self.program
    }

    /// Makes the draws read `buffer` as the program's vertex buffer `id`
    /// that its vertex stage reads as a storage buffer.
    pub(crate) fn pull(&mut self, id: usize, buffer: SharedBuffer) {
        debug_assert!(id >= FIRST_VERTEX_BUFFER_ID && self.pulled.len() < MAX_PULLED);
        self.pulled.push((id, buffer, None));
    }

    /// The key of the shader it draws with in a frame of `variant`'s
    /// projection, made again, with the shader's pipelines, bindings and
    /// geometry, where it last drew in another.
    fn shader(&mut self, context: &mut Resources, variant: ProjectionVariant) -> &ShaderKey {
        let variant = self.projection.unwrap_or(variant);
        if self.shader.as_ref().is_some_and(|(drawn, _)| *drawn != variant) {
            self.shader = None;
            self.shader_id = None;
            self.pipelines.clear();
            self.bindings = None;
            self.geometry = None;
        }
        let (program, uniforms) = (self.program, self.uniforms);
        &self.shader.get_or_insert_with(|| (variant, context.shader_key(program, uniforms, variant))).1
    }

    /// Uploads the indexes, vertex attributes and textures, where they
    /// changed since the last upload.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) {
        if let Err(error) = self.upload_in(gpu, context) {
            Log::error(Event::Render, format!("Uploading {} failed: {error}", self.program.name));
        }
    }

    fn upload_in(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) -> Result<(), GpuError> {
        let program = self.program;
        context.upload_index_buffer(gpu, &self.indexes)?;

        // The attributes bind at the first upload, and again where the
        // upload writes what they read, which may be new. A failed upload
        // leaves no bindings, so the next binds them again.
        if self.attribute_bindings.is_empty()
            || upload::needs_upload(context, program.attributes, &self.vertex_attributes)
        {
            let updated = upload::update_attribute_bindings(
                gpu,
                context,
                program.attributes,
                &mut self.vertex_attributes,
                self.vertex_count,
                &mut self.attribute_bindings,
            );
            if !matches!(updated, Ok(false)) {
                self.vertex_slots = None;
                self.geometry = None;
            }
            updated?;
        }

        let unbound = self.instance_bindings.is_empty();
        if let Some(attrs) = self
            .instance_attributes
            .as_mut()
            .filter(|attrs| unbound || upload::needs_upload(context, program.instance_attributes, attrs))
        {
            let instance_count = attrs.min_count();
            let updated = upload::update_attribute_bindings(
                gpu,
                context,
                program.instance_attributes,
                attrs,
                instance_count,
                &mut self.instance_bindings,
            );
            if !matches!(updated, Ok(false)) {
                self.vertex_slots = None;
                self.geometry = None;
            }
            updated?;
        }

        for (_, buffer, uploaded) in &mut self.pulled {
            *uploaded = context.upload_buffer(gpu, buffer)?;
        }

        for (texture, _) in self.textures.iter().flatten() {
            if let Err(error) = context.upload_texture(gpu, texture) {
                Log::error(Event::Render, format!("Uploading a texture failed: {error}"));
            }
        }

        Ok(())
    }

    /// Draws the segments into `parameters`' open pass on `gpu`, the
    /// context's, with `depth` and `stencil`, binding the buffers
    /// of the draws in `frame`. Shaders read the draw's entries in those
    /// storage buffers at `global_index`.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        frame: &UniformBufferArray,
        depth: DepthTest,
        stencil: Stencil,
        global_index: u32,
    ) {
        if let Err(error) = self.draw_in(gpu, parameters, frame, depth, stencil, global_index) {
            Log::warning(Event::Render, format!("Drawing {} failed: {error}", self.program.name));
        }
    }

    fn draw_in(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        frame: &UniformBufferArray,
        depth: DepthTest,
        stencil: Stencil,
        global_index: u32,
    ) -> Result<(), GpuError> {
        let Some(DrawTarget { size, targets, scissor }) = parameters.draw_target() else {
            return Ok(());
        };
        let variant =
            if parameters.frame.camera.is_globe() { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };
        let context = &mut *parameters.context;
        let shader = self.shader(context, variant).clone();
        let index_buffer = context.stored_buffer(&self.indexes).and_then(StoredBuffer::buffer);
        debug_assert!(index_buffer.is_some(), "a draw of {} draws indexes it didn't upload", self.program.name);
        let Some(index_buffer) = index_buffer else {
            return Ok(());
        };

        let shader_id = match self.shader_id {
            Some(id) => id,
            None => *self.shader_id.insert(context.shader(gpu, &shader)?),
        };
        let bindings = self.bindings(gpu, context, shader_id, frame)?;
        let slots = if let Some(slots) = &self.vertex_slots {
            slots
        } else {
            let slots = vertex_slots(self.program, &self.attribute_bindings, &self.instance_bindings);
            // New buffers in the layouts of the old ones keep the
            // pipelines.
            let layouts = slots.iter().map(|slot| &slot.layout);
            if !layouts.clone().eq(self.pipeline_layouts.iter()) {
                self.pipeline_layouts = layouts.cloned().collect();
                self.pipelines.clear();
            }
            self.vertex_slots.insert(slots)
        };
        let geometry = match &self.geometry {
            Some((geometry, indices)) if *indices == index_buffer => geometry.id(),
            _ => {
                self.geometry = None;
                // On the stack: the geometry's handles take the allocation.
                let mut buffers = [(BufferId::default(), 0); MAX_VERTEX_BUFFERS];
                for (buffer, slot) in buffers.iter_mut().zip(slots) {
                    *buffer = (slot.buffer, slot.offset);
                }
                let geometry = gpu.create_geometry(&GeometryDesc {
                    shader: shader_id,
                    layouts: &self.pipeline_layouts,
                    vertex_buffers: &buffers[..slots.len()],
                    index_buffer,
                })?;
                // Of the exact length: geometry is made as tiles come.
                let len = slots.iter().filter(|slot| slot.held.is_some()).count() + 1;
                let mut held = Vec::with_capacity(len);
                held.extend(slots.iter().filter_map(|slot| slot.held.clone()).chain([self.indexes.held()]));
                self.geometry = Some((context.own(geometry, held), index_buffer));
                geometry
            }
        };

        let full = Scissor { x: 0, y: 0, width: size.width, height: size.height };
        let dynamic = DynamicState {
            depth_range: depth.range,
            scissor: scissor.unwrap_or(full),
            stencil_reference: stencil.reference,
            blend_constant: self.color_mode.blend_color,
            line_width: self.mode.line_width(),
        };
        let (depth, stencil) = (depth.state, stencil.state);
        let mut draws = 0;
        let mode = self.mode;
        let instances = self.instance_attributes.as_ref().map_or(1, VertexAttributeArray::min_count);
        for segment in &self.segments {
            if segment.index_length == 0 {
                continue;
            }
            let variant = PipelineVariant { primitive: mode.primitive(), depth, stencil, targets };
            let pipeline = if let Some(&(_, id)) = self.pipelines.iter().find(|(cached, _)| *cached == variant) {
                id
            } else {
                let key = PipelineKey {
                    shader: shader_id,
                    vertex_buffers: self.pipeline_layouts.clone(),
                    state: fixed_state(variant, &self.color_mode, self.cull_face_mode),
                    targets,
                };
                let id = context.pipeline(gpu, &key)?;
                self.pipelines.push((variant, id));
                id
            };
            gpu.draw(&Draw {
                pipeline,
                bindings,
                geometry,
                first_index: segment.index_offset as u32,
                index_count: segment.index_length as u32,
                base_vertex: segment.vertex_offset as i32,
                first_instance: segment.base_instance as u32,
                instance_count: instances as u32,
                global_index,
                dynamic,
            })?;
            draws += 1;
        }
        let stats = context.rendering_stats();
        stats.num_draw_calls += draws;
        stats.total_draw_calls += draws;
        Ok(())
    }

    /// The bindings of what the shader, `shader_id` on `gpu`, reads of the
    /// context's global buffers, the buffers of the draws in `frame` and its
    /// textures, made again where one of them changed since the last draw.
    fn bindings(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Resources,
        shader_id: ShaderId,
        frame: &UniformBufferArray,
    ) -> Result<BindingsId, GpuError> {
        let mut textures = [None; MAX_TEXTURES];
        for id in 0..self.program.textures.len() {
            if let Some((texture, sampler)) = self.textures.get(id).and_then(Option::as_ref) {
                textures[id] = Some(context.texture_binding(gpu, texture, sampler.get())?);
            }
        }
        let global = context.global_uniform_buffers();
        let own = &self.own_buffers;
        let mut pulled = [None; MAX_PULLED];
        for (slot, (_, _, uploaded)) in pulled.iter_mut().zip(&self.pulled) {
            *slot = *uploaded;
        }
        let stamp = BindingsStamp { global: global.epoch(), frame: frame.epoch(), own: own.epoch(), textures, pulled };
        if let Some((bindings, made)) = &self.bindings
            && *made == stamp
        {
            return Ok(bindings.id());
        }
        self.bindings = None;
        let mut entries = Entries::new();
        for id in 0..FIRST_VERTEX_BUFFER_ID {
            // The draw's own buffer wins over the frame's, and the frame's
            // over the global one.
            let buffer = own.get(id).or_else(|| frame.get(id)).or_else(|| global.get(id));
            if let Some(buffer) = buffer {
                entries.push(BindGroupEntry::Buffer {
                    id: id as u32,
                    buffer: buffer.id(),
                    offset: 0,
                    size: buffer.size() as u64,
                });
            }
        }
        let mut held = Vec::new();
        for (id, buffer, uploaded) in &self.pulled {
            let size = context.stored_buffer(buffer).map_or(0, |stored| stored.count() * stored.vertex_size());
            if let Some(uploaded) = uploaded.filter(|_| size > 0) {
                entries.push(BindGroupEntry::Buffer { id: *id as u32, buffer: uploaded, offset: 0, size: size as u64 });
                held.push(buffer.held());
            }
        }
        for (id, binding) in textures.iter().enumerate() {
            if let Some((texture, sampler)) = *binding {
                entries.push(BindGroupEntry::Texture { id: id as u32, texture, sampler });
            }
        }
        let textures = 0..self.program.textures.len();
        held.extend(
            textures
                .filter_map(|id| context.gpu_texture(&self.textures.get(id)?.as_ref()?.0).cloned().map(Held::Texture)),
        );
        let bindings = gpu.create_bindings(&BindingsDesc {
            label: self.program.name,
            shader: shader_id,
            entries: entries.as_slice(),
        })?;
        self.bindings = Some((context.own(bindings, held), stamp));
        Ok(bindings)
    }

    /// Drops the bindings, if they were made of buffers of a uniform buffer
    /// array at one of the `retired` epochs.
    pub(crate) fn drop_retired_bindings(&mut self, retired: &[u64]) {
        if let Some((_, made)) = &self.bindings
            && [made.global, made.frame, made.own].iter().any(|epoch| retired.contains(epoch))
        {
            self.bindings = None;
        }
    }

    /// Writes `bytes` to the draw's own uniform buffer `id`, for its program's
    /// block of a draw's own at that id, unless the buffer holds them.
    pub(crate) fn write_own(&mut self, gpu: &mut dyn Gpu, id: usize, bytes: &[u8], device: &mut Device) {
        let index = match self.own_bytes.iter().position(|(written, _)| *written == id) {
            Some(index) if self.own_bytes[index].1 == bytes && self.own_buffers.get(id).is_some() => return,
            Some(index) => index,
            None => {
                self.own_bytes.push((id, Vec::new()));
                self.own_bytes.len() - 1
            }
        };
        let written = &mut self.own_bytes[index].1;
        written.clear();
        written.extend_from_slice(bytes);
        self.own_buffers.write_uniform(gpu, id, bytes, device);
    }
}

fn fixed_state(variant: PipelineVariant, color_mode: &ColorMode, cull: CullFaceMode) -> FixedState {
    FixedState {
        primitive: variant.primitive,
        cull,
        blend: color_mode.blend_function,
        color_mask: color_mode.mask,
        depth: variant.depth,
        stencil: variant.stencil,
    }
}

/// The vertex buffers of a draw's attribute and instance bindings, which
/// hold an attribute for each of the shader's in the shader's order.
/// Attributes that read one buffer with one stride from one offset share a
/// slot; attributes of a buffer at different vertex offsets (a fill
/// extrusion wall's outline vertex and the next one) read it through slots
/// of their own, since an attribute's offset must stay within the stride.
fn vertex_slots(
    program: &Program,
    attributes: &AttributeBindingArray,
    instances: &AttributeBindingArray,
) -> Vec<VertexSlot> {
    let mut slots: Vec<VertexSlot> = Vec::new();
    for (bindings, step) in [(attributes, VertexStep::Vertex), (instances, VertexStep::Instance)] {
        for (index, binding) in bindings.iter().enumerate() {
            let buffer = &binding.buffer;
            let attributes = match step {
                VertexStep::Vertex => program.attributes,
                VertexStep::Instance => program.instance_attributes,
            };
            if index >= attributes.len() {
                continue;
            }
            let offset = u64::from(binding.vertex_offset) * u64::from(binding.vertex_stride);
            let attribute = GpuVertexAttribute {
                id: index as u32,
                format: binding.attribute.data_type,
                offset: binding.attribute.offset,
            };
            let shared = slots.iter_mut().find(|slot| {
                slot.buffer == *buffer
                    && slot.layout.stride == binding.vertex_stride
                    && slot.layout.step == step
                    && slot.offset == offset
            });
            match shared {
                Some(slot) => slot.layout.attributes.push(attribute),
                None => slots.push(VertexSlot {
                    buffer: *buffer,
                    held: binding.held.clone(),
                    offset,
                    layout: VertexBufferLayout { stride: binding.vertex_stride, step, attributes: vec![attribute] },
                }),
            }
        }
    }
    if slots.len() > MAX_VERTEX_BUFFERS {
        Log::warning(
            Event::Render,
            format!(
                "{} uses {} vertex buffers, exceeding the limit of {MAX_VERTEX_BUFFERS}",
                program.name,
                slots.len()
            ),
        );
        slots.truncate(MAX_VERTEX_BUFFERS);
    }
    slots
}

#[cfg(test)]
mod tests {
    use mln_gfx::projection_variant::ProjectionVariant;

    use crate::gfx::context::Resources;
    use crate::mesh_desc::MeshDesc;
    use crate::shaders::programs;

    #[test]
    fn a_mesh_draws_in_the_projection_of_the_frame_unless_it_names_one() {
        let mut context = Resources::detached();
        let vertices = context.buffer::<[i16; 2]>(&[[0, 0], [1, 0], [0, 1]]);
        let triangles = context.index_buffer(&[0, 1, 2]);
        let mut desc = MeshDesc::new(&programs::TILE_DEBUG, &triangles);
        desc.vertices(&vertices);
        let follows: crate::draw::Mesh = context.mesh(&desc);
        desc.projection(ProjectionVariant::Mercator);
        let pinned: crate::draw::Mesh = context.mesh(&desc);

        let mut shaders = |mesh: &crate::draw::Mesh| {
            let mut objects = context.take_mesh(mesh.key()).expect("a mesh");
            let keys = [ProjectionVariant::Mercator, ProjectionVariant::Globe]
                .map(|variant| objects.shader(&mut context, variant).clone());
            context.restore_mesh(mesh.key(), objects);
            keys
        };
        let [mercator, globe] = shaders(&follows);
        assert_ne!(mercator, globe, "a mesh made on Mercator draws the globe's variant on the globe");
        let [mercator_pinned, globe_pinned] = shaders(&pinned);
        assert_eq!(mercator_pinned, globe_pinned);
        assert_eq!(mercator_pinned, mercator);
    }

    #[test]
    fn an_instance_attribute_from_a_vertex_on_reads_as_many_instances_as_follow_it() {
        let mut context = Resources::detached();
        let corners = context.buffer::<[i16; 2]>(&[[0, 0], [1, 0], [0, 1]]);
        let points = context.buffer::<[i16; 2]>(&[[0, 0], [1, 1], [2, 2], [3, 3]]);
        let triangles = context.index_buffer(&[0, 1, 2]);
        let mut desc = MeshDesc::new(&programs::TILE_DEBUG, &triangles);
        desc.vertices(&corners).instances(&points).instances_from(&points, 1);
        let mesh: crate::draw::Mesh = context.mesh(&desc);
        let objects = context.take_mesh(mesh.key()).expect("a mesh");
        let instances = objects.instance_attributes.as_ref().expect("instances");
        let reads: Vec<(usize, u32)> = instances.iter().map(|(at, attribute)| (at, attribute.vertex_offset)).collect();
        assert_eq!(reads, [(0, 0), (1, 1)]);
        assert_eq!(instances.min_count(), 3, "an instance for each point but the last");
        context.restore_mesh(mesh.key(), objects);
    }

    #[test]
    fn a_property_whose_attribute_the_mesh_leaves_unbound_reads_its_uniform() {
        use mln_gfx::gfx_types::AttributeDataType::{Float, Float4, Short2};
        use mln_gfx::shaders::program::{Attribute, Program, Projection, Property, Sources};

        static PROGRAM: Program = Program {
            name: "test",
            buffers: &[],
            attributes: &[
                Attribute::new("pos", Short2),
                Attribute::new("color", Float4),
                Attribute::new("width", Float),
            ],
            instance_attributes: &[],
            textures: &[],
            properties: &[Property::color("color"), Property::float("width")],
            projection: Projection::None,
            sources: Sources::NONE,
        };
        /// A vertex's width, bound to the program's attribute of its name.
        #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
        #[repr(C)]
        struct Width(f32);
        impl crate::draw::Vertex for Width {
            const ATTRIBUTES: &'static [mln_gfx::gfx_types::AttributeDataType] = &[Float];
            const NAMES: &'static [&'static str] = &["width"];
        }

        let mut context = Resources::detached();
        let positions = context.buffer::<[i16; 2]>(&[[0, 0], [1, 0], [0, 1]]);
        let widths = context.buffer(&[Width(1.0); 3]);
        let triangles = context.index_buffer(&[0, 1, 2]);
        let mut desc = MeshDesc::new(&PROGRAM, &triangles);
        desc.vertices(&positions).vertices(&widths);
        let mesh: crate::draw::Mesh = context.mesh(&desc);
        let objects = context.take_mesh(mesh.key()).expect("a mesh");
        assert!(objects.uniforms.contains(0), "the color, which no buffer binds");
        assert!(!objects.uniforms.contains(1), "the width, which a buffer binds by its name");
        context.restore_mesh(mesh.key(), objects);
    }

    #[test]
    fn quads_of_any_count_share_one_buffer_in_segments_of_their_vertices() {
        let mut context = Resources::detached();
        let (few, many) = (context.quads(2), context.quads(20_000));
        assert_eq!(few.buffer, many.buffer);
        let lengths = |quads: &crate::draw::IndexBuffer| -> Vec<(usize, usize, usize)> {
            quads.segments.iter().map(|s| (s.vertex_offset, s.vertex_length, s.index_length)).collect()
        };
        assert_eq!(lengths(&few), [(0, 8, 12)]);
        let most = mln_gfx::shaders::segment::MAX_SEGMENT_VERTICES / 4;
        assert_eq!(lengths(&many), [(0, 4 * most, 6 * most), (4 * most, 4 * (20_000 - most), 6 * (20_000 - most))]);
    }
}
