//! Meshes, the GPU objects of what a draw draws, which the renderer keeps
//! between frames and makes again only when what they are made of changes,
//! and the buffers a frame's draws bind with the bindings made of them.
//!
//! A mesh owns what is its own, its own indexes and attribute data, and
//! holds what it shares with other meshes, its textures and the indexes and
//! vertices of buckets and static geometry, through handles of the context's
//! stores (`store`), which keep them while the mesh lasts. It binds no
//! uniform buffers: the frame that draws it does ([`DrawBuffers`]), so that
//! frames with buffers of their own can draw one mesh.
//!
//! A shader reads vertices as attributes, or, where the device binds vertex
//! buffers (`Caps::binds_vertex_buffers`), as a storage buffer that it
//! indexes itself (`Mesh::read_as_storage`).

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType, PrimitiveType};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferId, DepthState, Draw, DynamicState, FixedState, GeometryDesc,
    GeometryId, Gpu, GpuError, PipelineId, PipelineKey, SamplerId, Scissor, ShaderId, ShaderKey, StencilState,
    TargetFormats, TextureId, VertexAttribute as GpuVertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::bindings::{BUFFER_IDS, MAX_TEXTURES};
use mln_gfx::shaders::program::Program;
use mln_gfx::stencil_mode::StencilMode;
use mln_style::hash::HashMap;

use crate::draw::atlas_binding::AtlasBinding;
use crate::draw::buffer::Buffer;
use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::draw::indexes::DrawIndexes;
use crate::draw::owned::{Held, Owned};
use crate::draw::resources::Device;
use crate::draw::segment::SegmentBase;
use crate::draw::store::{MeshKey, Shared, SharedTexture, WeakShared};
use crate::draw::uniform_buffer::UniformBufferArray;
use crate::draw::upload::{self, AttributeBindingArray, AttributeSlots};
use crate::draw::vertex_attribute::VertexAttributeArray;
use crate::draw::vertices::SharedVertices;
use crate::log_target;
use crate::render::paint_parameters::{DrawTarget, PaintParameters};

/// The textures a draw binds, by the slots of its program's textures.
pub type Textures = [Option<SharedTexture>; MAX_TEXTURES];

/// The most vertex buffers a draw binds, WebGPU's default limit.
const MAX_VERTEX_BUFFERS: usize = 8;

/// What a mesh draws: its vertices, and the primitives its segments draw of
/// them.
pub struct DrawGeometry {
    /// The vertex attributes, by the names of the program's attributes.
    pub vertices: VertexAttributeArray,
    /// How many vertices the attributes hold, which an attribute the
    /// shader reads and the draw lacks gets default values for.
    pub vertex_count: usize,
    /// The primitive the segments draw.
    pub mode: DrawMode,
    pub indexes: DrawIndexes,
    /// The ranges of the indexes and vertices each draw call draws.
    pub segments: Vec<SegmentBase>,
    /// The attributes of each instance, for a draw of instances.
    pub instances: Option<VertexAttributeArray>,
}

/// `count` vertices of `data_type` in `data` as the vertex attribute `name`,
/// none where there are no vertices.
#[must_use]
pub fn raw_vertices(
    name: &'static str,
    data: Vec<u8>,
    count: usize,
    data_type: AttributeDataType,
) -> VertexAttributeArray {
    let mut attributes = VertexAttributeArray::new();
    if count == 0 || data_type == AttributeDataType::Invalid || data.is_empty() {
        return attributes;
    }
    attributes.set_own(name, data_type, data);
    attributes
}

/// The most vertices a draw's shader reads as storage buffers.
const MAX_STORAGE_VERTICES: usize = 3;

/// The most entries bindings have.
const MAX_ENTRIES: usize = BUFFER_IDS + MAX_STORAGE_VERTICES + MAX_TEXTURES;

/// The entries of bindings, on the stack: at most a buffer at each uniform
/// buffer id, one of each of the vertices read as storage buffers, and a
/// texture at each texture slot.
pub(crate) struct Entries {
    entries: [BindGroupEntry; MAX_ENTRIES],
    len: usize,
}

