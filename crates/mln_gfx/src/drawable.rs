use std::any::Any;
use std::rc::Rc;

use geometry::Point;
use mln_tile_id::OverscaledTileID;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use crate::attribute::AttributeBindingArray;
use crate::bind_group::BindGroup;
use crate::buffer::BufferPtr;
use crate::color_mode::ColorMode;
use crate::context::Context;
use crate::cull_face_mode::CullFaceMode;
use crate::depth_mode::DepthMode;
use crate::draw_mode::DrawMode;
use crate::drawable_tweaker::DrawableTweakerPtr;
use crate::gfx_types::{AttributeDataType, DrawModeType, PrimitiveType};
use crate::gpu::{
    BindGroupEntry, BindGroupId, DepthState, GpuError, PipelineId, PipelineKey, PipelineState, RenderCommand, Scissor,
    StencilState, TargetFormats, VertexAttribute as GpuVertexAttribute, VertexBufferLayout, VertexStep, Viewport,
};
use crate::index_vector::IndexVectorBasePtr;
use crate::paint_parameters::PaintParameters;
use crate::renderer::render_pass::RenderPass;
use crate::shaders::segment::SegmentBase;
use crate::shaders::shader_defines::MAX_TEXTURE_COUNT_PER_SHADER;
use crate::shaders::shader_program::{ShaderProgram, ShaderProgramPtr, same_shader};
use crate::stencil_mode::StencilMode;
use crate::texture2d::Texture2DPtr;
use crate::types::DepthMaskType;
use crate::uniform_buffer::UniformBufferArray;
use crate::upload;
use crate::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};

pub(crate) type DrawPriority = i64;

/// The textures a drawable binds, by texture id.
pub type Textures = [Option<Texture2DPtr>; MAX_TEXTURE_COUNT_PER_SHADER];

/// The bind group slots of a draw: the frame's global buffers, then its
/// layer group's, then its own buffers and textures, each winning over the
/// ones before where they bind the same id.
const GLOBAL_SLOT: u32 = 0;
const LAYER_SLOT: u32 = 1;
const DRAWABLE_SLOT: u32 = 2;

/// The most vertex buffers a draw binds, WebGPU's default limit.
const MAX_VERTEX_BUFFERS: usize = 8;

#[derive(Debug)]
pub struct DrawSegment {
    mode: DrawMode,
    segment: SegmentBase,
}

impl DrawSegment {
    #[must_use]
    pub fn new(mode: DrawMode, segment: SegmentBase) -> Self {
        DrawSegment { mode, segment }
    }
    #[must_use]
    pub fn mode(&self) -> &DrawMode {
        &self.mode
    }
    #[must_use]
    pub fn segment(&self) -> &SegmentBase {
        &self.segment
    }
    pub(crate) fn segment_mut(&mut self) -> &mut SegmentBase {
        &mut self.segment
    }
}

pub type UniqueDrawSegment = Box<DrawSegment>;

/// A vertex buffer a draw binds, and how its pipeline reads it.
struct VertexSlot {
    buffer: BufferPtr,
    offset: u64,
    layout: VertexBufferLayout,
}

/// What, besides the drawable's own state, a draw's pipeline depends on.
#[derive(Clone, Copy, PartialEq)]
struct PipelineVariant {
    primitive: DrawModeType,
    depth: DepthState,
    stencil: StencilState,
    targets: TargetFormats,
}

