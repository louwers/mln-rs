use std::rc::Rc;

use geometry::Point;
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
use mln_gfx::types::DepthMaskType;
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
use mln_tile_id::OverscaledTileID;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::time::{Instant, Instant as Time};

use crate::gfx::context::Context;
use crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker;
use crate::gfx::owned::Owned;
use crate::gfx::texture2d::Texture2DPtr;
use crate::gfx::uniform_buffer::UniformBufferArray;
use crate::gfx::upload;
use crate::layer_tweaker::LayerTweakerPtr;
use crate::layers::collision_layer_tweaker::CollisionDrawableData;
use crate::layers::color_relief_layer_tweaker::ColorReliefDrawableData;
use crate::layers::hillshade_prepare_layer_tweaker::HillshadePrepareDrawableData;
use crate::layers::line_layer_tweaker::LineDrawableData;
use crate::layers::render_symbol_layer::SymbolDrawableData;
use crate::layers::{DrawableBinders, DrawableRenderTile};
use crate::paint_parameters::PaintParameters;
use crate::render_pass::RenderPass;

pub(crate) type DrawPriority = i64;

/// What a layer keeps with each of its drawables.
#[derive(Clone, Debug)]
pub(crate) enum DrawableData {
    Collision(CollisionDrawableData),
    ColorRelief(ColorReliefDrawableData),
    HillshadePrepare(HillshadePrepareDrawableData),
    /// When a hillshade drawable's geometry was made.
    HillshadeGeometryTime(Time),
    Line(LineDrawableData),
    Symbol(SymbolDrawableData),
    /// The text lines of a debug overlay drawable.
    DebugText(Vec<String>),
}

/// The textures a drawable binds, by texture id.
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
    #[must_use]
    pub(crate) fn mode(&self) -> &DrawMode {
        &self.mode
    }
    pub(crate) fn segment_mut(&mut self) -> &mut SegmentBase {
        &mut self.segment
    }
}

pub(crate) type UniqueDrawSegment = Box<DrawSegment>;

/// A vertex buffer a draw binds, and how its pipeline reads it.
struct VertexSlot {
    buffer: BufferPtr,
    offset: u64,
    layout: VertexBufferLayout,
}

/// What a drawable's bindings were made of: the epochs of the frame's,
/// the layer group's (0 for none) and its own uniform buffers, and the
/// texture and sampler it bound at each texture id.
#[derive(Clone, Copy, PartialEq)]
struct BindingsStamp {
    global: u64,
    layer: u64,
    own: u64,
    textures: [Option<(TextureId, SamplerId)>; MAX_TEXTURE_COUNT_PER_SHADER],
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
/// draws it into the open pass of the context's `Gpu`, with bindings and
/// geometry the drawable makes there when what they bind changes.
pub(crate) struct Drawable {
    pub(crate) enabled: bool,
    pub(crate) enable_stencil: bool,
    pub(crate) enable_depth: bool,
    pub(crate) is_3d: bool,
    pub(crate) name: String,
    pub(crate) unique_id: SimpleIdentity,
    shader: Option<ShaderProgramPtr>,
    pub(crate) render_pass: RenderPass,
    pub(crate) tile_id: Option<OverscaledTileID>,
    pub(crate) draw_priority: DrawPriority,
    pub(crate) sub_layer_index: i32,
    pub(crate) depth_type: DepthMaskType,
    /// The stencil mode a 2D drawable draws with, in place of the mode that
    /// clips it to its tile.
    pub(crate) stencil_mode: Option<StencilMode>,
    /// What the drawable's layer keeps with it.
    pub(crate) data: Option<DrawableData>,
    pub(crate) vertex_attributes: Option<VertexAttributeArrayPtr>,
    pub(crate) instance_attributes: Option<VertexAttributeArrayPtr>,
    pub(crate) attribute_update_time: Option<Instant>,
    pub(crate) textures: Textures,
    /// Sets the symbol atlases' textures on the drawable before it draws.
    pub(crate) atlases: Option<Rc<DrawableAtlasesTweaker>>,
    /// The tweaker of the drawable's layer, which writes its buffers.
    pub(crate) layer_tweaker: Option<LayerTweakerPtr>,
    pub(crate) type_: usize,
    pub(crate) origin: Option<Point<f64>>,
    /// The index of the drawable's entries in the storage buffers of its
    /// layer group, which shaders read as `globalIndex`.
    pub(crate) ubo_index: u32,
    /// When the drawable was made, to tell whether its geometry is older
    /// than its layer's.
    pub(crate) create_time: Instant,
    color_mode: ColorMode,
    cull_face_mode: CullFaceMode,
    /// The bucket and the binders of its data-driven attributes.
    pub(crate) binders: Option<Rc<DrawableBinders>>,
    /// The render tile the drawable draws.
    pub(crate) render_tile: Option<DrawableRenderTile>,

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
    /// The context's shader of the drawable's, once it drew.
    shader_id: Option<ShaderId>,
    /// The pipelines the drawable drew with, for its shader,
    /// `pipeline_layouts`, color and cull modes as they are.
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

impl Drawable {
    #[must_use]
    pub(crate) fn new(name: String) -> Self {
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
            data: None,
            vertex_attributes: None,
            instance_attributes: None,
            attribute_update_time: None,
            textures: Default::default(),
            atlases: None,
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
            shader_id: None,
            pipelines: Vec::new(),
            pipeline_layouts: Vec::new(),
            bindings: None,
            geometry: None,
        }
    }