impl Entries {
    pub(crate) fn new() -> Entries {
        let none = BindGroupEntry::Texture { slot: 0, texture: TextureId::default(), sampler: SamplerId::default() };
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

/// What a mesh's bindings were made of besides its frame's buffers: the
/// epoch of the global uniform buffers, how often the buffers of the
/// vertices the mesh reads as storage buffers changed, and the texture and
/// sampler it bound at each texture slot.
#[derive(Clone, Copy, PartialEq)]
struct BindingsStamp {
    global: u64,
    storage: u64,
    textures: [Option<(TextureId, SamplerId)>; MAX_TEXTURES],
}

/// The bindings a frame made of a mesh, while the mesh lasts.
struct MeshBindings {
    mesh: WeakShared<MeshKey>,
    bindings: Owned<BindingsId>,
    stamp: BindingsStamp,
}

/// The uniform and storage buffers that a frame's draws bind, by id, and
/// the bindings of each mesh the draws drew, made of them, of the context's
/// global buffers and of the mesh's textures and vertices read as storage
/// buffers. The bindings of a mesh go when the mesh drops, by the next
/// [`sweep`](Self::sweep), and all of them when a buffer is made or
/// replaced, before the buffer they were made of goes.
#[derive(Default)]
pub(crate) struct DrawBuffers {
    buffers: UniformBufferArray,
    bindings: HashMap<SimpleIdentity, MeshBindings>,
}

impl DrawBuffers {
    /// The buffer `id`, if the frame has one.
    pub(crate) fn get(&self, id: usize) -> Option<&Buffer> {
        self.buffers.get(id)
    }

    /// Writes `value` to the uniform buffer `id`, as
    /// [`UniformBufferArray::write_pod`] does.
    pub(crate) fn write_pod<T: bytemuck::Pod>(&mut self, gpu: &mut dyn Gpu, id: usize, value: &T, device: &mut Device) {
        let epoch = self.buffers.epoch();
        let _replaced = self.buffers.write_pod(gpu, id, value, device);
        self.made(epoch);
    }

    /// Writes `data`, the bytes of a block, to the uniform buffer `id`, as
    /// [`UniformBufferArray::write_uniform`] does.
    pub(crate) fn write_uniform(&mut self, gpu: &mut dyn Gpu, id: usize, data: &[u8], device: &mut Device) {
        let epoch = self.buffers.epoch();
        let _replaced = self.buffers.write_uniform(gpu, id, data, device);
        self.made(epoch);
    }

    /// Writes `data` to the storage buffer `id`, as
    /// [`UniformBufferArray::write_storage`] does.
    pub(crate) fn write_storage(&mut self, gpu: &mut dyn Gpu, id: usize, data: &[u8], device: &mut Device) {
        let epoch = self.buffers.epoch();
        let _replaced = self.buffers.write_storage(gpu, id, data, device);
        self.made(epoch);
    }

    /// Drops the bindings where a write since the buffers were at `epoch`
    /// made or replaced a buffer, which they don't bind.
    fn made(&mut self, epoch: u64) {
        if self.buffers.epoch() != epoch {
            self.bindings.clear();
        }
    }

    /// Drops the bindings of the meshes that dropped.
    pub(crate) fn sweep(&mut self) {
        self.bindings.retain(|_, made| made.mesh.is_held());
    }

    /// The bindings of what `mesh`'s shader, `shader_id` on `gpu`, reads of
    /// the context's global buffers, the frame's buffers, and the mesh's
    /// textures and vertices read as storage buffers, made again where one
    /// of them changed since the frame last drew the mesh.
    fn bindings(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        mesh: &DrawObjects,
        shader_id: ShaderId,
    ) -> Result<BindingsId, GpuError> {
        let program = mesh.shader.program();
        let mut textures = [None; MAX_TEXTURES];
        for slot in sampled_slots(program) {
            if let Some(texture) = mesh.textures.get(slot).and_then(Option::as_ref) {
                textures[slot] = Some(context.texture_binding(gpu, texture)?);
            }
        }
        let global = context.global_uniform_buffers();
        let stamp = BindingsStamp { global: global.epoch(), storage: mesh.storage_epoch, textures };
        if let Some(made) = self.bindings.get(&mesh.id)
            && made.stamp == stamp
        {
            return Ok(made.bindings.id());
        }
        self.bindings.remove(&mesh.id);
        let mut entries = Entries::new();
        for id in 0..BUFFER_IDS {
            // The frame's buffer wins over the global one.
            let buffer = self.buffers.get(id).or_else(|| global.get(id));
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
        for (id, vertices, buffer) in &mesh.storage_vertices {
            let size = context.vertices(vertices).map_or(0, |stored| stored.count() * stored.vertex_size());
            if let Some(buffer) = buffer.filter(|_| size > 0) {
                entries.push(BindGroupEntry::Buffer { id: *id as u32, buffer, offset: 0, size: size as u64 });
                held.push(vertices.held());
            }
        }
        for (slot, binding) in textures.iter().enumerate() {
            if let Some((texture, sampler)) = *binding {
                entries.push(BindGroupEntry::Texture { slot: slot as u32, texture, sampler });
            }
        }
        held.extend(
            sampled_slots(program)
                .filter_map(|slot| context.gpu_texture(mesh.textures.get(slot)?.as_ref()?).cloned().map(Held::Texture)),
        );
        let bindings =
            gpu.create_bindings(&BindingsDesc { label: program.name, shader: shader_id, entries: entries.as_slice() })?;
        let made = MeshBindings { mesh: mesh.handle.clone(), bindings: context.own(bindings, held), stamp };
        self.bindings.insert(mesh.id, made);
        Ok(bindings)
    }

    /// The frame's bindings of `mesh`, as a draw of it binds them.
    #[cfg(test)]
    pub(crate) fn bind(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        mesh: &Mesh,
    ) -> Result<BindingsId, GpuError> {
        let objects = context.take_mesh(mesh).ok_or(GpuError::UnknownId("mesh"))?;
        let bound =
            context.shader(gpu, &objects.shader).and_then(|shader| self.bindings(gpu, context, &objects, shader));
        context.put_mesh(mesh, objects);
        bound
    }

    /// How many meshes the frame keeps bindings of.
    #[cfg(test)]
    pub(crate) fn bound_meshes(&self) -> usize {
        self.bindings.len()
    }
}

/// What, besides the draw's own state, a draw's pipeline depends on.
#[derive(Clone, Copy, PartialEq)]
struct PipelineVariant {
    primitive: DrawModeType,
    depth: DepthState,
    stencil: StencilState,
    targets: TargetFormats,
}

/// Geometry drawn with one program, and the GPU objects that draw it: its
/// vertices, indexes and segments, the textures it binds, the color and cull
/// modes it draws with, and the shader, pipelines and geometry it made of
/// them on the context's `Gpu`, which it makes again when what they are made
/// of changes. The frames that draw it bind it with their buffers
/// ([`DrawBuffers`]).
pub(crate) struct DrawObjects {
    /// Tells the mesh apart from the others, and orders it after those made
    /// before it.
    id: SimpleIdentity,
    /// The mesh's handles, which the frames' bindings of it last while one
    /// does.
    handle: WeakShared<MeshKey>,
    shader: ShaderKey,
    vertex_attributes: VertexAttributeArray,
    /// The attributes of each instance, for a draw of instances.
    instance_attributes: Option<VertexAttributeArray>,
    /// Whether the vertex attributes changed since the last upload, which
    /// then binds them all again; true until the first.
    vertex_attributes_changed: bool,
    /// Whether the instance attributes changed since the last upload, as
    /// `vertex_attributes_changed` says of the vertex attributes.
    instance_attributes_changed: bool,
    /// Where the program's vertex and instance attributes are in the
    /// attribute arrays.
    vertex_slots_of: AttributeSlots,
    instance_slots_of: AttributeSlots,
    /// The textures it binds, by slot.
    textures: Textures,
    /// The vertices the shader reads as storage buffers, each with the
    /// buffer id it binds at, past the uniform buffers' ids, and its buffer
    /// as of the last upload.
    storage_vertices: Vec<(usize, SharedVertices, Option<BufferId>)>,
    /// How often a buffer of `storage_vertices` changed.
    storage_epoch: u64,
    color_mode: ColorMode,
    cull_face_mode: CullFaceMode,

    mode: DrawMode,
    segments: Vec<SegmentBase>,
    indexes: DrawIndexes,
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
    /// The geometry of its vertex slots and the index buffer it was made
    /// with.
    geometry: Option<(Owned<GeometryId>, BufferId)>,
    /// Gives it the textures of its tile's atlases before each draw.
    atlases: Option<AtlasBinding>,
}

impl DrawObjects {
    /// The objects of `geometry`, drawn with `shader` in `color_mode` and
    /// `cull_face_mode`, binding `textures`.
    #[must_use]
    pub(crate) fn new(
        shader: ShaderKey,
        color_mode: ColorMode,
        cull_face_mode: CullFaceMode,
        geometry: DrawGeometry,
        textures: Textures,
    ) -> Self {
        let DrawGeometry { vertices, vertex_count, mode, indexes, segments, instances } = geometry;
        DrawObjects {
            id: SimpleIdentity::new(),
            handle: WeakShared::default(),
            shader,
            vertex_attributes: vertices,
            instance_attributes: instances,
            vertex_attributes_changed: true,
            instance_attributes_changed: true,
            vertex_slots_of: AttributeSlots::default(),
            instance_slots_of: AttributeSlots::default(),
            textures,
            storage_vertices: Vec::new(),
            storage_epoch: 0,
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
            geometry: None,
            atlases: None,
        }
    }

    /// Uploads the indexes, vertex attributes, vertices read as storage
    /// buffers and textures, where they changed since the last upload.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if let Err(error) = self.upload_in(gpu, context) {
            context.fail(format!("Uploading {} failed: {error}", self.shader.program().name));
        }
    }

    fn upload_in(&mut self, gpu: &mut dyn Gpu, context: &mut Context) -> Result<(), GpuError> {
        let program = self.shader.program();
        if self.indexes.buffer(context).is_none() && self.indexes.len(context) == 0 {
            debug_assert!(false, "a draw of {} has no index data", program.name);
            return Ok(());
        }
        self.indexes.upload(gpu, context)?;

        // An attribute binds again where its array changed, or where the
        // upload writes what it reads, which may be new. A failed upload
        // leaves the attributes changed, so the next binds them again.
        if self.vertex_attributes_changed
            || upload::needs_upload(context, &self.vertex_attributes, &self.vertex_slots_of)
        {
            let updated = upload::update_attribute_bindings(
                gpu,
                context,
                program.attributes,
                &mut self.vertex_attributes,
                &mut self.vertex_slots_of,
                self.vertex_count,
                &mut self.attribute_bindings,
            );
            if !matches!(updated, Ok(false)) {
                self.vertex_slots = None;
                self.geometry = None;
            }
            updated?;
        }

        let changed = self.instance_attributes_changed;
        if let Some(attrs) = self
            .instance_attributes
            .as_mut()
            .filter(|attrs| changed || upload::needs_upload(context, attrs, &self.instance_slots_of))
        {
            let instance_count = attrs.min_count();
            let updated = upload::update_attribute_bindings(
                gpu,
                context,
                program.instance_attributes,
                attrs,
                &mut self.instance_slots_of,
                instance_count,
                &mut self.instance_bindings,
            );
            if !matches!(updated, Ok(false)) {
                self.vertex_slots = None;
                self.geometry = None;
            }
            updated?;
        }

        self.vertex_attributes_changed = false;
        self.instance_attributes_changed = false;

        for (_, vertices, bound) in &mut self.storage_vertices {
            let buffer = context.upload_vertices(gpu, vertices)?;
            if *bound != buffer {
                *bound = buffer;
                self.storage_epoch += 1;
            }
        }

        for texture in self.textures.iter().flatten() {
            if let Err(error) = context.upload_texture(gpu, texture) {
                context.fail(format!("Uploading a texture failed: {error}"));
            }
        }

        Ok(())
    }

    /// The program the mesh draws with.
    #[must_use]
    pub(crate) fn program(&self) -> &'static Program {
        self.shader.program()
    }

