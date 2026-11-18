//! The GPU objects of a draw, which the renderer keeps between frames and
//! makes again only when what they are made of changes.

use std::rc::Rc;

use mln_gfx::attribute::AttributeBindingArray;
use mln_gfx::buffer::BufferPtr;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType, PrimitiveType};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferId, DepthState, Draw, DynamicState, FixedState, GeometryDesc,
    GeometryId, Gpu, GpuError, PipelineId, PipelineKey, SamplerId, Scissor, ShaderId, StencilState, TargetFormats,
    TextureId, VertexAttribute as GpuVertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{MAX_TEXTURE_COUNT_PER_SHADER, MAX_UBO_COUNT_PER_SHADER};
use mln_gfx::shaders::shader_program::{ShaderProgram, ShaderProgramPtr};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray};
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use crate::gfx::context::Context;
use crate::gfx::indexes::Indexes;
use crate::gfx::owned::Owned;
use crate::gfx::texture2d::TextureKey;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::gfx::upload;
use crate::paint_parameters::PaintParameters;

/// The textures a draw binds, by texture id.
pub(crate) type Textures = [Option<TextureKey>; MAX_TEXTURE_COUNT_PER_SHADER];

/// The most vertex buffers a draw binds, WebGPU's default limit.
const MAX_VERTEX_BUFFERS: usize = 8;

/// What a draw draws: its vertices, and the primitives its segments draw of
/// them.
pub(crate) struct DrawGeometry {
    /// The vertex attributes, by attribute id.
    pub(crate) vertices: VertexAttributeArray,
    /// How many vertices the attributes hold, which an attribute the
    /// shader reads and the draw lacks gets default values for.
    pub(crate) vertex_count: usize,
    /// The primitive the segments draw.
    pub(crate) mode: DrawMode,
    pub(crate) indexes: Indexes,
    /// The ranges of the indexes and vertices each draw call draws.
    pub(crate) segments: Vec<SegmentBase>,
    /// The attributes of each instance, for a draw of instances.
    pub(crate) instances: Option<VertexAttributeArray>,
}

/// `count` vertices of `data_type` in `data` as the vertex attribute `id`,
/// none where there are no vertices.
#[must_use]
pub(crate) fn raw_vertices(
    id: usize,
    data: Vec<u8>,
    count: usize,
    data_type: AttributeDataType,
) -> VertexAttributeArray {
    let mut attributes = VertexAttributeArray::new();
    if count == 0 || data_type == AttributeDataType::Invalid || data.is_empty() {
        return attributes;
    }
    if let Some(attribute) = attributes.set(id, -1, data_type, 0) {
        attribute.set_raw_data(data);
        attribute.set_stride(VertexAttribute::get_stride_of(data_type));
    }
    attributes
}

/// A vertex buffer a draw binds, and how its pipeline reads it.
struct VertexSlot {
    buffer: BufferPtr,
    offset: u64,
    layout: VertexBufferLayout,
}

/// What a draw's bindings were made of: the epochs of the global uniform
/// buffers, those of its frame's draws and its own, and the texture and
/// sampler it bound at each texture id.
#[derive(Clone, Copy, PartialEq)]
struct BindingsStamp {
    global: u64,
    frame: u64,
    own: u64,
    textures: [Option<(TextureId, SamplerId)>; MAX_TEXTURE_COUNT_PER_SHADER],
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
/// vertices, indexes and segments, the buffers and textures it binds, the
/// color and cull modes it draws with, and the shader, pipelines, bindings
/// and geometry it made of them on the context's `Gpu`, which it makes again
/// when what they are made of changes.
pub(crate) struct DrawObjects {
    shader: ShaderProgramPtr,
    vertex_attributes: VertexAttributeArray,
    /// The attributes of each instance, for a draw of instances.
    instance_attributes: Option<VertexAttributeArray>,
    /// When the attributes last uploaded, `None` until they upload, which
    /// binds them all again.
    attribute_update_time: Option<Instant>,
    /// Whether the vertex attributes were replaced since the last upload,
    /// which then binds them again.
    vertex_attributes_replaced: bool,
    /// The textures it binds, by texture id.
    textures: Textures,
    /// The draw's own uniform and storage buffers, by id, which win over
    /// its layer's and the frame's.
    pub(crate) uniform_buffers: UniformBufferArray,
    color_mode: ColorMode,
    cull_face_mode: CullFaceMode,

