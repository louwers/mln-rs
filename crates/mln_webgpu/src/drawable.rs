use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::attribute::AttributeBindingArray;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::drawable::{self, DrawSegment, DrawableBase, UniqueDrawSegment};
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{
    DepthState, PipelineState, StencilState, VertexAttribute as GpuVertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::index_vector::{IndexBufferBase, IndexVectorBasePtr};
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBasePtr, same_shader};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::{Texture2D as _, Texture2DPtr};
use mln_gfx::types::{BufferUsageType, DepthMaskType};
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_gfx::upload_pass::UploadPass as _;
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
use mln_gfx::vertex_buffer::IndexBuffer as GfxIndexBuffer;
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use crate::bind_groups::{BindGroupKey, EntryResource, SharedBindGroup};
use crate::context::Context;
use crate::render_pass::RenderPass;
use crate::renderer_backend::BackendImpl;
use crate::shaders::shader_program::{BindingInfo, BindingType, ShaderProgram, as_webgpu_shader};
use crate::texture2d::Texture2D;
use crate::upload_pass::UploadPass;
use crate::vertex_buffer_resource::{IndexBufferResource, VertexBufferResource};

/// The uploaded buffer of an index vector.
struct IndexBuffer {
    buffer: GfxIndexBuffer,
}

impl IndexBufferBase for IndexBuffer {}

struct BindGroupRecord {
    slot: u32,
    group: u32,
    /// A later draw that binds the same layout and resources reuses it
    /// instead of looking it up.
    bind_group: Rc<SharedBindGroup>,
}

struct DrawableImpl {
    segments: Vec<UniqueDrawSegment>,
    indexes: Option<IndexVectorBasePtr>,
    vertex_count: usize,
    vertex_type: AttributeDataType,
    attribute_bindings: AttributeBindingArray,
    instance_bindings: AttributeBindingArray,
    /// The vertex buffer slots of [`attribute_bindings`] and
    /// [`instance_bindings`], `None` until a draw after either changes
    /// computes them.
    ///
    /// [`attribute_bindings`]: Self::attribute_bindings
    /// [`instance_bindings`]: Self::instance_bindings
    vertex_slots: Option<Vec<UniqueBinding>>,
    uniform_buffers: UniformBufferArray,
    depth_mode: DepthMode,
    stencil_mode: StencilMode,
    /// The stencil state the pipeline was made with, `None` until it is.
    pipeline_stencil: Option<StencilState>,
    vertex_attr_id: usize,
    /// The pipeline, which the shader program's cache owns, and the target
    /// formats it was made for.
    pipeline_state: Option<wgpu::RenderPipeline>,
    pipeline_formats: Option<(wgpu::TextureFormat, Option<wgpu::TextureFormat>)>,
    bind_groups: Vec<BindGroupRecord>,
    /// The slot of the shader's index group, which [`bind_groups`] leaves
    /// out: a draw binds the context's table at its `ubo_index`.
    ///
    /// [`bind_groups`]: Self::bind_groups
    index_slot: Option<u32>,
    /// The shader whose groups [`bind_groups`] holds, `None` until a draw
    /// builds all of them. Holding it keeps another shader from taking its
    /// address.
    ///
    /// [`bind_groups`]: Self::bind_groups
    bind_groups_shader: Option<ShaderProgramBasePtr>,
}

pub(crate) struct Drawable {
    base: DrawableBase,
    impl_: DrawableImpl,
}

thread_local! {
    static DRAW_CALL_COUNT: Cell<u64> = const { Cell::new(0) };
}

impl Drawable {
    #[must_use]
    pub(crate) fn new(name: String) -> Self {
        Drawable {
            base: DrawableBase::new(name),
            impl_: DrawableImpl {
                segments: Vec::new(),
                indexes: None,
                vertex_count: 0,
                vertex_type: AttributeDataType::Invalid,
                attribute_bindings: Vec::new(),
                instance_bindings: Vec::new(),
                vertex_slots: None,
                uniform_buffers: UniformBufferArray::new(),
                depth_mode: DepthMode::disabled(),
                stencil_mode: StencilMode::default(),
                pipeline_stencil: None,
                vertex_attr_id: 0,
                pipeline_state: None,
                pipeline_formats: None,
                bind_groups: Vec::new(),
                index_slot: None,
                bind_groups_shader: None,
            },
        }
    }