    /// Draws the segments into `parameters`' open pass on `gpu`, the
    /// context's, with `depth_mode` and `stencil_mode`, binding the buffers
    /// of the draws in `frame`. Shaders read the draw's entries in those
    /// storage buffers at `global_index`. Returns whether it didn't fail; a
    /// draw that fails fails the frame
    /// ([`Context::fail`](crate::draw::context::Context::fail)).
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        frame: &mut DrawBuffers,
        depth_mode: DepthMode,
        stencil_mode: StencilMode,
        global_index: u32,
    ) -> bool {
        if let Some(atlases) = self.atlases.take() {
            atlases.bind(self, parameters);
            self.atlases = Some(atlases);
        }
        let drawn = self.draw_in(gpu, parameters, frame, depth_mode, stencil_mode, global_index);
        if let Err(error) = &drawn {
            parameters.context.fail(format!("Drawing {} failed: {error}", self.shader.program().name));
        }
        drawn.is_ok()
    }

    fn draw_in(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        frame: &mut DrawBuffers,
        depth_mode: DepthMode,
        stencil_mode: StencilMode,
        global_index: u32,
    ) -> Result<(), GpuError> {
        let Some(DrawTarget { size, targets, scissor }) = parameters.draw_target() else {
            return Ok(());
        };
        let context = &mut *parameters.context;
        let Some(index_buffer) = self.indexes.buffer(context) else {
            // Only a failed upload, which failed the frame, leaves a draw
            // without its indexes.
            debug_assert!(
                context.device().failed(),
                "a draw of {} draws indexes it didn't upload",
                self.shader.program().name
            );
            return Err(GpuError::Invalid("its indexes aren't uploaded".to_owned()));
        };

        let shader_id = match self.shader_id {
            Some(id) => id,
            None => *self.shader_id.insert(context.shader(gpu, &self.shader)?),
        };
        let bindings = frame.bindings(gpu, context, self, shader_id)?;
        let slots = if let Some(slots) = &self.vertex_slots {
            slots
        } else {
            let slots = vertex_slots(self.shader.program(), &self.attribute_bindings, &self.instance_bindings);
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
                let index_held = self.indexes.held();
                // Of the exact length: geometry is made as tiles come.
                let len = slots.iter().filter(|slot| slot.held.is_some()).count() + usize::from(index_held.is_some());
                let mut held = Vec::with_capacity(len);
                held.extend(slots.iter().filter_map(|slot| slot.held.clone()).chain(index_held));
                self.geometry = Some((context.own(geometry, held), index_buffer));
                geometry
            }
        };

        let full = Scissor { x: 0, y: 0, width: size.width, height: size.height };
        let dynamic = DynamicState {
            depth_range: depth_mode.range,
            scissor: scissor.unwrap_or(full),
            stencil_reference: stencil_mode.ref_ as u32,
            blend_constant: self.color_mode.blend_color,
            line_width: 1.0,
        };
        let depth = DepthState::from(&depth_mode);
        let stencil = StencilState::from(&stencil_mode);
        let mut draws = 0;
        let mode = self.mode;
        for segment in &self.segments {
            if segment.index_length == 0 {
                continue;
            }
            let variant = PipelineVariant { primitive: mode.type_, depth, stencil, targets };
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
            let line_width = if mode.primitive_type() == PrimitiveType::Line { mode.size } else { 1.0 };
            gpu.draw(&Draw {
                pipeline,
                bindings,
                geometry,
                first_index: segment.index_offset as u32,
                index_count: segment.index_length as u32,
                base_vertex: segment.vertex_offset as i32,
                first_instance: segment.base_instance as u32,
                instance_count: segment.instance_count as u32,
                global_index,
                dynamic: DynamicState { line_width, ..dynamic },
            })?;
            draws += 1;
        }
        let stats = context.rendering_stats();
        stats.num_draw_calls += draws;
        stats.total_draw_calls += draws;
        Ok(())
    }

    /// The vertex attributes, to change, which the next upload binds again.
    /// The draw drops its geometry where they have values of their own,
    /// whose buffers a change may drop.
    pub(crate) fn vertex_attributes_mut(&mut self) -> &mut VertexAttributeArray {
        self.vertex_attributes_changed = true;
        if self.vertex_attributes.has_own() {
            self.geometry = None;
        }
        &mut self.vertex_attributes
    }

    /// The attributes the paint values are read into, to change, which the
    /// next upload binds again: the instance attributes of a draw of
    /// instances, else the vertex attributes.
    pub(crate) fn paint_attributes_mut(&mut self) -> &mut VertexAttributeArray {
        let (attributes, changed) = match &mut self.instance_attributes {
            Some(instances) => (instances, &mut self.instance_attributes_changed),
            None => (&mut self.vertex_attributes, &mut self.vertex_attributes_changed),
        };
        *changed = true;
        if attributes.has_own() {
            self.geometry = None;
        }
        attributes
    }

    /// Makes `vertices` what the shader reads as the storage buffer `id`,
    /// an id past the uniform buffers', on a device that binds vertex
    /// buffers; nothing with `None`. The next upload writes them, and the
    /// draws after it bind their buffer.
    pub(crate) fn read_as_storage(&mut self, id: usize, vertices: Option<SharedVertices>) {
        debug_assert!(id >= BUFFER_IDS, "a uniform buffer's id");
        self.storage_vertices.retain(|(at, ..)| *at != id);
        debug_assert!(vertices.is_none() || self.storage_vertices.len() < MAX_STORAGE_VERTICES);
        if let Some(vertices) = vertices {
            self.storage_vertices.push((id, vertices, None));
        }
        self.storage_epoch += 1;
    }

    /// The buffer the draws bind as the storage buffer `id`, of the
    /// vertices read there as of the last upload.
    pub(crate) fn storage_buffer(&self, id: usize) -> Option<BufferId> {
        self.storage_vertices.iter().find(|(at, ..)| *at == id).and_then(|&(_, _, buffer)| buffer)
    }

    /// Whether the draw draws instances.
    #[must_use]
    pub(crate) fn has_instances(&self) -> bool {
        self.instance_attributes.is_some()
    }

    /// The texture it binds at its program's texture slot `slot`.
    #[must_use]
    pub(crate) fn texture(&self, slot: usize) -> Option<&SharedTexture> {
        self.textures[slot].as_ref()
    }

    /// Makes `texture` the one it binds at its program's texture slot
    /// `slot`.
    pub(crate) fn set_texture(&mut self, slot: usize, texture: Option<&SharedTexture>) {
        let bound = &mut self.textures[slot];
        if bound.as_ref() != texture {
            *bound = texture.cloned();
        }
    }

    /// Replaces the indexes and the segments drawn from them.
    pub(crate) fn replace_indexes(&mut self, indexes: DrawIndexes, segments: Vec<SegmentBase>) {
        if let DrawIndexes::Own(_) = self.indexes {
            self.geometry = None;
        }
        self.indexes = indexes;
        self.segments = segments;
    }

    /// Culls faces as `cull_face_mode` says from the next draw on.
    pub(crate) fn set_cull_face_mode(&mut self, cull_face_mode: CullFaceMode) {
        if self.cull_face_mode != cull_face_mode {
            self.cull_face_mode = cull_face_mode;
            // The pipelines bake the culling in.
            self.pipelines.clear();
        }
    }

    #[must_use]
    pub(crate) fn shader(&self) -> &ShaderKey {
        &self.shader
    }
}

