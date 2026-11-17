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
use mln_gfx::index_vector::IndexVectorBasePtr;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{MAX_TEXTURE_COUNT_PER_SHADER, MAX_UBO_COUNT_PER_SHADER};
use mln_gfx::shaders::shader_program::{ShaderProgram, ShaderProgramPtr, same_shader};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray};
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use crate::gfx::context::Context;
use crate::gfx::owned::Owned;
use crate::gfx::texture2d::Texture2DPtr;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::gfx::upload;
use crate::paint_parameters::PaintParameters;

/// The textures a draw binds, by texture id.
pub(crate) type Textures = [Option<Texture2DPtr>; MAX_TEXTURE_COUNT_PER_SHADER];

/// The most vertex buffers a draw binds, WebGPU's default limit.
const MAX_VERTEX_BUFFERS: usize = 8;

#[derive(Debug)]
pub(crate) struct DrawSegment {
    mode: DrawMode,
    segment: SegmentBase,
}

impl DrawSegment {
    #[must_use]
    pub(crate) fn new(mode: DrawMode, segment: SegmentBase) -> Self {
        DrawSegment { mode, segment }
    }
}

pub(crate) type UniqueDrawSegment = Box<DrawSegment>;

/// A vertex buffer a draw binds, and how its pipeline reads it.
struct VertexSlot {
    buffer: BufferPtr,
    offset: u64,
    layout: VertexBufferLayout,
}

/// What a draw's bindings were made of: the epochs of the frame's, the
/// layer's (0 for none) and its own uniform buffers, and the texture and
/// sampler it bound at each texture id.
#[derive(Clone, Copy, PartialEq)]
struct BindingsStamp {
    global: u64,
    layer: u64,
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
    pub(crate) name: String,
    shader: Option<ShaderProgramPtr>,
    pub(crate) vertex_attributes: VertexAttributeArray,
    pub(crate) instance_attributes: Option<VertexAttributeArray>,
    pub(crate) attribute_update_time: Option<Instant>,
    pub(crate) textures: Textures,
    color_mode: ColorMode,
    cull_face_mode: CullFaceMode,

    segments: Vec<UniqueDrawSegment>,
    indexes: Option<IndexVectorBasePtr>,
    vertex_count: usize,
    vertex_attr_id: usize,
    attribute_bindings: AttributeBindingArray,
    instance_bindings: AttributeBindingArray,
    /// The vertex buffers of the bindings, `None` until a draw after either
    /// changes computes them.
    vertex_slots: Option<Vec<VertexSlot>>,
    uniform_buffers: UniformBufferArray,
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
    #[must_use]
    pub(crate) fn new(name: String) -> Self {
        DrawObjects {
            name,
            shader: None,
            vertex_attributes: VertexAttributeArray::new(),
            instance_attributes: None,
            attribute_update_time: None,
            textures: Default::default(),
            color_mode: ColorMode::disabled(),
            cull_face_mode: CullFaceMode::disabled(),
            segments: Vec::new(),
            indexes: None,
            vertex_count: 0,
            vertex_attr_id: 0,
            attribute_bindings: Vec::new(),
            instance_bindings: Vec::new(),
            vertex_slots: None,
            uniform_buffers: UniformBufferArray::new(),
            shader_id: None,
            pipelines: Vec::new(),
            pipeline_layouts: Vec::new(),
            bindings: None,
            geometry: None,
        }
    }

    /// Uploads the indexes, vertex attributes and textures, where they
    /// changed since the last upload.
    pub(crate) fn upload(&mut self, context: &Context) {
        if let Err(error) = self.upload_in(context) {
            Log::error(Event::Render, format!("Uploading draw {} failed: {error}", self.name));
        }
    }