    pub(crate) fn set_depth_mode_for_3d(&mut self, value: &DepthMode) {
        if self.impl_.depth_mode.func != value.func || self.impl_.depth_mode.mask != value.mask {
            self.impl_.pipeline_state = None;
        }
        self.impl_.depth_mode = *value;
    }

    pub(crate) fn set_stencil_mode_for_3d(&mut self, value: &StencilMode) {
        if self.impl_.pipeline_stencil != Some(StencilState::from(value)) {
            self.impl_.pipeline_state = None;
        }
        self.impl_.stencil_mode = *value;
    }

    pub(crate) fn set_vertex_attr_id(&mut self, id: usize) {
        self.impl_.vertex_attr_id = id;
    }

    fn upload_in(&mut self, upload_pass: &UploadPass) {
        let Some(shader) = self.base.shader.clone() else {
            Log::warning(
                Event::General,
                format!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name),
            );
            debug_assert!(false);
            return;
        };

        let usage = BufferUsageType::StaticDraw;

        // We need either raw index data or a buffer already created from them.
        // We can have a buffer and no indexes, but only if it's not marked dirty.
        let Some(indexes) = self.impl_.indexes.clone() else {
            debug_assert!(false, "Missing index data");
            return;
        };
        {
            let mut indexes = indexes.borrow_mut();
            if indexes.empty() && (indexes.get_buffer().is_none() || indexes.get_dirty()) {
                debug_assert!(false, "Missing index data");
                return;
            }
            if indexes.get_buffer().is_none() || indexes.get_dirty() {
                // Create or update a buffer for the index data. We don't
                // update any existing buffer because it may still be in use
                // by the previous frame.
                let resource = upload_pass.create_index_buffer_of(&indexes, usage);
                let gfx_index_buffer = GfxIndexBuffer::new(resource);
                indexes.set_buffer(Some(Box::new(IndexBuffer { buffer: gfx_index_buffer })));
                indexes.set_dirty(false);
            }
        }

