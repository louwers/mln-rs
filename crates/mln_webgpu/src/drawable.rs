// Ports include/mln/webgpu/drawable.hpp, src/mln/webgpu/drawable_impl.hpp and
// src/mln/webgpu/drawable.cpp
//
// Logging: the C++ `Log::Warning`/`Log::Error` calls print to stderr here.
// TODO(port): route them through mln::Log once a logging port exists.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::attribute::AttributeBindingArray;
use mln_gfx::color_mode::{BlendFunction, ColorMode};
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::drawable::{self, DrawSegment, DrawableBase, UniqueDrawSegment};
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::index_vector::{IndexBufferBase, IndexVectorBasePtr};
use mln_gfx::paint_parameters::PaintParameters;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_program_base::{ShaderProgramBasePtr, same_shader};
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::texture2d::{Texture2D as _, Texture2DPtr};
use mln_gfx::types::{BufferUsageType, DepthMaskType};
use mln_gfx::uniform_buffer::UniformBufferArray;
use mln_gfx::upload_pass::UploadPass as _;
use mln_gfx::util_hash::hash;
use mln_gfx::vertex_attribute::{VertexAttribute, VertexAttributeArray, VertexAttributeArrayPtr};
use mln_gfx::vertex_buffer::IndexBuffer as GfxIndexBuffer;
use mln_util::time::Instant;

use crate::bind_groups::{BindGroupKey, EntryResource, SharedBindGroup};
use crate::context::Context;
use crate::render_pass::RenderPass;
use crate::shaders::shader_program::{BindingInfo, BindingType, ShaderProgram, VertexLayout, as_webgpu_shader};
use crate::texture2d::Texture2D;
use crate::uniform_buffer::UniformBuffer;
use crate::upload_pass::UploadPass;
use crate::vertex_buffer_resource::{IndexBufferResource, VertexBufferResource};

// Leaves out the reference, which draw() sets per draw rather than in the pipeline.
fn hash_stencil_mode(mode: &StencilMode) -> u64 {
    let mut h = hash(&[u64::from(mode.mask), mode.fail as u64, mode.depth_fail as u64, mode.pass as u64]);
    h = hash(&[h, mode.test.func() as u64]);
    if let Some(mask) = mode.test.masked() {
        h = hash(&[h, u64::from(mask)]);
    }
    h
}

fn hash_pipeline_state(
    vertex_layouts: &[VertexLayout],
    color_mode: &ColorMode,
    depth_mode: &DepthMode,
    stencil_mode: &StencilMode,
    draw_mode_type: DrawModeType,
) -> u64 {
    let mut h = 0u64;
    // Hash vertex layouts
    for layout in vertex_layouts {
        h = hash(&[h, u64::from(layout.array_stride as u32)]);
        h = hash(&[h, layout.step_mode as u64]);
        h = hash(&[h, layout.attributes.len() as u64]);
        for attr in layout.attributes.iter().take(16) {
            h = hash(&[h, attr.format as u64, u64::from(attr.shader_location), u64::from(attr.offset as u32)]);
        }
    }
    // Hash color mode
    h = hash(&[
        h,
        u64::from(color_mode.mask.r),
        u64::from(color_mode.mask.g),
        u64::from(color_mode.mask.b),
        u64::from(color_mode.mask.a),
    ]);
    match &color_mode.blend_function {
        BlendFunction::Replace => h = hash(&[h, 0]),
        f => {
            h = hash(&[h, f.equation() as u64, f.src_factor() as u64, f.dst_factor() as u64]);
        }
    }
    // Hash depth mode
    h = hash(&[h, depth_mode.func as u64, depth_mode.mask as u64]);
    // Hash stencil mode
    h = hash(&[h, hash_stencil_mode(stencil_mode)]);
    // Hash draw mode
    hash(&[h, draw_mode_type as u64])
}