/// A handle of a mesh, the GPU objects of what a draw draws
/// (`DrawObjects`), which the context's store keeps while a clone of the
/// handle lasts (`Context::mesh`). A layer keeps its meshes' handles, in
/// its tile values or its own state, and the draws it pushes name them
/// ([`Group::push`]), so that the engine uploads and draws them after the
/// layer made its draws. Clones of a handle are the same mesh. A mesh
/// changes through the context, which the methods here take.
///
/// [`Group::push`]: crate::draw::draws::Group::push
#[derive(Clone)]
pub struct Mesh {
    key: Shared<MeshKey>,
    id: SimpleIdentity,
    program: &'static Program,
}

impl Mesh {
    /// The mesh of `geometry`, drawn with `shader` in `color_mode` and
    /// `cull_face_mode`, binding `textures`, in `context`'s store.
    #[must_use]
    pub fn new(
        context: &mut Context,
        shader: ShaderKey,
        color_mode: ColorMode,
        cull_face_mode: CullFaceMode,
        geometry: DrawGeometry,
        textures: Textures,
    ) -> Self {
        let mut objects = DrawObjects::new(shader, color_mode, cull_face_mode, geometry, textures);
        let (id, program) = (objects.id, objects.program());
        let key = context.add_mesh_with(|key| {
            objects.handle = key.downgrade();
            objects
        });
        Mesh { key, id, program }
    }