    fn upload_in(&mut self, context: &Context) -> Result<(), GpuError> {
        let Some(shader) = self.shader.clone() else {
            debug_assert!(false, "draw {} has no shader", self.name);
            return Ok(());
        };
        let Some(indexes) = self.indexes.clone() else {
            debug_assert!(false, "draw {} has no indexes", self.name);
            return Ok(());
        };
        {
            let mut indexes = indexes.borrow_mut();
            if indexes.empty() && (indexes.get_buffer().is_none() || indexes.get_dirty()) {
                debug_assert!(false, "draw {} has no index data", self.name);
                return Ok(());
            }
            upload::upload_indexes(context, &mut indexes)?;
        }

        let force = self.attribute_update_time.is_none();
        let update_time = self.attribute_update_time;
        let modified = move |attrs: &VertexAttributeArray| update_time.is_none_or(|time| attrs.is_modified_after(time));
        if modified(&self.vertex_attributes) {
            let mut bindings = upload::build_attribute_bindings(
                context,
                shader.vertex_attributes(),
                &mut self.vertex_attributes,
                force,
            )?;
            bind_default_values(context, &mut bindings, self.vertex_count)?;
            if self.attribute_bindings != bindings {
                self.attribute_bindings = bindings;
                self.forget_vertex_slots();
            }
        }

        if let Some(attrs) = self.instance_attributes.as_mut().filter(|attrs| modified(attrs)) {
            let instance_count = attrs.get_min_count();
            let mut bindings = upload::build_attribute_bindings(context, shader.instance_attributes(), attrs, force)?;
            bind_default_values(context, &mut bindings, instance_count)?;
            if self.instance_bindings != bindings {
                self.instance_bindings = bindings;
                self.forget_vertex_slots();
            }
        }

        for texture in self.textures.iter().flatten() {
            if let Err(error) = texture.borrow_mut().upload(context) {
                Log::error(Event::Render, format!("Uploading a texture failed: {error}"));
            }
        }

        self.attribute_update_time = Some(Instant::now());
        Ok(())
    }