    /// Uploads the drawable's indexes, vertex attributes and textures, where
    /// they changed since its last upload.
    pub(crate) fn upload(&mut self, context: &Context) {
        if let Err(error) = self.upload_in(context) {
            Log::error(Event::Render, format!("Uploading drawable {} failed: {error}", self.name));
        }
    }

    fn upload_in(&mut self, context: &Context) -> Result<(), GpuError> {
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
                self.forget_vertex_slots();
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

    /// Draws the drawable into `parameters`' open pass on `gpu`, the
    /// context's, binding the layer group's buffers in `layer`.
    pub(crate) fn draw(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &PaintParameters<'_>,
        layer: Option<&UniformBufferArray>,
    ) {
        if !self.enabled {
            return;
        }
        if let Err(error) = self.draw_in(gpu, parameters, layer) {
            let shader = self.shader.as_ref().map_or("no shader", |shader| shader.name());
            Log::warning(Event::Render, format!("Drawing drawable {} of {shader} failed: {error}", self.name));
        }
    }

    fn draw_in(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &PaintParameters<'_>,
        layer: Option<&UniformBufferArray>,
    ) -> Result<(), GpuError> {
        let Some(shader) = self.shader.clone() else {
            debug_assert!(false, "drawable {} has no shader", self.name);
            return Ok(());
        };
        let Some((size, targets)) = parameters.pass_target() else {
            return Ok(());
        };
        let context = parameters.context;
        let Some(index_buffer) = self.indexes.as_ref().and_then(|indexes| {
            let indexes = indexes.borrow();
            debug_assert!(!indexes.get_dirty(), "drawable {} draws indexes it didn't upload", self.name);
            indexes.get_buffer().filter(|_| !indexes.get_dirty()).map(|buffer| buffer.id())
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
                global_index: self.ubo_index,
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
    /// layer group's in `layer`, the drawable's own and its textures, made
    /// again where one of them changed since the last draw.
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
            // The drawable's buffer wins over its layer group's, and that
            // over the frame's.
            let buffer = self
                .uniform_buffers
                .binding(id)
                .or_else(|| layer.and_then(|layer| layer.binding(id)))
                .or_else(|| global.binding(id));
            if let Some((buffer, offset, size)) = buffer {
                entries.push(BindGroupEntry::Buffer { id: id as u32, buffer: buffer.id(), offset, size });
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

    /// Sets the depth and stencil modes a 3D drawable draws with.
    pub(crate) fn set_3d_modes(&mut self, depth: DepthMode, stencil: StencilMode) {
        self.depth_mode_3d = depth;
        self.stencil_mode_3d = stencil;
    }

    /// Replaces the vertex attributes, vertex count, indexes and segments.
    pub(crate) fn update_vertex_attributes(
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
    pub(crate) fn set_vertices(&mut self, data: Vec<u8>, count: usize, data_type: AttributeDataType) {
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
    pub(crate) fn set_index_data(&mut self, indexes: Option<IndexVectorBasePtr>, segments: Vec<UniqueDrawSegment>) {
        self.indexes = indexes;
        self.segments = segments;
    }

    /// The drawable's own uniform and storage buffers, by id.
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

    /// Whether the drawable draws in pass `value`.
    #[must_use]
    pub(crate) fn has_render_pass(&self, value: RenderPass) -> bool {
        (self.render_pass.underlying_type() & value.underlying_type()) != 0
    }

    /// Replaces the vertex attributes, which the next upload binds again
    /// even where they appear unchanged.
    pub(crate) fn set_vertex_attributes(&mut self, value: Option<VertexAttributeArrayPtr>) {
        self.vertex_attributes = value;
        self.attribute_update_time = None;
    }
}

pub(crate) type UniqueDrawable = Box<Drawable>;

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
fn clamped_scissor(parameters: &PaintParameters<'_>, size: mln_util::size::Size) -> Option<Scissor> {
    let rect = parameters.scissor_rect;
    let (x, y) = (rect.x as u32, rect.y as u32);
    let width = if x < size.width { rect.width.min(size.width - x) } else { 0 };
    let height = if y < size.height { rect.height.min(size.height - y) } else { 0 };
    (width > 0 && height > 0).then_some(Scissor { x, y, width, height })
}