    /// The handle of the mesh's entry in the context's store.
    pub(crate) fn key(&self) -> &Shared<MeshKey> {
        &self.key
    }

    /// The place of the mesh among the meshes, by when they were made, as a
    /// draw's order ([`Draw::order`](crate::draw::draws::Draw::order)).
    #[must_use]
    pub fn seq(&self) -> i64 {
        self.id.get() as i64
    }

    /// The program the mesh draws with.
    #[must_use]
    pub(crate) fn program(&self) -> &'static Program {
        self.program
    }

    /// Runs `f` with the mesh's objects, taken out of `context`'s store
    /// while it runs, and the context; `None` without them.
    pub(crate) fn update<R>(
        &self,
        context: &mut Context,
        f: impl FnOnce(&mut DrawObjects, &mut Context) -> R,
    ) -> Option<R> {
        let mut objects = context.take_mesh(self)?;
        let result = f(&mut objects, context);
        context.put_mesh(self, objects);
        Some(result)
    }

    /// See `DrawObjects::upload`.
    pub fn upload(&self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.update(context, |objects, context| objects.upload(gpu, context));
    }

    /// Runs `f` with the mesh's vertex attributes, to change, and the
    /// context; `None` for a mesh whose objects went. The next upload binds
    /// the attributes again.
    pub fn update_vertex_attributes<R>(
        &self,
        context: &mut Context,
        f: impl FnOnce(&mut VertexAttributeArray, &mut Context) -> R,
    ) -> Option<R> {
        self.update(context, |objects, context| f(objects.vertex_attributes_mut(), context))
    }

    /// See [`DrawObjects::vertex_attributes_mut`].
    pub(crate) fn vertex_attributes_mut<'c>(&self, context: &'c mut Context) -> Option<&'c mut VertexAttributeArray> {
        context.mesh_mut(self).map(DrawObjects::vertex_attributes_mut)
    }

    /// See `DrawObjects::paint_attributes_mut`.
    pub fn paint_attributes_mut<'c>(&self, context: &'c mut Context) -> Option<&'c mut VertexAttributeArray> {
        context.mesh_mut(self).map(DrawObjects::paint_attributes_mut)
    }

    /// See `DrawObjects::read_as_storage`.
    pub fn read_as_storage(&self, context: &mut Context, id: usize, vertices: Option<SharedVertices>) {
        if let Some(objects) = context.mesh_mut(self) {
            objects.read_as_storage(id, vertices);
        }
    }

    /// See `DrawObjects::storage_buffer`.
    #[must_use]
    pub fn storage_buffer(&self, context: &Context, id: usize) -> Option<BufferId> {
        context.mesh(self)?.storage_buffer(id)
    }

    /// See `DrawObjects::has_instances`.
    #[must_use]
    pub fn has_instances(&self, context: &Context) -> bool {
        context.mesh(self).is_some_and(DrawObjects::has_instances)
    }

    /// Replaces the vertex attributes with a copy of those of `other`
    /// ([`copy_shared_attributes`]), which the next upload binds.
    pub fn share_vertex_attributes(&self, context: &mut Context, other: &Mesh) {
        if self.key == other.key {
            return;
        }
        let Some(copy) = context.mesh(other).map(|other| copy_shared_attributes(&other.vertex_attributes)) else {
            return;
        };
        if let Some(objects) = context.mesh_mut(self) {
            *objects.vertex_attributes_mut() = copy;
        }
    }

    /// See `DrawObjects::texture`.
    #[must_use]
    pub fn texture<'c>(&self, context: &'c Context, slot: usize) -> Option<&'c SharedTexture> {
        context.mesh(self)?.texture(slot)
    }

    /// See `DrawObjects::set_texture`.
    pub fn set_texture(&self, context: &mut Context, slot: usize, texture: Option<&SharedTexture>) {
        if let Some(objects) = context.mesh_mut(self) {
            objects.set_texture(slot, texture);
        }
    }

    /// Replaces the mesh's indexes and the segments drawn from them.
    pub fn replace_indexes(&self, context: &mut Context, indexes: DrawIndexes, segments: Vec<SegmentBase>) {
        if let Some(objects) = context.mesh_mut(self) {
            objects.replace_indexes(indexes, segments);
        }
    }

    /// See `DrawObjects::set_cull_face_mode`.
    pub fn set_cull_face_mode(&self, context: &mut Context, cull_face_mode: CullFaceMode) {
        if let Some(objects) = context.mesh_mut(self) {
            objects.set_cull_face_mode(cull_face_mode);
        }
    }

    /// Makes `atlases` what gives the mesh the textures of its tile's glyph
    /// and icon atlases before each draw.
    pub fn set_atlases(&self, context: &mut Context, atlases: Option<AtlasBinding>) {
        if let Some(objects) = context.mesh_mut(self) {
            objects.atlases = atlases;
        }
    }
}