        // Upload vertex attributes (like Metal does)
        let build_attribs = match (&self.base.vertex_attributes, self.base.attribute_update_time) {
            (Some(attrs), Some(t)) => attrs.borrow().is_modified_after(t),
            _ => true,
        };
        if build_attribs {
            let attrs = Rc::clone(self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr));
            // Apply drawable values to shader defaults (matching Metal's approach)
            let attribute_bindings = upload_pass.build_attribute_bindings(
                self.impl_.vertex_count,
                self.impl_.vertex_type,
                /*vertexAttributeIndex=*/ usize::MAX,
                /*vertexData=*/ &[],
                shader.get_vertex_attributes(),
                &mut attrs.borrow_mut(),
                usage,
                self.base.attribute_update_time,
            );
            if self.impl_.attribute_bindings != attribute_bindings {
                self.impl_.attribute_bindings = attribute_bindings;
                self.impl_.vertex_slots = None;
                self.impl_.pipeline_state = None; // Reset pipeline state when attribute bindings change
            }
        }

        // Build instance buffer (like Metal does)
        let build_instance_buffer = match (&self.base.instance_attributes, self.base.attribute_update_time) {
            (Some(attrs), Some(t)) => attrs.borrow().is_modified_after(t),
            (Some(_), None) => true,
            (None, _) => false,
        };
        if build_instance_buffer {
            let attrs = self.base.instance_attributes.clone().unwrap();
            let instance_count = attrs.borrow().get_min_count();
            let instance_bindings = upload_pass.build_attribute_bindings(
                instance_count,
                self.impl_.vertex_type,
                usize::MAX,
                &[],
                shader.get_instance_attributes(),
                &mut attrs.borrow_mut(),
                usage,
                self.base.attribute_update_time,
            );
            if self.impl_.instance_bindings != instance_bindings {
                self.impl_.instance_bindings = instance_bindings;
                self.impl_.vertex_slots = None;
                self.impl_.pipeline_state = None; // Reset pipeline state when instance bindings change
            }
        }

        // Upload textures if needed
        let textures_need_upload = self.base.textures.iter().flatten().any(|t| t.borrow().needs_upload());
        if textures_need_upload {
            for texture in self.base.textures.iter().flatten() {
                let mut t = texture.borrow_mut();
                if t.needs_upload() {
                    t.upload();
                }
            }
        }

        self.base.attribute_update_time = Some(Instant::now());
    }

    /// Build the bind groups for this draw from the shader's metadata.
    /// Returns false when the draw must be skipped.
    ///
    /// Keeps the previous draw's bind groups as they are while they bind
    /// what this draw would, and otherwise reuses each one whose layout and
    /// bound buffers, samplers and texture views (compared by identity) are
    /// unchanged, takes the others from the bind groups drawables share, and
    /// leaves the index group to the draw. A replaced resource stays alive until the
    /// drawable's next draw or its drop.
    fn build_bind_groups(
        &mut self,
        context: &Context,
        render_pass: &RenderPass,
        shader_ptr: &ShaderProgramBasePtr,
        shader: &ShaderProgram,
    ) -> bool {
        if self.bind_groups_current(render_pass, shader_ptr, shader) {
            return true;
        }
        self.impl_.bind_groups_shader = None;
        let mut previous = std::mem::take(&mut self.impl_.bind_groups);
        self.impl_.index_slot = None;
        let device = context.get_backend().device();
        let draw_call_count = DRAW_CALL_COUNT.with(Cell::get);
        let global_buffers = render_pass.get_global_uniform_buffers();
        let textures = &self.base.textures;

        for (slot, &group) in shader.bind_group_order().iter().enumerate() {
            let Some(layout) = shader.bind_group_layout(group) else {
                Log::warning(
                    Event::Render,
                    format!("WebGPU: missing bind group layout for group {group} in shader '{}'", shader.name()),
                );
                return false;
            };
            if Some(group) == shader.index_group() {
                self.impl_.index_slot = Some(slot as u32);
                continue;
            }
            let binding_infos = shader.binding_infos(group);

            let mut resources: Vec<(u32, EntryResource)> = Vec::with_capacity(binding_infos.len());
            let mut valid_bindings = true;
            for binding_info in binding_infos {
                match binding_info.type_ {
                    BindingType::UniformBuffer | BindingType::ReadOnlyStorageBuffer | BindingType::StorageBuffer => {
                        let mut buffer = self.impl_.uniform_buffers.get(binding_info.binding as usize).cloned();
                        if buffer.is_none()
                            && let Some(globals) = &global_buffers
                        {
                            buffer = globals.get(binding_info.binding as usize).cloned();
                        }
                        let Some(buffer) = buffer else {
                            if draw_call_count <= 200 {
                                Log::warning(
                                    Event::Render,
                                    format!(
                                        "No buffer found for binding {} type={} group={group}",
                                        binding_info.binding, binding_info.type_ as i32
                                    ),
                                );
                            }
                            valid_bindings = false;
                            break;
                        };
                        let Some((gpu_buffer, offset, size)) = context.get_backend().native_buffer(buffer.id()) else {
                            if draw_call_count <= 200 {
                                Log::warning(
                                    Event::Render,
                                    format!(
                                        "Uniform buffer missing GPU handle for binding {} group={group}",
                                        binding_info.binding
                                    ),
                                );
                            }
                            valid_bindings = false;
                            break;
                        };
                        // The UBO's whole (256-byte aligned) slot: some
                        // UBOs hold less data than their shader struct.
                        resources.push((binding_info.binding, EntryResource::Buffer(gpu_buffer, offset, size)));
                    }
                    BindingType::Sampler => {
                        let Some(texture) = texture_for_binding(textures, shader, binding_info.binding, true) else {
                            valid_bindings = false;
                            break;
                        };
                        let mut texture = texture.borrow_mut();
                        let Some(texture) = texture.downcast_mut::<Texture2D>() else {
                            valid_bindings = false;
                            break;
                        };
                        if texture.get_sampler().is_none() {
                            let state = *texture.get_sampler_state();
                            texture.set_sampler_configuration(state);
                        }
                        let Some(sampler) = texture.get_sampler().cloned() else {
                            valid_bindings = false;
                            break;
                        };
                        resources.push((binding_info.binding, EntryResource::Sampler(sampler)));
                    }
                    BindingType::Texture | BindingType::UnfilterableTexture => {
                        let Some(texture) = texture_for_binding(textures, shader, binding_info.binding, false) else {
                            valid_bindings = false;
                            break;
                        };
                        let mut texture = texture.borrow_mut();
                        let Some(texture) = texture.downcast_mut::<Texture2D>() else {
                            valid_bindings = false;
                            break;
                        };
                        if texture.get_texture().is_none() || texture.get_texture_view().is_none() {
                            texture.create();
                        }
                        let Some(view) = texture.get_texture_view().cloned() else {
                            valid_bindings = false;
                            break;
                        };
                        resources.push((binding_info.binding, EntryResource::TextureView(view)));
                    }
                }
            }

            if !valid_bindings || resources.is_empty() {
                Log::warning(
                    Event::Render,
                    format!("WebGPU: invalid bindings for drawable '{}' (group={group}, slot={slot})", self.base.name),
                );
                return false;
            }

            if let Some(index) = previous.iter().position(|record| {
                let key = &record.bind_group.key;
                record.group == group && key.layout == *layout && key.resources == resources
            }) {
                let mut record = previous.swap_remove(index);
                record.slot = slot as u32;
                self.impl_.bind_groups.push(record);
                continue;
            }

            let key = BindGroupKey { layout: layout.clone(), resources };
            let bind_group = context.bind_groups().get_or_create(key, |key| {
                let entries: Vec<wgpu::BindGroupEntry<'_>> = key
                    .resources
                    .iter()
                    .map(|(binding, resource)| wgpu::BindGroupEntry {
                        binding: *binding,
                        resource: match resource {
                            EntryResource::Buffer(buffer, offset, size) => {
                                wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer,
                                    offset: *offset,
                                    size: wgpu::BufferSize::new(*size),
                                })
                            }
                            EntryResource::Sampler(sampler) => wgpu::BindingResource::Sampler(sampler),
                            EntryResource::TextureView(view) => wgpu::BindingResource::TextureView(view),
                        },
                    })
                    .collect();
                let label = format!("{} bind-group {group}", self.base.name);
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&label),
                    layout: &key.layout,
                    entries: &entries,
                })
            });
            self.impl_.bind_groups.push(BindGroupRecord { slot: slot as u32, group, bind_group });
        }
        self.impl_.bind_groups_shader = Some(Rc::clone(shader_ptr));
        true
    }

    /// Whether the bind groups the last draw built for `shader` bind what
    /// this draw would: the same buffers, samplers and texture views, which
    /// a buffer or texture the drawable or the pass replaced, or a texture
    /// that recreated its view, doesn't.
    fn bind_groups_current(
        &self,
        render_pass: &RenderPass,
        shader_ptr: &ShaderProgramBasePtr,
        shader: &ShaderProgram,
    ) -> bool {
        let Some(built_for) = &self.impl_.bind_groups_shader else {
            return false;
        };
        if !std::ptr::addr_eq(Rc::as_ptr(built_for), Rc::as_ptr(shader_ptr)) {
            return false;
        }
        let global_buffers = render_pass.get_global_uniform_buffers();
        let backend = render_pass.context().get_backend();
        self.impl_.bind_groups.iter().all(|record| {
            let resources = &record.bind_group.key.resources;
            let binding_infos = shader.binding_infos(record.group);
            binding_infos.len() == resources.len()
                && binding_infos.iter().zip(resources).all(|(info, (binding, resource))| {
                    info.binding == *binding && self.binds(info, resource, global_buffers.as_deref(), backend, shader)
                })
        })
    }

    /// Whether `resource` is what a draw with `shader` binds at `info`.
    fn binds(
        &self,
        info: &BindingInfo,
        resource: &EntryResource,
        global_buffers: Option<&UniformBufferArray>,
        backend: &BackendImpl,
        shader: &ShaderProgram,
    ) -> bool {
        let texture = |sampler_binding| {
            texture_for_binding(&self.base.textures, shader, info.binding, sampler_binding).map(|t| t.borrow())
        };
        match (info.type_, resource) {
            (
                BindingType::UniformBuffer | BindingType::ReadOnlyStorageBuffer | BindingType::StorageBuffer,
                EntryResource::Buffer(bound, bound_offset, bound_size),
            ) => self
                .impl_
                .uniform_buffers
                .get(info.binding as usize)
                .or_else(|| global_buffers?.get(info.binding as usize))
                .and_then(|buffer| backend.native_buffer(buffer.id()))
                .is_some_and(|(buffer, offset, size)| {
                    buffer == *bound && offset == *bound_offset && size == *bound_size
                }),
            (BindingType::Sampler, EntryResource::Sampler(bound)) => texture(true).is_some_and(|texture| {
                texture.downcast_ref::<Texture2D>().and_then(Texture2D::get_sampler) == Some(bound)
            }),
            (BindingType::Texture | BindingType::UnfilterableTexture, EntryResource::TextureView(bound)) => {
                texture(false).is_some_and(|texture| {
                    texture.downcast_ref::<Texture2D>().is_some_and(|texture| {
                        texture.get_texture().is_some() && texture.get_texture_view() == Some(bound)
                    })
                })
            }
            _ => false,
        }
    }
}