fn wgpu_vertex_format_of(type_: AttributeDataType) -> wgpu::VertexFormat {
    use AttributeDataType as A;
    use wgpu::VertexFormat as F;
    match type_ {
        A::Byte => F::Sint8,
        A::Byte2 => F::Sint8x2,
        A::Byte4 => F::Sint8x4,
        A::UByte => F::Uint8,
        A::UByte2 => F::Uint8x2,
        A::UByte4 => F::Uint8x4,
        A::UByte4Normalized => F::Unorm8x4,
        A::Short => F::Sint16,
        A::Short2 => F::Sint16x2,
        A::Short4 => F::Sint16x4,
        A::UShort => F::Uint16,
        A::UShort2 => F::Uint16x2,
        A::UShort4 => F::Uint16x4,
        A::Int => F::Sint32,
        A::Int2 => F::Sint32x2,
        A::Int3 => F::Sint32x3,
        A::Int4 => F::Sint32x4,
        A::UInt => F::Uint32,
        A::UInt2 => F::Uint32x2,
        A::UInt3 => F::Uint32x3,
        A::UInt4 => F::Uint32x4,
        A::Float => F::Float32,
        A::Float2 => F::Float32x2,
        A::Float3 => F::Float32x3,
        A::Float4 => F::Float32x4,
        _ => {
            debug_assert!(false, "Unsupported vertex attribute format");
            F::Float32x2 // Default fallback
        }
    }
}

/// `struct IndexBuffer : gfx::IndexBufferBase`: adapts `gfx::IndexBuffer`.
struct IndexBuffer {
    buffer: GfxIndexBuffer,
}