    /// Draws the segments into `parameters`' open pass on `gpu`, the
    /// context's, with `depth_mode` and `stencil_mode`, binding the layer's
    /// buffers in `layer`. Shaders read the draw's entries in the layer's
    /// storage buffers at `global_index`.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &PaintParameters<'_>,
        layer: Option<&UniformBufferArray>,
        depth_mode: DepthMode,
        stencil_mode: StencilMode,
        global_index: u32,
    ) {
        if let Err(error) = self.draw_in(gpu, parameters, layer, depth_mode, stencil_mode, global_index) {
            let shader = self.shader.as_ref().map_or("no shader", |shader| shader.name());
            Log::warning(Event::Render, format!("Drawing draw {} of {shader} failed: {error}", self.name));
        }
    }

    fn draw_in(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &PaintParameters<'_>,
        layer: Option<&UniformBufferArray>,
        depth_mode: DepthMode,
        stencil_mode: StencilMode,
        global_index: u32,
    ) -> Result<(), GpuError> {
        let Some(shader) = self.shader.clone() else {
            debug_assert!(false, "draw {} has no shader", self.name);
            return Ok(());
        };
        let Some((size, targets)) = parameters.pass_target() else {
            return Ok(());
        };
        let context = parameters.context;
        let Some(index_buffer) = self.indexes.as_ref().and_then(|indexes| {
            let indexes = indexes.borrow();
            debug_assert!(!indexes.get_dirty(), "draw {} draws indexes it didn't upload", self.name);
            indexes.get_buffer().filter(|_| !indexes.get_dirty()).map(|buffer| buffer.id())
        }) else {
            return Ok(());
        };

        let shader_id = match self.shader_id {
            Some(id) => id,
            None => *self.shader_id.insert(context.shader(gpu, shader.key())?),
        };
        let bindings = self.bindings(gpu, context, &shader, shader_id, layer)?;
        let slots = if let Some(slots) = &self.vertex_slots {
            slots
        } else {
            let slots = vertex_slots(&shader, &self.attribute_bindings, &self.instance_bindings);
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
            scissor: clamped_scissor(parameters, size).unwrap_or(full),
            stencil_reference: stencil_mode.ref_ as u32,
            blend_constant: self.color_mode.blend_color,
            line_width: 1.0,
        };
        let depth = DepthState::from(&depth_mode);
        let stencil = StencilState::from(&stencil_mode);
        let mut draws = 0;
        for segment in &self.segments {
            let (mode, segment) = (segment.mode, &segment.segment);
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
        let mut stats = context.rendering_stats();
        stats.num_draw_calls += draws;
        stats.total_draw_calls += draws;
        Ok(())
    }

    /// The bindings of what `shader` reads of the frame's buffers, the
    /// layer's in `layer`, the draw's own and its textures, made again where
    /// one of them changed since the last draw.
    fn bindings(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &Context,
        shader: &ShaderProgram,
        shader_id: ShaderId,
        layer: Option<&UniformBufferArray>,
    ) -> Result<BindingsId, GpuError> {
        let mut textures = [None; MAX_TEXTURE_COUNT_PER_SHADER];
        for &id in shader.textures() {
            if let Some(texture) = self.textures.get(id).and_then(Option::as_ref) {
                textures[id] = Some(texture.borrow_mut().binding(gpu, context)?);
            }
        }
        let global = context.global_uniform_buffers();
        let stamp = BindingsStamp {
            global: global.epoch(),
            layer: layer.map_or(0, UniformBufferArray::epoch),
            own: self.uniform_buffers.epoch(),
            textures,
        };
        if let Some((bindings, made)) = &self.bindings
            && *made == stamp
        {
            return Ok(bindings.id());
        }
        self.bindings = None;
        let mut entries = Vec::with_capacity(MAX_UBO_COUNT_PER_SHADER + MAX_TEXTURE_COUNT_PER_SHADER);
        for id in 0..MAX_UBO_COUNT_PER_SHADER {
            // The draw's buffer wins over its layer's, and that over the
            // frame's.
            let buffer = self
                .uniform_buffers
                .get(id)
                .or_else(|| layer.and_then(|layer| layer.get(id)))
                .or_else(|| global.get(id));
            if let Some(buffer) = buffer {
                entries.push(BindGroupEntry::Buffer {
                    id: id as u32,
                    buffer: buffer.id(),
                    offset: 0,
                    size: buffer.size() as u64,
                });
            }
        }
        drop(global);
        for (id, binding) in textures.iter().enumerate() {
            if let Some((texture, sampler)) = *binding {
                entries.push(BindGroupEntry::Texture { id: id as u32, texture, sampler });
            }
        }
        let bindings =
            gpu.create_bindings(&BindingsDesc { label: &self.name, shader: shader_id, entries: &entries })?;
        self.bindings = Some((Owned::new(bindings, context.released()), stamp));
        Ok(bindings)
    }

    /// Drops the vertex slots and the geometry of them.
    fn forget_vertex_slots(&mut self) {
        self.vertex_slots = None;
        self.geometry = None;
    }

    /// Replaces the vertex attributes, vertex count, indexes and segments.
    pub(crate) fn update_vertex_attributes(
        &mut self,
        vertices: VertexAttributeArray,
        vertex_count: usize,
        mode: DrawMode,
        indexes: Option<IndexVectorBasePtr>,
        segments: &[SegmentBase],
    ) {
        self.set_vertex_attributes(vertices);
        self.vertex_count = vertex_count;
        let segments = segments.iter().map(|segment| Box::new(DrawSegment::new(mode, segment.copy()))).collect();
        self.set_index_data(indexes, segments);
    }

    /// Takes `data`, `count` vertices of `data_type`, as the attribute the
    /// builder named the vertex attribute.
    pub(crate) fn set_vertices(&mut self, data: Vec<u8>, count: usize, data_type: AttributeDataType) {
        self.vertex_count = count;
        if count == 0 || data_type == AttributeDataType::Invalid || data.is_empty() {
            return;
        }
        let Some(attribute) = self.vertex_attributes.set(self.vertex_attr_id, -1, data_type, 0) else {
            debug_assert!(false, "draw {} has no vertex attribute {}", self.name, self.vertex_attr_id);
            return;
        };
        attribute.set_raw_data(data);
        attribute.set_stride(VertexAttribute::get_stride_of(data_type));
    }

    pub(crate) fn set_vertex_attr_id(&mut self, id: usize) {
        self.vertex_attr_id = id;
    }

    /// Sets the shared indexes and the segments drawn from them.
    pub(crate) fn set_index_data(&mut self, indexes: Option<IndexVectorBasePtr>, segments: Vec<UniqueDrawSegment>) {
        self.indexes = indexes;
        self.segments = segments;
    }

    /// The draw's own uniform and storage buffers, by id.
    pub(crate) fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.uniform_buffers
    }

    #[must_use]
    pub(crate) fn shader(&self) -> Option<&ShaderProgramPtr> {
        self.shader.as_ref()
    }

    pub(crate) fn set_shader(&mut self, value: Option<ShaderProgramPtr>) {
        if !same_shader(&self.shader, &value) {
            self.shader = value;
            self.shader_id = None;
            self.bindings = None;
            self.pipelines.clear();
            self.pipeline_layouts.clear();
            self.forget_vertex_slots();
        }
    }

    pub(crate) fn set_color_mode(&mut self, value: &ColorMode) {
        self.color_mode = *value;
        self.pipelines.clear();
    }

    pub(crate) fn set_cull_face_mode(&mut self, value: CullFaceMode) {
        self.cull_face_mode = value;
        self.pipelines.clear();
    }

    /// Attaches `texture` at texture id `id`.
    pub(crate) fn set_texture(&mut self, texture: Option<Texture2DPtr>, id: usize) {
        debug_assert!(id < self.textures.len());
        if let Some(slot) = self.textures.get_mut(id) {
            *slot = texture;
        }
    }

    /// Replaces the vertex attributes, which the next upload binds again
    /// even where they appear unchanged.
    pub(crate) fn set_vertex_attributes(&mut self, value: VertexAttributeArray) {
        self.vertex_attributes = value;
        self.attribute_update_time = None;
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
    context: &Context,
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