/// The texture of `textures` that `shader` samples at `binding`: the one
/// whose sampler is there if `sampler_binding`, else the one whose texture
/// is (see [`ShaderProgram::texture_at`]).
fn texture_for_binding<'a>(
    textures: &'a [Option<Texture2DPtr>],
    shader: &ShaderProgram,
    binding: u32,
    sampler_binding: bool,
) -> Option<&'a Texture2DPtr> {
    textures.get(shader.texture_at(binding, sampler_binding)?)?.as_ref()
}

/// A vertex buffer slot of a draw. A draw reads the buffer from
/// `vertex_resource`, so the slot holds no handle of it.
struct UniqueBinding {
    vertex_resource: Rc<dyn mln_gfx::vertex_buffer::VertexBufferResource>,
    stride: u32,
    base_offset: u64,
    step: VertexStep,
    attributes: Vec<GpuVertexAttribute>,
}

/// Adds the bindings of `source`, which is indexed by `shader`'s locations,
/// to the slots in `out`.
fn accumulate_bindings(
    shader: &ShaderProgram,
    source: &AttributeBindingArray,
    step: VertexStep,
    out: &mut Vec<UniqueBinding>,
) {
    for (location, binding) in source.iter().enumerate() {
        let Some(binding) = binding else { continue };
        let Some(id) = shader.attribute_id(step, location) else { continue };
        let Some(resource) = &binding.vertex_buffer_resource else { continue };
        let Some(vertex_buffer_res) = resource.downcast_ref::<VertexBufferResource>() else {
            continue;
        };
        if !vertex_buffer_res.get_buffer().is_valid() {
            continue;
        }
        let binding_byte_offset = u64::from(binding.vertex_offset) * u64::from(binding.vertex_stride);
        // Attributes of a buffer at different vertex offsets (a fill
        // extrusion wall's outline vertex and the next one) read it through
        // slots of their own, since an attribute's offset must stay within
        // the stride.
        let pos = out.iter().position(|e| {
            std::ptr::addr_eq(Rc::as_ptr(&e.vertex_resource), Rc::as_ptr(resource))
                && e.stride == binding.vertex_stride
                && e.step == step
                && e.base_offset == binding_byte_offset
        });
        let entry = match pos {
            None => {
                out.push(UniqueBinding {
                    vertex_resource: Rc::clone(resource),
                    stride: binding.vertex_stride,
                    base_offset: binding_byte_offset,
                    step,
                    attributes: Vec::with_capacity(4),
                });
                out.last_mut().unwrap()
            }
            Some(i) => &mut out[i],
        };
        entry.attributes.push(GpuVertexAttribute {
            id,
            format: binding.attribute.data_type,
            offset: binding.attribute.offset,
        });
    }
}