impl IndexBufferBase for IndexBuffer {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct BindGroupRecord {
    slot: u32,
    group: u32,
    /// A later draw that binds the same layout and resources reuses it
    /// instead of looking it up.
    bind_group: Rc<SharedBindGroup>,
}

/// `webgpu::Drawable::Impl`
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
    vertex_attr_id: usize,
    stencil_mode_hash: u64,
    stencil_mode_hash_valid: bool,
    /// We don't own the pipeline - it's owned by the shader program's cache.
    pipeline_state: Option<wgpu::RenderPipeline>,
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

pub struct Drawable {
    base: DrawableBase,
    impl_: DrawableImpl,
}

thread_local! {
    static DRAW_CALL_COUNT: Cell<u64> = const { Cell::new(0) };
}

impl Drawable {
    #[must_use]
    pub fn new(name: String) -> Self {
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
                vertex_attr_id: 0,
                stencil_mode_hash: 0,
                stencil_mode_hash_valid: false,
                pipeline_state: None,
                bind_groups: Vec::new(),
                index_slot: None,
                bind_groups_shader: None,
            },
        }
    }

    pub fn set_depth_mode_for_3d(&mut self, value: &DepthMode) {
        if self.impl_.depth_mode.func != value.func || self.impl_.depth_mode.mask != value.mask {
            self.impl_.pipeline_state = None;
        }
        self.impl_.depth_mode = *value;
    }

    pub fn set_stencil_mode_for_3d(&mut self, value: &StencilMode) {
        let new_hash = hash_stencil_mode(value);
        let m = &self.impl_.stencil_mode;
        if !self.impl_.stencil_mode_hash_valid
            || self.impl_.stencil_mode_hash != new_hash
            || m.mask != value.mask
            || m.fail != value.fail
            || m.depth_fail != value.depth_fail
            || m.pass != value.pass
        {
            self.impl_.pipeline_state = None;
            self.impl_.stencil_mode_hash_valid = false;
        }
        self.impl_.stencil_mode = *value;
        self.impl_.stencil_mode_hash = new_hash;
    }

    pub fn set_vertex_attr_id(&mut self, id: usize) {
        self.impl_.vertex_attr_id = id;
    }

    pub fn upload(&mut self, upload_pass: &UploadPass) {
        let Some(shader) = self.base.shader.clone() else {
            eprintln!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name);
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
                let gfx_index_buffer = GfxIndexBuffer::new(indexes.elements(), resource);
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
            let attrs = self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr).clone();
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

        // Upload uniform buffers to ensure they're ready for the GPU: only a
        // check that each has a GPU buffer.
        for i in 0..self.impl_.uniform_buffers.allocated_size() {
            if let Some(ub) = self.impl_.uniform_buffers.get(i)
                && let Some(webgpu_ub) = ub.as_any().downcast_ref::<UniformBuffer>()
                && webgpu_ub.get_buffer().is_none()
            {
                eprintln!("Uniform buffer {i} has no GPU buffer!");
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
    /// Returns false when the draw must be skipped (C++ `return`).
    ///
    /// C++ creates every bind group on every draw, with a `GlobalUBOIndex`
    /// buffer of the drawable's. The port keeps the previous draw's bind
    /// groups as they are while they bind what this draw would, and
    /// otherwise reuses each one whose layout and bound buffers, samplers
    /// and texture views (compared by identity) are unchanged, takes the
    /// others from the bind groups drawables share, and leaves the index
    /// group to the draw. A replaced resource stays alive until the
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
        let Some(device) = context.get_backend().get_device() else {
            return true;
        };
        let draw_call_count = DRAW_CALL_COUNT.with(Cell::get);
        let global_buffers = render_pass.get_global_uniform_buffers();
        let textures = &self.base.textures;

        for (slot, &group) in shader.get_bind_group_order().iter().enumerate() {
            let Some(layout) = shader.get_bind_group_layout(group) else {
                eprintln!(
                    "WebGPU: missing bind group layout for group {group} in shader '{}'",
                    shader.get_shader_name()
                );
                return false;
            };
            if Some(group) == shader.get_index_group() {
                self.impl_.index_slot = Some(slot as u32);
                continue;
            }
            let binding_infos = shader.get_binding_infos_for_group(group);

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
                                eprintln!(
                                    "No buffer found for binding {} type={} group={group}",
                                    binding_info.binding, binding_info.type_ as i32
                                );
                            }
                            valid_bindings = false;
                            break;
                        };
                        let gpu_buffer = buffer
                            .as_any()
                            .downcast_ref::<UniformBuffer>()
                            .and_then(|b| Some((b.get_buffer()?.clone(), b.get_offset(), b.get_aligned_size())));
                        let Some((gpu_buffer, offset, size)) = gpu_buffer else {
                            if draw_call_count <= 200 {
                                eprintln!(
                                    "Uniform buffer missing GPU handle for binding {} group={group}",
                                    binding_info.binding
                                );
                            }
                            valid_bindings = false;
                            break;
                        };
                        // The UBO's whole (256-byte aligned) slot, as C++
                        // binds `getSize()`: some UBOs hold less data than
                        // their shader struct.
                        resources.push((binding_info.binding, EntryResource::Buffer(gpu_buffer, offset, size)));
                    }
                    BindingType::Sampler => {
                        let Some(texture) = texture_for_binding(textures, shader, binding_info.binding, true) else {
                            valid_bindings = false;
                            break;
                        };
                        let mut texture = texture.borrow_mut();
                        let Some(texture) = texture.as_any_mut().downcast_mut::<Texture2D>() else {
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
                        let Some(texture) = texture.as_any_mut().downcast_mut::<Texture2D>() else {
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
                eprintln!("WebGPU: invalid bindings for drawable '{}' (group={group}, slot={slot})", self.base.name);
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
        self.impl_.bind_groups_shader = Some(shader_ptr.clone());
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
        self.impl_.bind_groups.iter().all(|record| {
            let resources = &record.bind_group.key.resources;
            let binding_infos = shader.get_binding_infos_for_group(record.group);
            binding_infos.len() == resources.len()
                && binding_infos.iter().zip(resources).all(|(info, (binding, resource))| {
                    info.binding == *binding && self.binds(info, resource, global_buffers.as_deref(), shader)
                })
        })
    }

    /// Whether `resource` is what a draw with `shader` binds at `info`.
    fn binds(
        &self,
        info: &BindingInfo,
        resource: &EntryResource,
        global_buffers: Option<&UniformBufferArray>,
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
                .and_then(|buffer| buffer.as_any().downcast_ref::<UniformBuffer>())
                .is_some_and(|buffer| {
                    buffer.get_buffer() == Some(bound)
                        && buffer.get_offset() == *bound_offset
                        && buffer.get_aligned_size() == *bound_size
                }),
            (BindingType::Sampler, EntryResource::Sampler(bound)) => texture(true).is_some_and(|texture| {
                texture.as_any().downcast_ref::<Texture2D>().and_then(Texture2D::get_sampler) == Some(bound)
            }),
            (BindingType::Texture | BindingType::UnfilterableTexture, EntryResource::TextureView(bound)) => {
                texture(false).is_some_and(|texture| {
                    texture.as_any().downcast_ref::<Texture2D>().is_some_and(|texture| {
                        texture.get_texture().is_some() && texture.get_texture_view() == Some(bound)
                    })
                })
            }
            _ => false,
        }
    }
}

/// The texture of `textures` that `shader` samples at `binding`: the one
/// whose sampler is at `binding` if `sampler_binding`, else the one whose
/// texture is, at its sampler's binding or the next.
fn texture_for_binding<'a>(
    textures: &'a [Option<Texture2DPtr>],
    shader: &ShaderProgram,
    binding: u32,
    sampler_binding: bool,
) -> Option<&'a Texture2DPtr> {
    textures.iter().enumerate().find_map(|(tex_index, texture)| {
        let texture = texture.as_ref()?;
        let location = shader.get_sampler_location_of(tex_index)? as u32;
        let matches = location == binding || (!sampler_binding && location + 1 == binding);
        matches.then_some(texture)
    })
}