/// Geometry drawn with one program: its vertices, indexes and segments, the
/// buffers and textures it binds, and the state it draws with. A layer group
/// draws it by recording [`RenderCommand`]s.
pub struct Drawable {
    pub enabled: bool,
    pub enable_stencil: bool,
    pub enable_depth: bool,
    pub is_3d: bool,
    pub name: String,
    pub unique_id: SimpleIdentity,
    shader: Option<ShaderProgramPtr>,
    pub render_pass: RenderPass,
    pub tile_id: Option<OverscaledTileID>,
    pub(crate) draw_priority: DrawPriority,
    pub sub_layer_index: i32,
    pub depth_type: DepthMaskType,
    /// The stencil mode a 2D drawable draws with, in place of the mode that
    /// clips it to its tile.
    pub stencil_mode: Option<StencilMode>,
    /// Layer-specific data.
    pub(crate) drawable_data: Option<Box<dyn Any>>,
    pub vertex_attributes: Option<VertexAttributeArrayPtr>,
    pub instance_attributes: Option<VertexAttributeArrayPtr>,
    pub attribute_update_time: Option<Instant>,
    pub textures: Textures,
    pub(crate) tweakers: Vec<DrawableTweakerPtr>,
    /// Keeps the layer tweaker alive; the drawable doesn't otherwise use it.
    /// `LayerTweaker` lives in `mln_renderer`, so the handle is opaque here.
    pub(crate) layer_tweaker: Option<Rc<dyn Any>>,
    pub(crate) type_: usize,
    pub(crate) origin: Option<Point<f64>>,
    /// The index of the drawable's entries in the storage buffers of its
    /// layer group, which shaders read as `globalIndex`.
    pub ubo_index: u32,
    /// When the drawable was made, to tell whether its geometry is older
    /// than its layer's.
    pub create_time: Instant,
    color_mode: ColorMode,
    cull_face_mode: CullFaceMode,
    /// The bucket and binders handle. Buckets and binders are renderer
    /// types, so the slot is opaque here; the renderer stores its own handle
    /// and downcasts it in the tweakers.
    pub(crate) binders: Option<Rc<dyn Any>>,
    /// The render tile handle, opaque for the same reason.
    pub(crate) render_tile: Option<Rc<dyn Any>>,

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
    /// The depth and stencil modes of a 3D drawable, which its layer group
    /// sets before it draws.
    depth_mode_3d: DepthMode,
    stencil_mode_3d: StencilMode,
    /// The pipelines the drawable drew with, for its shader, vertex buffer
    /// layouts, color and cull modes as they are.
    pipelines: Vec<(PipelineVariant, PipelineId)>,
    bind_group: BindGroup,
    /// The commands of a draw, kept to reuse their allocation.
    commands: Vec<RenderCommand>,
}

impl Drawable {
    #[must_use]
    pub fn new(name: String) -> Self {
        Drawable {
            enabled: true,
            enable_stencil: false,
            enable_depth: true,
            is_3d: false,
            name,
            unique_id: SimpleIdentity::new(),
            shader: None,
            render_pass: RenderPass::Opaque,
            tile_id: None,
            draw_priority: 0,
            sub_layer_index: 0,
            depth_type: DepthMaskType::ReadOnly,
            stencil_mode: None,
            drawable_data: None,
            vertex_attributes: None,
            instance_attributes: None,
            attribute_update_time: None,
            textures: Default::default(),
            tweakers: Vec::new(),
            layer_tweaker: None,
            type_: 0,
            origin: None,
            ubo_index: 0,
            create_time: Instant::now(),
            color_mode: ColorMode::disabled(),
            cull_face_mode: CullFaceMode::disabled(),
            binders: None,
            render_tile: None,
            segments: Vec::new(),
            indexes: None,
            vertex_count: 0,
            vertex_attr_id: 0,
            attribute_bindings: Vec::new(),
            instance_bindings: Vec::new(),
            vertex_slots: None,
            uniform_buffers: UniformBufferArray::new(),
            depth_mode_3d: DepthMode::disabled(),
            stencil_mode_3d: StencilMode::default(),
            pipelines: Vec::new(),
            bind_group: BindGroup::default(),
            commands: Vec::new(),
        }
    }

    /// Uploads the drawable's indexes, vertex attributes and textures, where
    /// they changed since its last upload.
    pub fn upload(&mut self, context: &dyn Context) {
        if let Err(error) = self.upload_in(context) {
            Log::error(Event::Render, format!("Uploading drawable {} failed: {error}", self.name));
        }
    }

    fn upload_in(&mut self, context: &dyn Context) -> Result<(), GpuError> {
        let Some(shader) = self.shader.clone() else {
            debug_assert!(false, "drawable {} has no shader", self.name);
            return Ok(());
        };
        let Some(indexes) = self.indexes.clone() else {
            debug_assert!(false, "drawable {} has no indexes", self.name);
            return Ok(());
        };
        {
            let mut indexes = indexes.borrow_mut();
            if indexes.empty() && (indexes.get_buffer().is_none() || indexes.get_dirty()) {
                debug_assert!(false, "drawable {} has no index data", self.name);
                return Ok(());
            }
            upload::upload_indexes(context, &mut indexes)?;
        }

        let force = self.attribute_update_time.is_none();
        let build_attributes = match (&self.vertex_attributes, self.attribute_update_time) {
            (Some(attrs), Some(time)) => attrs.borrow().is_modified_after(time),
            _ => true,
        };
        if build_attributes {
            let attrs = Rc::clone(self.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr));
            let mut bindings =
                upload::build_attribute_bindings(context, shader.vertex_attributes(), &mut attrs.borrow_mut(), force)?;
            bind_default_values(context, &mut bindings, self.vertex_count)?;
            if self.attribute_bindings != bindings {
                self.attribute_bindings = bindings;
                self.vertex_slots = None;
                self.pipelines.clear();
            }
        }