impl drawable::Drawable for Drawable {
    fn base(&self) -> &DrawableBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut DrawableBase {
        &mut self.base
    }

    fn upload(&mut self, upload_pass: &dyn mln_gfx::upload_pass::UploadPass) {
        if let Some(upload_pass) = upload_pass.downcast_ref::<UploadPass>() {
            self.upload_in(upload_pass);
        }
    }

    fn draw(&mut self, parameters: &mut dyn PaintParameters) {
        let draw_call_count = DRAW_CALL_COUNT.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        // Get WebGPU context and render pass (following Metal's pattern)
        let parameters: &dyn PaintParameters = parameters;
        let Some(context) = parameters.context().downcast_ref::<Context>() else {
            return;
        };
        let Some(render_pass) = parameters.render_pass().and_then(|p| p.downcast_ref::<RenderPass>()) else {
            Log::error(Event::Render, "No render pass encoder available");
            debug_assert!(false);
            return;
        };
        if !render_pass.has_encoder() {
            Log::error(Event::Render, "No render pass encoder available");
            debug_assert!(false);
            return;
        }

        let Some(shader_ptr) = self.base.shader.clone() else {
            Log::warning(
                Event::General,
                format!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name),
            );
            debug_assert!(false);
            return;
        };
        if !self.base.enabled {
            return;
        }