/// The slots of the textures `program` samples.
fn sampled_slots(program: &Program) -> impl Iterator<Item = usize> + use<> {
    let textures = program.textures;
    (0..textures.len()).filter(move |&slot| !textures[slot].is_unused())
}

/// A copy of `attrs`, all of whose attributes read vertices of the
/// context's store: the copy reads the same vertices, so draws of either
/// bind the same buffers.
#[must_use]
pub fn copy_shared_attributes(attrs: &VertexAttributeArray) -> VertexAttributeArray {
    let mut copy = VertexAttributeArray::new();
    for (name, attr) in attrs.iter() {
        debug_assert!(attr.shared().is_some(), "a copied attribute has values of its own");
        if let Some(shared) = attr.shared() {
            copy.set_shared(name, shared.clone());
        }
    }
    copy
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
        let (count, first) = match step {
            VertexStep::Vertex => (program.attributes.len(), 0),
            VertexStep::Instance => (program.instance_attributes.len(), program.attributes.len()),
        };
        for (index, binding) in bindings.iter().enumerate().take(count) {
            let buffer = &binding.buffer;
            let offset = u64::from(binding.vertex_offset) * u64::from(binding.vertex_stride);
            let attribute = GpuVertexAttribute {
                location: (first + index) as u32,
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
        log::warn!(
            target: log_target::RENDER,
            "{} uses {} vertex buffers, exceeding the limit of {MAX_VERTEX_BUFFERS}",
            program.name,
            slots.len()
        );
        slots.truncate(MAX_VERTEX_BUFFERS);
    }
    slots
}