impl ShaderProgram {
    fn get_sampler_location_of(&self, id: usize) -> Option<usize> {
        mln_gfx::shaders::shader_program_base::ShaderProgramBase::get_sampler_location(self, id)
    }
}

/// `UniqueBinding` in `Drawable::draw`: a vertex buffer slot. A draw reads
/// the buffer from `vertex_resource`, so the slot holds no handle of it.
struct UniqueBinding {
    vertex_resource: Rc<dyn mln_gfx::vertex_buffer::VertexBufferResource>,
    stride: u32,
    base_offset: u64,
    step_mode: wgpu::VertexStepMode,
    attributes: Vec<wgpu::VertexAttribute>,
}

fn accumulate_bindings(source: &AttributeBindingArray, step_mode: wgpu::VertexStepMode, out: &mut Vec<UniqueBinding>) {
    for (attribute_index, binding) in source.iter().enumerate() {
        let Some(binding) = binding else { continue };
        let Some(resource) = &binding.vertex_buffer_resource else { continue };
        let Some(vertex_buffer_res) = resource.as_any().downcast_ref::<VertexBufferResource>() else {
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
                && e.step_mode == step_mode
                && e.base_offset == binding_byte_offset
        });
        let entry = match pos {
            None => {
                out.push(UniqueBinding {
                    vertex_resource: resource.clone(),
                    stride: binding.vertex_stride,
                    base_offset: binding_byte_offset,
                    step_mode,
                    attributes: Vec::with_capacity(4),
                });
                out.last_mut().unwrap()
            }
            Some(i) => &mut out[i],
        };
        entry.attributes.push(wgpu::VertexAttribute {
            format: wgpu_vertex_format_of(binding.attribute.data_type),
            offset: u64::from(binding.attribute.offset),
            shader_location: attribute_index as u32,
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
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn draw(&mut self, parameters: &mut dyn PaintParameters) {
        let draw_call_count = DRAW_CALL_COUNT.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        // Get WebGPU context and render pass (following Metal's pattern)
        let parameters: &dyn PaintParameters = parameters;
        let Some(context) = parameters.context().as_any().downcast_ref::<Context>() else {
            return;
        };
        let Some(render_pass) = parameters.render_pass().and_then(|p| p.as_any().downcast_ref::<RenderPass>()) else {
            eprintln!("No render pass encoder available");
            debug_assert!(false);
            return;
        };
        if !render_pass.has_encoder() {
            eprintln!("No render pass encoder available");
            debug_assert!(false);
            return;
        }

        let Some(shader_ptr) = self.base.shader.clone() else {
            eprintln!("Missing shader for drawable {}/{}", self.base.unique_id, self.base.name);
            debug_assert!(false);
            return;
        };
        if !self.base.enabled {
            return;
        }

        let backend = context.get_backend();
        if backend.get_device().is_none() || backend.get_queue().is_none() {
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
            if self.base.enable_stencil
                && let Some(tile_id) = drawable_tile_id
            {
                stencil_mode = parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped());
            }
        }

        let new_stencil_hash = hash_stencil_mode(&stencil_mode);
        if !self.impl_.stencil_mode_hash_valid
            || self.impl_.stencil_mode_hash != new_stencil_hash
            || self.impl_.depth_mode.func != depth_mode.func
            || self.impl_.depth_mode.mask != depth_mode.mask
        {
            self.impl_.pipeline_state = None;
            self.impl_.stencil_mode_hash_valid = true;
        }
        self.impl_.depth_mode = depth_mode;
        self.impl_.stencil_mode = stencil_mode;
        self.impl_.stencil_mode_hash = new_stencil_hash;

        let unique_bindings = self.impl_.vertex_slots.get_or_insert_with(|| {
            const MAX_VERTEX_BUFFER_SLOTS: usize = 8;

            let mut unique_bindings =
                Vec::with_capacity(self.impl_.attribute_bindings.len() + self.impl_.instance_bindings.len());
            accumulate_bindings(&self.impl_.attribute_bindings, wgpu::VertexStepMode::Vertex, &mut unique_bindings);
            accumulate_bindings(&self.impl_.instance_bindings, wgpu::VertexStepMode::Instance, &mut unique_bindings);

            if unique_bindings.len() > MAX_VERTEX_BUFFER_SLOTS {
                eprintln!(
                    "WebGPU: reducing vertex buffer bindings from {} to {MAX_VERTEX_BUFFER_SLOTS}",
                    unique_bindings.len()
                );
                unique_bindings.truncate(MAX_VERTEX_BUFFER_SLOTS);
            }
            unique_bindings
        });

        if self.impl_.pipeline_state.is_none() {
            // Create vertex layout from attribute bindings
            let vertex_layouts: Vec<VertexLayout> = unique_bindings
                .iter()
                .map(|b| VertexLayout {
                    array_stride: u64::from(b.stride),
                    step_mode: b.step_mode,
                    attributes: b.attributes.clone(),
                })
                .collect();
            // Use the actual color mode from the drawable (like Metal does)
            let color_mode = self.base.color_mode;
            let draw_mode_type = self.impl_.segments.first().map_or(DrawModeType::Triangles, |s| s.get_mode().type_);
            // Compute pipeline state hash for caching
            let cull_mode = self.base.cull_face_mode;
            let pipeline_hash = hash(&[
                hash_pipeline_state(&vertex_layouts, &color_mode, &depth_mode, &stencil_mode, draw_mode_type),
                u64::from(cull_mode.enabled),
                cull_mode.side as u64,
                cull_mode.winding as u64,
            ]);
            self.impl_.pipeline_state = shader.get_render_pipeline(
                &vertex_layouts,
                &color_mode,
                &depth_mode,
                &stencil_mode,
                draw_mode_type,
                Some(pipeline_hash),
                &cull_mode,
            );
            if self.impl_.pipeline_state.is_none() {
                eprintln!(
                    "WebGPU: skipping pipeline creation for drawable '{}' (shader='{}')",
                    self.base.name,
                    mln_gfx::shader::Shader::type_name(shader)
                );
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
            eprintln!("renderPassEncoder is null!");
            return;
        };
        let Some(pipeline) = &self.impl_.pipeline_state else {
            eprintln!("WebGPU: no pipeline available for drawable '{}'; draw skipped", self.base.name);
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

        if self.base.enable_stencil {
            encoder.set_stencil_reference(stencil_mode.ref_ as u32);
        }

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
            let Some(resource) = binding.vertex_resource.as_any().downcast_ref::<VertexBufferResource>() else {
                continue;
            };
            let buffer_resource = resource.get_buffer();
            let Some(buffer) = buffer_resource.get_buffer() else { continue };
            let buffer_size = buffer_resource.get_size_in_bytes() as u64;
            if binding.base_offset >= buffer_size {
                eprintln!(
                    "Skipping vertex buffer slot {buffer_slot} due to base offset {} >= size {buffer_size}",
                    binding.base_offset
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
                && let Some(index_buffer) = indexes.get_buffer().and_then(|b| b.as_any().downcast_ref::<IndexBuffer>())
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

        // Handle depth and stencil states (like Metal does). For 3D mode,
        // stenciling is handled by the layer group.
        if !self.base.is_3d {
            if self.base.enable_stencil {
                if let Some(tile_id) = self.base.tile_id {
                    let expected_stencil = parameters.stencil_mode_for_clipping(&tile_id.to_unwrapped());
                    if hash_stencil_mode(&expected_stencil) != hash_stencil_mode(&self.impl_.stencil_mode) {
                        debug_assert!(false, "stencil mismatch");
                        eprintln!("WebGPU drawable stencil mismatch for '{}' tile={tile_id}", self.base.name);
                    }
                    encoder.set_stencil_reference(expected_stencil.ref_ as u32);
                }
            } else {
                // Reset the reference so subsequent drawables that re-enable
                // stencilling start from a known value.
                encoder.set_stencil_reference(0);
            }
            if self.base.enable_depth {
                #[cfg(debug_assertions)]
                {
                    let expected_depth =
                        parameters.depth_mode_for_sublayer(self.base.sub_layer_index as u8, self.base.depth_type);
                    debug_assert_eq!(expected_depth.func, self.impl_.depth_mode.func);
                    debug_assert_eq!(expected_depth.mask, self.impl_.depth_mode.mask);
                }
            }
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
            let attrs = self.base.vertex_attributes.get_or_insert_with(VertexAttributeArray::new_ptr).clone();
            let mut attrs = attrs.borrow_mut();
            if let Some(attrib) = attrs.set(self.impl_.vertex_attr_id, /*index=*/ -1, data_type, 0) {
                attrib.set_raw_data(data);
                attrib.set_stride(VertexAttribute::get_stride_of(data_type));
            } else {
                eprintln!("Vertex attribute type mismatch: {} / {}", self.base.name, self.impl_.vertex_attr_id);
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