    mode: DrawMode,
    segments: Vec<SegmentBase>,
    indexes: Indexes,
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
    /// The objects of a draw of `geometry` with `shader`, in `color_mode`
    /// and `cull_face_mode`, binding `textures`, without buffers of its own.
    #[must_use]
    pub(crate) fn new(
        shader: ShaderProgramPtr,
        color_mode: ColorMode,
        cull_face_mode: CullFaceMode,
        geometry: DrawGeometry,
        textures: Textures,
    ) -> Self {
        let DrawGeometry { vertices, vertex_count, mode, indexes, segments, instances } = geometry;
        DrawObjects {
            shader,
            vertex_attributes: vertices,
            instance_attributes: instances,
            attribute_update_time: None,
            vertex_attributes_replaced: false,
            textures,
            uniform_buffers: UniformBufferArray::new(),
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

    /// Uploads the indexes, vertex attributes and textures, where they
    /// changed since the last upload.
    pub(crate) fn upload(&mut self, context: &mut Context) {
        if let Err(error) = self.upload_in(context) {
            Log::error(Event::Render, format!("Uploading a draw of {} failed: {error}", self.shader.name()));
        }
    }

    fn upload_in(&mut self, context: &mut Context) -> Result<(), GpuError> {
        let shader = &*self.shader;
        if self.indexes.is_empty() && self.indexes.buffer().is_none() {
            debug_assert!(false, "a draw of {} has no index data", shader.name());
            return Ok(());
        }
        self.indexes.upload(context)?;

        let force = self.attribute_update_time.is_none();
        let update_time = self.attribute_update_time;
        let modified = move |attrs: &VertexAttributeArray| update_time.is_none_or(|time| attrs.is_modified_after(time));
        if std::mem::take(&mut self.vertex_attributes_replaced) || modified(&self.vertex_attributes) {
            let mut bindings = upload::build_attribute_bindings(
                context,
                shader.vertex_attributes(),
                &mut self.vertex_attributes,
                force,
            )?;
            bind_default_values(context, &mut bindings, self.vertex_count)?;
            if self.attribute_bindings != bindings {
                self.attribute_bindings = bindings;
                self.vertex_slots = None;
                self.geometry = None;
            }
        }

        if let Some(attrs) = self.instance_attributes.as_mut().filter(|attrs| modified(attrs)) {
            let instance_count = attrs.get_min_count();
            let mut bindings = upload::build_attribute_bindings(context, shader.instance_attributes(), attrs, force)?;
            bind_default_values(context, &mut bindings, instance_count)?;
            if self.instance_bindings != bindings {
                self.instance_bindings = bindings;
                self.vertex_slots = None;
                self.geometry = None;
            }
        }

        for &texture in self.textures.iter().flatten() {
            if let Err(error) = context.upload_texture(texture) {
                Log::error(Event::Render, format!("Uploading a texture failed: {error}"));
            }
        }

        self.attribute_update_time = Some(Instant::now());
        Ok(())
    }

    /// Draws the segments into `parameters`' open pass on `gpu`, the
    /// context's, with `depth_mode` and `stencil_mode`, binding the buffers
    /// of the draws in `frame`. Shaders read the draw's entries in those
    /// storage buffers at `global_index`.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        frame: &UniformBufferArray,
        depth_mode: DepthMode,
        stencil_mode: StencilMode,
        global_index: u32,
    ) {
        if let Err(error) = self.draw_in(gpu, parameters, frame, depth_mode, stencil_mode, global_index) {
            Log::warning(Event::Render, format!("Drawing a draw of {} failed: {error}", self.shader.name()));
        }
    }

    fn draw_in(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        frame: &UniformBufferArray,
        depth_mode: DepthMode,
        stencil_mode: StencilMode,
        global_index: u32,
    ) -> Result<(), GpuError> {
        let Some((size, targets)) = parameters.pass_target() else {
            return Ok(());
        };
        let scissor = clamped_scissor(parameters, size);
        let context = &mut *parameters.context;
        let index_buffer = self.indexes.buffer().map(|buffer| buffer.id());
        debug_assert!(index_buffer.is_some(), "a draw of {} draws indexes it didn't upload", self.shader.name());
        let Some(index_buffer) = index_buffer else {
            return Ok(());
        };

        let shader_id = match self.shader_id {
            Some(id) => id,
            None => *self.shader_id.insert(context.shader(gpu, self.shader.key())?),
        };
        let bindings = self.bindings(gpu, context, shader_id, frame)?;
        let slots = if let Some(slots) = &self.vertex_slots {
            slots
        } else {
            let slots = vertex_slots(&self.shader, &self.attribute_bindings, &self.instance_bindings);
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
                let buffers: Vec<(BufferId, u64)> = slots.iter().map(|slot| (slot.buffer.id(), slot.offset)).collect();
                let geometry = gpu.create_geometry(&GeometryDesc {
                    shader: shader_id,
                    layouts: &self.pipeline_layouts,
                    vertex_buffers: &buffers,
                    index_buffer,
                })?;
                self.geometry = Some((Owned::new(geometry, context.released()), index_buffer));
                geometry
            }
        };

        let full = Scissor { x: 0, y: 0, width: size.width, height: size.height };
        let dynamic = DynamicState {
            depth_range: (depth_mode.range.min, depth_mode.range.max),
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

    /// The bindings of what the shader, `shader_id` on `gpu`, reads of the
    /// context's global buffers, the buffers of the draws in `frame`, the
    /// draw's own and its textures, made again where one of them changed
    /// since the last draw.
    fn bindings(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        shader_id: ShaderId,
        frame: &UniformBufferArray,
    ) -> Result<BindingsId, GpuError> {
        let mut textures = [None; MAX_TEXTURE_COUNT_PER_SHADER];
        for &id in self.shader.textures() {
            if let Some(&texture) = self.textures.get(id).and_then(Option::as_ref) {
                textures[id] = Some(context.texture_binding(gpu, texture)?);
            }
        }
        let global = context.global_uniform_buffers();
        let stamp =
            BindingsStamp { global: global.epoch(), frame: frame.epoch(), own: self.uniform_buffers.epoch(), textures };
        if let Some((bindings, made)) = &self.bindings
            && *made == stamp
        {
            return Ok(bindings.id());
        }
        self.bindings = None;
        let mut entries = Vec::with_capacity(MAX_UBO_COUNT_PER_SHADER + MAX_TEXTURE_COUNT_PER_SHADER);
        for id in 0..MAX_UBO_COUNT_PER_SHADER {
            // The draw's buffer wins over its frame's, and that over the
            // global one.
            let buffer = self.uniform_buffers.get(id).or_else(|| frame.get(id)).or_else(|| global.get(id));
            if let Some(buffer) = buffer {
                entries.push(BindGroupEntry::Buffer {
                    id: id as u32,
                    buffer: buffer.id(),
                    offset: 0,
                    size: buffer.size() as u64,
                });
            }
        }
        for (id, binding) in textures.iter().enumerate() {
            if let Some((texture, sampler)) = *binding {
                entries.push(BindGroupEntry::Texture { id: id as u32, texture, sampler });
            }
        }
        let bindings =
            gpu.create_bindings(&BindingsDesc { label: self.shader.name(), shader: shader_id, entries: &entries })?;
        self.bindings = Some((Owned::new(bindings, context.released()), stamp));
        Ok(bindings)
    }

    /// Replaces the geometry, whose attributes the next upload binds again
    /// even where they appear unchanged.
    pub(crate) fn replace_geometry(&mut self, geometry: DrawGeometry) {
        let DrawGeometry { vertices, vertex_count, mode, indexes, segments, instances } = geometry;
        self.vertex_attributes = vertices;
        self.instance_attributes = instances;
        self.attribute_update_time = None;
        self.vertex_count = vertex_count;
        self.mode = mode;
        self.indexes = indexes;
        self.segments = segments;
    }

    /// The vertex attributes, to change their data in place, which the next
    /// upload binds where it changed.
    pub(crate) fn vertex_attributes_mut(&mut self) -> &mut VertexAttributeArray {
        &mut self.vertex_attributes
    }

    /// The attributes the paint values are read into: the instance
    /// attributes of a draw of instances, else the vertex attributes.
    pub(crate) fn paint_attributes_mut(&mut self) -> &mut VertexAttributeArray {
        self.instance_attributes.as_mut().unwrap_or(&mut self.vertex_attributes)
    }

    /// Whether the draw draws instances.
    #[must_use]
    pub(crate) fn has_instances(&self) -> bool {
        self.instance_attributes.is_some()
    }

    /// Replaces the vertex attributes with a copy of those of `other`
    /// ([`copy_shared_attributes`]), which the next upload binds.
    pub(crate) fn share_vertex_attributes(&mut self, other: &DrawObjects) {
        self.vertex_attributes = copy_shared_attributes(&other.vertex_attributes);
        self.vertex_attributes_replaced = true;
    }

    /// The texture it binds as `id`.
    #[must_use]
    pub(crate) fn texture(&self, id: usize) -> Option<TextureKey> {
        self.textures[id]
    }

    /// Makes `texture` the one it binds as `id`.
    pub(crate) fn set_texture(&mut self, id: usize, texture: Option<TextureKey>) {
        self.textures[id] = texture;
    }

    /// Makes `textures` those it binds.
    pub(crate) fn set_textures(&mut self, textures: Textures) {
        self.textures = textures;
    }

    /// The texture it binds as `id`, which `make` makes where it has none;
    /// `None` where it has none and `make` returns `None`.
    pub(crate) fn texture_or_insert_with(
        &mut self,
        id: usize,
        make: impl FnOnce() -> Option<TextureKey>,
    ) -> Option<TextureKey> {
        let texture = &mut self.textures[id];
        if texture.is_none() {
            *texture = make();
        }
        *texture
    }

    /// Replaces the indexes and the segments drawn from them.
    pub(crate) fn replace_indexes(&mut self, indexes: Indexes, segments: Vec<SegmentBase>) {
        self.indexes = indexes;
        self.segments = segments;
    }

    #[must_use]
    pub(crate) fn shader(&self) -> &ShaderProgramPtr {
        &self.shader
    }
}

/// A copy of `attrs`, none of whose attributes has data of its own: the
/// copy reads the same shared vertex vectors, so draws of either bind the
/// same buffers.
#[must_use]
pub(crate) fn copy_shared_attributes(attrs: &VertexAttributeArray) -> VertexAttributeArray {
    let mut copy = VertexAttributeArray::new();
    for id in 0..attrs.allocated_size() {
        let Some(attr) = attrs.get(id) else {
            continue;
        };
        debug_assert!(
            attr.get_shared_raw_data().is_some() || attr.get_raw_data().is_empty(),
            "a copied attribute has data of its own"
        );
        if let Some(copied) = copy.set(id, attr.get_index(), attr.get_data_type(), 0) {
            copied.set_shared_raw_data(
                attr.get_shared_raw_data().cloned(),
                attr.get_shared_offset(),
                attr.get_shared_vertex_offset(),
                attr.get_shared_stride(),
                attr.get_shared_type(),
            );
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

/// Gives each binding without data a buffer of its format's default value,
/// for `vertex_count` vertices.
fn bind_default_values(
    context: &mut Context,
    bindings: &mut AttributeBindingArray,
    vertex_count: usize,
) -> Result<(), GpuError> {
    for binding in bindings.iter_mut().flatten().filter(|binding| binding.buffer.is_none()) {
        binding.buffer = Some(context.default_vertex_buffer(binding.attribute.data_type, vertex_count)?);
    }
    Ok(())
}

/// The vertex buffers of a draw's attribute and instance bindings, which
/// hold an attribute for each of the shader's in the shader's order.
/// Attributes that read one buffer with one stride from one offset share a
/// slot; attributes of a buffer at different vertex offsets (a fill
/// extrusion wall's outline vertex and the next one) read it through slots
/// of their own, since an attribute's offset must stay within the stride.
fn vertex_slots(
    shader: &ShaderProgram,
    attributes: &AttributeBindingArray,
    instances: &AttributeBindingArray,
) -> Vec<VertexSlot> {
    let mut slots: Vec<VertexSlot> = Vec::new();
    for (bindings, step) in [(attributes, VertexStep::Vertex), (instances, VertexStep::Instance)] {
        for (index, binding) in bindings.iter().enumerate() {
            let Some(binding) = binding else { continue };
            let Some(buffer) = &binding.buffer else { continue };
            let Some(id) = shader.attribute_id(step, index) else { continue };
            let offset = u64::from(binding.vertex_offset) * u64::from(binding.vertex_stride);
            let attribute = GpuVertexAttribute {
                id: id as u32,
                format: binding.attribute.data_type,
                offset: binding.attribute.offset,
            };
            let shared = slots.iter_mut().find(|slot| {
                slot.buffer.id() == buffer.id()
                    && slot.layout.stride == binding.vertex_stride
                    && slot.layout.step == step
                    && slot.offset == offset
            });
            match shared {
                Some(slot) => slot.layout.attributes.push(attribute),
                None => slots.push(VertexSlot {
                    buffer: Rc::clone(buffer),
                    offset,
                    layout: VertexBufferLayout { stride: binding.vertex_stride, step, attributes: vec![attribute] },
                }),
            }
        }
    }
    if slots.len() > MAX_VERTEX_BUFFERS {
        Log::warning(
            Event::Render,
            format!("{} reads {} vertex buffers, past the {MAX_VERTEX_BUFFERS} a draw may", shader.name(), slots.len()),
        );
        slots.truncate(MAX_VERTEX_BUFFERS);
    }
    slots
}

/// The paint parameters' scissor rect within a target of `size`, `None`
/// where nothing of it is.
pub(crate) fn clamped_scissor(parameters: &PaintParameters<'_>, size: mln_util::size::Size) -> Option<Scissor> {
    let rect = parameters.scissor_rect;
    let (x, y) = (rect.x as u32, rect.y as u32);
    let width = if x < size.width { rect.width.min(size.width - x) } else { 0 };
    let height = if y < size.height { rect.height.min(size.height - y) } else { 0 };
    (width > 0 && height > 0).then_some(Scissor { x, y, width, height })
}