        // Bind uniform buffers (a no-op in WebGPU, see UniformBufferArray).

        // Check index buffer is valid (like Metal does)
        if let Some(indexes) = &self.impl_.indexes {
            let indexes = indexes.borrow();
            if indexes.get_buffer().is_none() || indexes.get_dirty() {
                debug_assert!(false, "Index buffer not uploaded");
                return;
            }
        }

        // Build bind groups based on shader metadata
        let Some(shader) = as_webgpu_shader(&shader_ptr) else {
            return;
        };
        if !self.build_bind_groups(context, render_pass, &shader_ptr, shader) {
            return;
        }

        let drawable_tile_id = self.base.tile_id;
        let mut depth_mode = DepthMode::disabled();
        let mut stencil_mode = StencilMode::disabled();
        if self.base.is_3d {
            depth_mode = self.impl_.depth_mode;
            stencil_mode = self.impl_.stencil_mode;
        } else {
            if self.base.enable_depth {
                depth_mode = parameters.depth_mode_for_sublayer(self.base.sub_layer_index as u8, self.base.depth_type);
            }
            if let Some(mode) = self.base.stencil_mode {
                stencil_mode = mode;
            } else if self.base.enable_stencil
                && let Some(tile_id) = drawable_tile_id
            {
                stencil_mode = parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped());
            }
        }

        let stencil = StencilState::from(&stencil_mode);
        if self.impl_.pipeline_stencil != Some(stencil)
            || self.impl_.depth_mode.func != depth_mode.func
            || self.impl_.depth_mode.mask != depth_mode.mask
        {
            self.impl_.pipeline_state = None;
            self.impl_.pipeline_stencil = Some(stencil);
        }
        self.impl_.depth_mode = depth_mode;
        self.impl_.stencil_mode = stencil_mode;

        let unique_bindings = self.impl_.vertex_slots.get_or_insert_with(|| {
            const MAX_VERTEX_BUFFER_SLOTS: usize = 8;

            let mut unique_bindings =
                Vec::with_capacity(self.impl_.attribute_bindings.len() + self.impl_.instance_bindings.len());
            accumulate_bindings(shader, &self.impl_.attribute_bindings, VertexStep::Vertex, &mut unique_bindings);
            accumulate_bindings(shader, &self.impl_.instance_bindings, VertexStep::Instance, &mut unique_bindings);

            if unique_bindings.len() > MAX_VERTEX_BUFFER_SLOTS {
                Log::warning(
                    Event::Render,
                    format!(
                        "WebGPU: reducing vertex buffer bindings from {} to {MAX_VERTEX_BUFFER_SLOTS}",
                        unique_bindings.len()
                    ),
                );
                unique_bindings.truncate(MAX_VERTEX_BUFFER_SLOTS);
            }
            unique_bindings
        });

        let formats = context.get_backend().target_formats();
        if self.impl_.pipeline_formats != Some(formats) {
            self.impl_.pipeline_state = None;
            self.impl_.pipeline_formats = Some(formats);
        }
        if self.impl_.pipeline_state.is_none() {
            let color_mode = self.base.color_mode;
            let state = PipelineState {
                vertex_buffers: unique_bindings
                    .iter()
                    .map(|b| VertexBufferLayout { stride: b.stride, step: b.step, attributes: b.attributes.clone() })
                    .collect(),
                primitive: self.impl_.segments.first().map_or(DrawModeType::Triangles, |s| s.get_mode().type_),
                cull: self.base.cull_face_mode,
                blend: color_mode.blend_function,
                color_mask: color_mode.mask,
                depth: DepthState::from(&depth_mode),
                stencil: StencilState::from(&stencil_mode),
            };
            match shader.render_pipeline(&state, formats.0, formats.1) {
                Ok(pipeline) => self.impl_.pipeline_state = Some(pipeline),
                Err(error) => {
                    Log::warning(Event::Render, format!("WebGPU: drawable '{}': {error}", self.base.name));
                }
            }
        }

        let index_bind_group = match self.impl_.index_slot {
            Some(slot) => {
                let Some((bind_group, offset)) = context.index_bind_group(self.base.ubo_index) else {
                    return;
                };
                Some((slot, bind_group, offset))
            }
            None => None,
        };

        let Some(mut encoder) = render_pass.state_filter() else {
            Log::error(Event::Render, "renderPassEncoder is null!");
            return;
        };
        let Some(pipeline) = &self.impl_.pipeline_state else {
            Log::warning(
                Event::Render,
                format!("WebGPU: no pipeline available for drawable '{}'; draw skipped", self.base.name),
            );
            return;
        };
        encoder.set_pipeline(pipeline);

        let size = render_pass.get_renderable_size();
        encoder.set_viewport(
            0.0,
            0.0,
            size.width as f32,
            size.height as f32,
            depth_mode.range.min,
            depth_mode.range.max,
        );

        encoder.set_stencil_reference(stencil_mode.ref_ as u32);

        // Clamp scissor to the current render pass size (not the default
        // renderable, which may be different during offscreen passes).
        {
            let rt_size = render_pass.get_renderable_size();
            let scissor = parameters.scissor_rect();
            let sx = scissor.x as u32;
            let sy = scissor.y as u32;
            let sw = if sx < rt_size.width { scissor.width.min(rt_size.width - sx) } else { 0 };
            let sh = if sy < rt_size.height { scissor.height.min(rt_size.height - sy) } else { 0 };
            if sw > 0 && sh > 0 {
                encoder.set_scissor_rect(sx, sy, sw, sh);
            }
        }

        let mut buffer_slot = 0u32;
        for binding in unique_bindings.iter() {
            let Some(resource) = binding.vertex_resource.downcast_ref::<VertexBufferResource>() else {
                continue;
            };
            let buffer_resource = resource.get_buffer();
            let Some(buffer) = buffer_resource.get_buffer() else { continue };
            let buffer_size = buffer_resource.get_size_in_bytes() as u64;
            if binding.base_offset >= buffer_size {
                Log::warning(
                    Event::Render,
                    format!(
                        "Skipping vertex buffer slot {buffer_slot} due to base offset {} >= size {buffer_size}",
                        binding.base_offset
                    ),
                );
                continue;
            }
            encoder.set_vertex_buffer(buffer_slot, buffer, binding.base_offset..buffer_size);
            buffer_slot += 1;
        }

        // Bind index buffer if present
        if let Some(indexes) = &self.impl_.indexes {
            let indexes = indexes.borrow();
            if indexes.elements() > 0
                && !indexes.get_dirty()
                && let Some(index_buffer) = indexes.get_buffer().and_then(|b| b.downcast_ref::<IndexBuffer>())
                && let Some(resource) = index_buffer.buffer.get_resource::<IndexBufferResource>()
            {
                let buffer_resource = resource.get_buffer();
                if let Some(buffer) = buffer_resource.get_buffer() {
                    // Always use Uint16 format for now
                    encoder.set_index_buffer(
                        buffer,
                        0..buffer_resource.get_size_in_bytes() as u64,
                        wgpu::IndexFormat::Uint16,
                    );
                }
            }
        }

        // Set bind groups
        for record in &self.impl_.bind_groups {
            encoder.set_bind_group(record.slot, &record.bind_group.handle, None);
        }
        if let Some((slot, bind_group, offset)) = &index_bind_group {
            encoder.set_bind_group(*slot, bind_group, Some(*offset));
        }

        // Draw indexed geometry - loop through segments (exactly like Metal does)
        let mut draw_calls = 0;
        for segment in &self.impl_.segments {
            let ml_segment = segment.get_segment();
            if ml_segment.index_length > 0 {
                let index_offset = ml_segment.index_offset as u32;
                let base_vertex = ml_segment.vertex_offset as i32;
                let base_instance = ml_segment.base_instance as u32;
                encoder.draw_indexed(
                    index_offset..index_offset + ml_segment.index_length as u32,
                    base_vertex,
                    base_instance..base_instance + ml_segment.instance_count as u32,
                );
                draw_calls += 1;
            }
        }
        drop(encoder);
        if draw_calls > 0 {
            let mut stats = context.rendering_stats_mut();
            stats.num_draw_calls += draw_calls;
            stats.total_draw_calls += draw_calls;
        }
        let _ = draw_call_count;
    }

    fn update_vertex_attributes(
        &mut self,
        vertices: Option<VertexAttributeArrayPtr>,
        vertex_count: usize,
        mode: DrawMode,
        indexes: Option<IndexVectorBasePtr>,
        segments: &[SegmentBase],
    ) {
        // Store the vertex attributes (base class method)
        drawable::Drawable::set_vertex_attributes(self, vertices);
        // Store vertex count
        self.impl_.vertex_count = vertex_count;
        // Handle segments
        let draw_segs: Vec<UniqueDrawSegment> =
            segments.iter().map(|seg| Box::new(DrawSegment::new(mode, seg.copy()))).collect();
        // Set the index data with segments
        self.set_index_data(indexes, draw_segs);
    }

    fn set_vertices(&mut self, data: Vec<u8>, count: usize, data_type: AttributeDataType) {
        self.impl_.vertex_count = count;
        self.impl_.vertex_type = data_type;
        if count != 0 && data_type != AttributeDataType::Invalid && !data.is_empty() {
            let attrs = Rc::clone(self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr));
            let mut attrs = attrs.borrow_mut();
            if let Some(attrib) = attrs.set(self.impl_.vertex_attr_id, /*index=*/ -1, data_type, 0) {
                attrib.set_raw_data(data);
                attrib.set_stride(VertexAttribute::get_stride_of(data_type));
            } else {
                Log::warning(
                    Event::General,
                    format!("Vertex attribute type mismatch: {} / {}", self.base.name, self.impl_.vertex_attr_id),
                );
                debug_assert!(false);
            }
        }
    }

    fn set_index_data(&mut self, indexes: Option<IndexVectorBasePtr>, segments: Vec<UniqueDrawSegment>) {
        self.impl_.indexes = indexes;
        self.impl_.segments = segments;
    }

    fn mutable_uniform_buffers(&mut self) -> &mut UniformBufferArray {
        &mut self.impl_.uniform_buffers
    }

    fn set_color_mode(&mut self, value: &ColorMode) {
        self.base.color_mode = *value;
        self.impl_.pipeline_state = None; // Rebuild pipeline to honour new color mode
    }

    fn set_enable_stencil(&mut self, value: bool) {
        if self.base.enable_stencil == value {
            return;
        }
        self.base.enable_stencil = value;
        self.impl_.pipeline_state = None; // Force recreation with new stencil state
    }

    fn set_enable_depth(&mut self, value: bool) {
        if self.base.enable_depth == value {
            return;
        }
        self.base.enable_depth = value;
        self.impl_.pipeline_state = None; // Force recreation with new depth state
    }

    fn set_sub_layer_index(&mut self, value: i32) {
        if self.base.sub_layer_index == value {
            return;
        }
        self.base.sub_layer_index = value;
    }

    fn set_depth_type(&mut self, value: DepthMaskType) {
        if self.base.depth_type == value {
            return;
        }
        self.base.depth_type = value;
        self.impl_.pipeline_state = None; // Force recreation with new depth mask
    }

    fn set_shader(&mut self, value: Option<ShaderProgramBasePtr>) {
        if same_shader(&self.base.shader, &value) {
            return;
        }
        self.base.shader = value;
        self.impl_.pipeline_state = None; // Reset pipeline when shader changes
    }
}