        let build_instances = match (&self.instance_attributes, self.attribute_update_time) {
            (Some(attrs), Some(time)) => attrs.borrow().is_modified_after(time),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if build_instances && let Some(attrs) = self.instance_attributes.clone() {
            let instance_count = attrs.borrow().get_min_count();
            let mut bindings = upload::build_attribute_bindings(
                context,
                shader.instance_attributes(),
                &mut attrs.borrow_mut(),
                force,
            )?;
            bind_default_values(context, &mut bindings, instance_count)?;
            if self.instance_bindings != bindings {
                self.instance_bindings = bindings;
                self.vertex_slots = None;
                self.pipelines.clear();
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

    /// Draws the drawable into `parameters`' render pass, binding the layer
    /// group's buffers in `layer`.
    pub fn draw(&mut self, parameters: &mut dyn PaintParameters, layer: Option<BindGroupId>) {
        if !self.enabled {
            return;
        }
        if let Err(error) = self.record(parameters, layer) {
            let shader = self.shader.as_ref().map_or("no shader", |shader| shader.name());
            Log::warning(Event::Render, format!("Drawing drawable {} of {shader} failed: {error}", self.name));
        }
    }

    fn record(&mut self, parameters: &dyn PaintParameters, layer: Option<BindGroupId>) -> Result<(), GpuError> {
        let Some(shader) = self.shader.clone() else {
            debug_assert!(false, "drawable {} has no shader", self.name);
            return Ok(());
        };
        let Some(render_pass) = parameters.render_pass() else {
            return Ok(());
        };
        let Some((size, targets)) = render_pass.target() else {
            return Ok(());
        };
        let context = parameters.context();
        let Some(index_buffer) = self.indexes.as_ref().and_then(|indexes| {
            let indexes = indexes.borrow();
            debug_assert!(!indexes.get_dirty(), "drawable {} draws indexes it didn't upload", self.name);
            indexes.get_buffer().filter(|_| !indexes.get_dirty()).cloned()
        }) else {
            return Ok(());
        };

        let (depth_mode, stencil_mode) = if self.is_3d {
            (self.depth_mode_3d, self.stencil_mode_3d)
        } else {
            let depth = if self.enable_depth {
                parameters.depth_mode_for_sublayer(self.sub_layer_index as u8, self.depth_type)
            } else {
                DepthMode::disabled()
            };
            let stencil = match (self.stencil_mode, self.tile_id) {
                (Some(mode), _) => mode,
                (None, Some(tile_id)) if self.enable_stencil => {
                    parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped())
                }
                _ => StencilMode::disabled(),
            };
            (depth, stencil)
        };

        let mut entries = self.uniform_buffers.entries();
        for &id in shader.textures() {
            if let Some(texture) = self.textures.get(id).and_then(Option::as_ref) {
                let (texture, sampler) = texture.borrow_mut().binding(context)?;
                entries.push(BindGroupEntry::Texture { id: id as u32, texture, sampler });
            }
        }
        let group = self.bind_group.get(context, &self.name, entries)?;
        let global = context.global_bind_group()?;

        let slots = self
            .vertex_slots
            .get_or_insert_with(|| vertex_slots(&shader, &self.attribute_bindings, &self.instance_bindings));

        let mut commands = std::mem::take(&mut self.commands);
        commands.clear();
        commands.push(RenderCommand::SetBindGroup { slot: GLOBAL_SLOT, group: global });
        if let Some(layer) = layer {
            commands.push(RenderCommand::SetBindGroup { slot: LAYER_SLOT, group: layer });
        }
        commands.push(RenderCommand::SetBindGroup { slot: DRAWABLE_SLOT, group });
        commands.push(RenderCommand::SetGlobalIndex(self.ubo_index));
        for (slot, vertex_slot) in slots.iter().enumerate() {
            commands.push(RenderCommand::SetVertexBuffer {
                slot: slot as u32,
                buffer: vertex_slot.buffer.id(),
                offset: vertex_slot.offset,
            });
        }
        commands.push(RenderCommand::SetIndexBuffer { buffer: index_buffer.id(), offset: 0 });
        commands.push(RenderCommand::SetViewport(Viewport {
            x: 0.0,
            y: 0.0,
            width: size.width as f32,
            height: size.height as f32,
            min_depth: depth_mode.range.min,
            max_depth: depth_mode.range.max,
        }));
        if let Some(scissor) = clamped_scissor(parameters, size) {
            commands.push(RenderCommand::SetScissor(scissor));
        }
        commands.push(RenderCommand::SetStencilReference(stencil_mode.ref_ as u32));
        commands.push(RenderCommand::SetBlendConstant(self.color_mode.blend_color));

        let depth = DepthState::from(&depth_mode);
        let stencil = StencilState::from(&stencil_mode);
        let mut pipeline = None;
        let mut draws = 0;
        for segment in &self.segments {
            let (mode, segment) = (segment.mode, &segment.segment);
            if segment.index_length == 0 {
                continue;
            }
            let variant = PipelineVariant { primitive: mode.type_, depth, stencil, targets };
            let id = if let Some(&(_, id)) = self.pipelines.iter().find(|(cached, _)| *cached == variant) {
                id
            } else {
                let key = PipelineKey {
                    shader: shader.key().clone(),
                    state: pipeline_state(slots, variant, &self.color_mode, self.cull_face_mode),
                    targets,
                };
                let id = context.pipeline(&key)?;
                self.pipelines.push((variant, id));
                id
            };
            if pipeline != Some(id) {
                commands.push(RenderCommand::SetPipeline(id));
                pipeline = Some(id);
            }
            if mode.primitive_type() == PrimitiveType::Line {
                commands.push(RenderCommand::SetLineWidth(mode.size));
            }
            let index_offset = segment.index_offset as u32;
            let base_instance = segment.base_instance as u32;
            commands.push(RenderCommand::DrawIndexed {
                indices: index_offset..index_offset + segment.index_length as u32,
                base_vertex: segment.vertex_offset as i32,
                instances: base_instance..base_instance + segment.instance_count as u32,
            });
            draws += 1;
        }
        let encoded = if draws > 0 { render_pass.encode(&commands) } else { Ok(()) };
        self.commands = commands;
        encoded?;
        let mut stats = context.rendering_stats();
        stats.num_draw_calls += draws;
        stats.total_draw_calls += draws;
        Ok(())
    }

    /// Sets the depth and stencil modes a 3D drawable draws with.
    pub fn set_3d_modes(&mut self, depth: DepthMode, stencil: StencilMode) {
        self.depth_mode_3d = depth;
        self.stencil_mode_3d = stencil;
    }

    /// Replaces the vertex attributes, vertex count, indexes and segments.
    pub fn update_vertex_attributes(
        &mut self,
        vertices: Option<VertexAttributeArrayPtr>,
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
    pub fn set_vertices(&mut self, data: Vec<u8>, count: usize, data_type: AttributeDataType) {
        self.vertex_count = count;
        if count == 0 || data_type == AttributeDataType::Invalid || data.is_empty() {
            return;
        }
        let attrs = Rc::clone(self.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr));
        let mut attrs = attrs.borrow_mut();
        let Some(attribute) = attrs.set(self.vertex_attr_id, -1, data_type, 0) else {
            debug_assert!(false, "drawable {} has no vertex attribute {}", self.name, self.vertex_attr_id);
            return;
        };
        attribute.set_raw_data(data);
        attribute.set_stride(VertexAttribute::get_stride_of(data_type));
    }

    pub(crate) fn set_vertex_attr_id(&mut self, id: usize) {
        self.vertex_attr_id = id;
    }

    /// Sets the shared indexes and the segments drawn from them.
    pub fn set_index_data(&mut self, indexes: Option<IndexVectorBasePtr>, segments: Vec<UniqueDrawSegment>) {
        self.indexes = indexes;
        self.segments = segments;
    }

    /// The drawable's own uniform and storage buffers, by id.
    pub fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.uniform_buffers
    }

    #[must_use]
    pub fn get_shader(&self) -> &Option<ShaderProgramPtr> {
        &self.shader
    }

    pub fn set_shader(&mut self, value: Option<ShaderProgramPtr>) {
        if !same_shader(&self.shader, &value) {
            self.shader = value;
            self.vertex_slots = None;
            self.pipelines.clear();
        }
    }

    pub fn set_color_mode(&mut self, value: &ColorMode) {
        self.color_mode = *value;
        self.pipelines.clear();
    }

    pub fn set_cull_face_mode(&mut self, value: &CullFaceMode) {
        self.cull_face_mode = *value;
        self.pipelines.clear();
    }

    /// Attaches `texture` at texture id `id`.
    pub fn set_texture(&mut self, texture: Option<Texture2DPtr>, id: usize) {
        debug_assert!(id < self.textures.len());
        if let Some(slot) = self.textures.get_mut(id) {
            *slot = texture;
        }
    }

    #[must_use]
    pub fn get_texture(&self, id: usize) -> Option<&Texture2DPtr> {
        self.textures.get(id).and_then(Option::as_ref)
    }

    #[must_use]
    pub fn get_id(&self) -> SimpleIdentity {
        self.unique_id
    }

    #[must_use]
    pub fn get_name(&self) -> &str {
        &self.name
    }

    /// Whether the drawable draws in pass `value`.
    #[must_use]
    pub fn has_render_pass(&self, value: RenderPass) -> bool {
        (self.render_pass.underlying_type() & value.underlying_type()) != 0
    }

    #[must_use]
    pub fn get_draw_priority(&self) -> DrawPriority {
        self.draw_priority
    }

    pub fn set_draw_priority(&mut self, value: DrawPriority) {
        self.draw_priority = value;
    }

    #[must_use]
    pub fn get_tile_id(&self) -> Option<OverscaledTileID> {
        self.tile_id
    }

    pub fn set_tile_id(&mut self, value: OverscaledTileID) {
        self.tile_id = Some(value);
    }

    #[must_use]
    pub fn get_vertex_attributes(&self) -> Option<&VertexAttributeArrayPtr> {
        self.vertex_attributes.as_ref()
    }

    /// Replaces the vertex attributes, which the next upload binds again
    /// even where they appear unchanged.
    pub fn set_vertex_attributes(&mut self, value: Option<VertexAttributeArrayPtr>) {
        self.vertex_attributes = value;
        self.attribute_update_time = None;
    }

    #[must_use]
    pub fn get_tweakers(&self) -> &Vec<DrawableTweakerPtr> {
        &self.tweakers
    }

    pub fn set_tweakers(&mut self, value: Vec<DrawableTweakerPtr>) {
        self.tweakers = value;
    }

    #[must_use]
    pub fn get_data(&self) -> Option<&dyn Any> {
        self.drawable_data.as_deref()
    }

    pub fn set_data(&mut self, value: Option<Box<dyn Any>>) {
        self.drawable_data = value;
    }

    pub fn set_type(&mut self, type_: usize) {
        self.type_ = type_;
    }

    #[must_use]
    pub fn get_type(&self) -> usize {
        self.type_
    }

    /// Associates the drawable with a layer tweaker, which it keeps alive.
    pub fn set_layer_tweaker(&mut self, tweaker: Option<Rc<dyn Any>>) {
        self.layer_tweaker = tweaker;
    }

    #[must_use]
    pub fn get_layer_tweaker(&self) -> Option<&Rc<dyn Any>> {
        self.layer_tweaker.as_ref()
    }

    pub fn set_binders(&mut self, binders: Option<Rc<dyn Any>>) {
        self.binders = binders;
    }

    /// The bucket and binders handle, as the renderer stored it.
    #[must_use]
    pub fn get_binders(&self) -> Option<&Rc<dyn Any>> {
        self.binders.as_ref()
    }

    pub fn set_render_tile(&mut self, render_tile: Option<Rc<dyn Any>>) {
        self.render_tile = render_tile;
    }

    #[must_use]
    pub fn get_render_tile(&self) -> Option<&Rc<dyn Any>> {
        self.render_tile.as_ref()
    }

    #[must_use]
    pub fn get_origin(&self) -> Option<Point<f64>> {
        self.origin
    }

    pub fn set_origin(&mut self, p: Option<Point<f64>>) {
        self.origin = p;
    }
}

pub type UniqueDrawable = Box<Drawable>;

fn pipeline_state(
    slots: &[VertexSlot],
    variant: PipelineVariant,
    color_mode: &ColorMode,
    cull: CullFaceMode,
) -> PipelineState {
    PipelineState {
        vertex_buffers: slots.iter().map(|slot| slot.layout.clone()).collect(),
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
    context: &dyn Context,
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
fn clamped_scissor(parameters: &dyn PaintParameters, size: mln_util::size::Size) -> Option<Scissor> {
    let rect = parameters.scissor_rect();
    let (x, y) = (rect.x as u32, rect.y as u32);
    let width = if x < size.width { rect.width.min(size.width - x) } else { 0 };
    let height = if y < size.height { rect.height.min(size.height - y) } else { 0 };
    (width > 0 && height > 0).then_some(Scissor { x, y, width, height })
}
