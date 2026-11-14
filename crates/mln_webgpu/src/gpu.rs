//! The [`Gpu`] interface on wgpu.

use std::collections::HashMap;
use std::rc::Rc;

use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferDesc, BufferId, BufferUsage, Gpu, GpuError, Limits, LoadOp, Pass,
    PipelineId, PipelineKey, RenderCommand, SamplerId, ShaderKey, TextureDesc, TextureFormat, TextureId, TextureRegion,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use slotmap::SlotMap;

use crate::Device;
use crate::bind_groups::IndexTable;
use crate::shaders::shader_group;
use crate::shaders::shader_program::{BindingInfo, BindingType, ShaderProgram, validated};

struct Texture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    shader: Rc<ShaderProgram>,
}

/// A resource a native bind group binds: wgpu bind groups are made per
/// shader group, from what the bound [`BindGroupEntry`]s give its bindings.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum NativeResource {
    Buffer(BufferId, u64, u64),
    Sampler(SamplerId),
    Texture(TextureId),
}

/// What a native bind group is made from.
#[derive(Clone, PartialEq, Eq, Hash)]
struct NativeGroupKey {
    layout: wgpu::BindGroupLayout,
    resources: Vec<(u32, NativeResource)>,
}

/// A [`Gpu`] on a WebGPU [`Device`].
///
/// A shader's groups are WebGPU bind groups, made at the first draw that
/// binds their resources and kept until one of the resources is destroyed.
/// `globalIndex` is a dynamic offset into a table of indices.
///
/// On a browser's WebGPU, a `create_*` call returns before the browser
/// validates it, so a rejected descriptor reaches the device's
/// uncaptured-error callback instead of the call's result.
pub struct WebGpu {
    device: Device,
    index_table: IndexTable,
    buffers: SlotMap<BufferId, wgpu::Buffer>,
    textures: SlotMap<TextureId, Texture>,
    samplers: SlotMap<SamplerId, wgpu::Sampler>,
    shaders: HashMap<ShaderKey, Rc<ShaderProgram>>,
    pipelines: SlotMap<PipelineId, Pipeline>,
    bind_groups: SlotMap<BindGroupId, Vec<BindGroupEntry>>,
    native_groups: HashMap<NativeGroupKey, wgpu::BindGroup>,
}

impl WebGpu {
    #[must_use]
    pub fn new(device: &Device) -> WebGpu {
        WebGpu {
            index_table: IndexTable::new(device.device()),
            device: device.clone(),
            buffers: SlotMap::with_key(),
            textures: SlotMap::with_key(),
            samplers: SlotMap::with_key(),
            shaders: HashMap::new(),
            pipelines: SlotMap::with_key(),
            bind_groups: SlotMap::with_key(),
            native_groups: HashMap::new(),
        }
    }

    /// Takes a texture made elsewhere, such as a surface's frame, as a
    /// texture of this backend. Destroying it drops this backend's
    /// reference.
    pub fn import_texture(&mut self, texture: &wgpu::Texture) -> TextureId {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.textures.insert(Texture { texture: texture.clone(), view })
    }

    /// Runs `create` in a validation error scope, as the error of the call
    /// that `what` names.
    fn validated<T>(&self, what: &str, create: impl FnOnce(&wgpu::Device) -> T) -> Result<T, GpuError> {
        let device = self.device.device();
        validated(device, || create(device)).map_err(|error| GpuError::Invalid(format!("{what}: {error}")))
    }

    fn forget_native_groups(&mut self, uses: impl Fn(&NativeResource) -> bool) {
        self.native_groups.retain(|key, _| !key.resources.iter().any(|(_, resource)| uses(resource)));
    }

    /// The wgpu bind group that gives the bindings `infos` of `layout` what
    /// `bound` binds, made on first use.
    fn native_group(
        &mut self,
        shader: &ShaderProgram,
        layout: &wgpu::BindGroupLayout,
        infos: &[BindingInfo],
        bound: &[Option<BindGroupId>],
    ) -> Result<wgpu::BindGroup, GpuError> {
        // The highest slot that binds an id wins.
        let entries = bound.iter().rev().flatten().filter_map(|id| self.bind_groups.get(*id)).flatten();
        let mut resources = Vec::with_capacity(infos.len());
        for info in infos {
            let missing = || {
                GpuError::Invalid(format!(
                    "{} reads binding {} of group {}, which no bind group binds",
                    shader.name(),
                    info.binding,
                    info.group
                ))
            };
            let resource = match info.type_ {
                BindingType::UniformBuffer | BindingType::ReadOnlyStorageBuffer | BindingType::StorageBuffer => {
                    entries.clone().find_map(|entry| match *entry {
                        BindGroupEntry::Buffer { id, buffer, offset, size } if id == info.binding => {
                            Some(NativeResource::Buffer(buffer, offset, size))
                        }
                        _ => None,
                    })
                }
                BindingType::Sampler | BindingType::Texture | BindingType::UnfilterableTexture => {
                    let sampler = info.type_ == BindingType::Sampler;
                    let texture_id = shader.texture_at(info.binding, sampler).ok_or_else(missing)? as u32;
                    entries.clone().find_map(|entry| match *entry {
                        BindGroupEntry::Texture { id, texture, sampler: texture_sampler } if id == texture_id => {
                            Some(if sampler {
                                NativeResource::Sampler(texture_sampler)
                            } else {
                                NativeResource::Texture(texture)
                            })
                        }
                        _ => None,
                    })
                }
            };
            resources.push((info.binding, resource.ok_or_else(missing)?));
        }

        let key = NativeGroupKey { layout: layout.clone(), resources };
        if let Some(group) = self.native_groups.get(&key) {
            return Ok(group.clone());
        }
        let mut entries = Vec::with_capacity(key.resources.len());
        for &(binding, resource) in &key.resources {
            let resource = match resource {
                NativeResource::Buffer(buffer, offset, size) => wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?,
                    offset,
                    size: wgpu::BufferSize::new(size),
                }),
                NativeResource::Sampler(sampler) => {
                    wgpu::BindingResource::Sampler(self.samplers.get(sampler).ok_or(GpuError::UnknownId("sampler"))?)
                }
                NativeResource::Texture(texture) => wgpu::BindingResource::TextureView(
                    &self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?.view,
                ),
            };
            entries.push(wgpu::BindGroupEntry { binding, resource });
        }
        let group = self.validated("bind group", |device| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(shader.name()),
                layout: &key.layout,
                entries: &entries,
            })
        })?;
        drop(entries);
        self.native_groups.insert(key, group.clone());
        Ok(group)
    }

    /// Records `pass` into `encoder`.
    fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, pass: &Pass) -> Result<(), GpuError> {
        let view = |textures: &SlotMap<TextureId, Texture>, id| {
            textures.get(id).map(|texture| texture.view.clone()).ok_or(GpuError::UnknownId("texture"))
        };
        let color = pass
            .color
            .map(|attachment| Ok((view(&self.textures, attachment.texture)?, attachment.load)))
            .transpose()?;
        let depth_stencil = pass
            .depth_stencil
            .map(|attachment| {
                let texture = self.textures.get(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
                Ok((texture.view.clone(), texture.texture.format().has_stencil_aspect(), attachment))
            })
            .transpose()?;
        let mut render_pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&pass.label),
                color_attachments: &[color.as_ref().map(|(view, load)| wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: match load {
                            LoadOp::Load => wgpu::LoadOp::Load,
                            LoadOp::Clear(color) => wgpu::LoadOp::Clear(wgpu::Color {
                                r: f64::from(color.r),
                                g: f64::from(color.g),
                                b: f64::from(color.b),
                                a: f64::from(color.a),
                            }),
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: depth_stencil.as_ref().map(|(view, has_stencil, attachment)| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: match attachment.depth {
                                LoadOp::Load => wgpu::LoadOp::Load,
                                LoadOp::Clear(depth) => wgpu::LoadOp::Clear(depth),
                            },
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: has_stencil.then(|| wgpu::Operations {
                            load: match attachment.stencil {
                                LoadOp::Load => wgpu::LoadOp::Load,
                                LoadOp::Clear(stencil) => wgpu::LoadOp::Clear(stencil),
                            },
                            store: wgpu::StoreOp::Store,
                        }),
                    }
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();

        let mut pipeline: Option<PipelineId> = None;
        let mut bound: Vec<Option<BindGroupId>> = Vec::new();
        let mut global_index = 0u32;
        // Whether the next draw sets its bind groups.
        let mut groups_dirty = true;
        for command in &pass.commands {
            match command {
                RenderCommand::SetPipeline(id) => {
                    let entry = self.pipelines.get(*id).ok_or(GpuError::UnknownId("pipeline"))?;
                    render_pass.set_pipeline(&entry.pipeline);
                    pipeline = Some(*id);
                    groups_dirty = true;
                }
                RenderCommand::SetBindGroup { slot, group } => {
                    if !self.bind_groups.contains_key(*group) {
                        return Err(GpuError::UnknownId("bind group"));
                    }
                    let slot = *slot as usize;
                    if bound.len() <= slot {
                        bound.resize(slot + 1, None);
                    }
                    bound[slot] = Some(*group);
                    groups_dirty = true;
                }
                RenderCommand::ClearBindGroup { slot } => {
                    if let Some(group) = bound.get_mut(*slot as usize) {
                        *group = None;
                    }
                    groups_dirty = true;
                }
                RenderCommand::SetGlobalIndex(index) => {
                    global_index = *index;
                    groups_dirty = true;
                }
                RenderCommand::SetVertexBuffer { slot, buffer, offset } => {
                    let buffer = self.buffers.get(*buffer).ok_or(GpuError::UnknownId("buffer"))?;
                    render_pass.set_vertex_buffer(*slot, buffer.slice(*offset..));
                }
                RenderCommand::SetIndexBuffer { buffer, offset } => {
                    let buffer = self.buffers.get(*buffer).ok_or(GpuError::UnknownId("buffer"))?;
                    render_pass.set_index_buffer(buffer.slice(*offset..), wgpu::IndexFormat::Uint16);
                }
                RenderCommand::SetViewport(viewport) => render_pass.set_viewport(
                    viewport.x,
                    viewport.y,
                    viewport.width,
                    viewport.height,
                    viewport.min_depth,
                    viewport.max_depth,
                ),
                RenderCommand::SetScissor(scissor) => {
                    render_pass.set_scissor_rect(scissor.x, scissor.y, scissor.width, scissor.height);
                }
                RenderCommand::SetStencilReference(reference) => render_pass.set_stencil_reference(*reference),
                RenderCommand::SetBlendConstant(color) => render_pass.set_blend_constant(wgpu::Color {
                    r: f64::from(color.r),
                    g: f64::from(color.g),
                    b: f64::from(color.b),
                    a: f64::from(color.a),
                }),
                // WebGPU draws lines 1 pixel wide.
                RenderCommand::SetLineWidth(_) => {}
                RenderCommand::Draw { .. } | RenderCommand::DrawIndexed { .. } => {
                    let id = pipeline
                        .ok_or_else(|| GpuError::Invalid(format!("{}: a draw before a pipeline", pass.label)))?;
                    if groups_dirty {
                        self.set_groups(&mut render_pass, id, &bound, global_index)?;
                        groups_dirty = false;
                    }
                    match command {
                        RenderCommand::Draw { vertices, instances } => {
                            render_pass.draw(vertices.clone(), instances.clone());
                        }
                        RenderCommand::DrawIndexed { indices, base_vertex, instances } => {
                            render_pass.draw_indexed(indices.clone(), *base_vertex, instances.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// Sets the bind groups of pipeline `id`'s shader from what `bound`
    /// binds, and its index group at `global_index`.
    fn set_groups(
        &mut self,
        render_pass: &mut wgpu::RenderPass<'static>,
        id: PipelineId,
        bound: &[Option<BindGroupId>],
        global_index: u32,
    ) -> Result<(), GpuError> {
        let shader = Rc::clone(&self.pipelines.get(id).ok_or(GpuError::UnknownId("pipeline"))?.shader);
        for (slot, &group) in shader.bind_group_order().iter().enumerate() {
            if Some(group) == shader.index_group() {
                let device = self.device.device();
                self.index_table.reserve(device, self.device.queue(), global_index);
                let (index_group, offset) = self
                    .index_table
                    .get(global_index)
                    .ok_or_else(|| GpuError::Invalid(format!("global index {global_index}")))?;
                render_pass.set_bind_group(slot as u32, index_group, &[offset]);
                continue;
            }
            let layout = shader.bind_group_layout(group).ok_or(GpuError::UnknownId("bind group layout"))?;
            let native = self.native_group(&shader, layout, shader.binding_infos(group), bound)?;
            render_pass.set_bind_group(slot as u32, &native, &[]);
        }
        Ok(())
    }
}

fn texture_format(format: TextureFormat) -> wgpu::TextureFormat {
    match format {
        TextureFormat::R8Unorm => wgpu::TextureFormat::R8Unorm,
        TextureFormat::R16Float => wgpu::TextureFormat::R16Float,
        TextureFormat::R32Float => wgpu::TextureFormat::R32Float,
        TextureFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        TextureFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        TextureFormat::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        TextureFormat::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
        TextureFormat::Rgb10a2Unorm => wgpu::TextureFormat::Rgb10a2Unorm,
        TextureFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
        TextureFormat::Rgba32Float => wgpu::TextureFormat::Rgba32Float,
        TextureFormat::Depth32Float => wgpu::TextureFormat::Depth32Float,
        TextureFormat::Depth24PlusStencil8 => wgpu::TextureFormat::Depth24PlusStencil8,
        TextureFormat::Depth32FloatStencil8 => wgpu::TextureFormat::Depth32FloatStencil8,
    }
}

impl Gpu for WebGpu {
    fn limits(&self) -> Limits {
        let limits = self.device.device().limits();
        Limits {
            buffer_offset_alignment: limits
                .min_uniform_buffer_offset_alignment
                .max(limits.min_storage_buffer_offset_alignment),
            max_texture_size: limits.max_texture_dimension_2d,
        }
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        let usage = match desc.usage {
            BufferUsage::Vertex => wgpu::BufferUsages::VERTEX,
            BufferUsage::Index => wgpu::BufferUsages::INDEX,
            BufferUsage::Uniform => wgpu::BufferUsages::UNIFORM,
            BufferUsage::Storage => wgpu::BufferUsages::STORAGE,
        };
        let buffer = self.validated(desc.label, |device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(desc.label),
                size: desc.size.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;
        Ok(self.buffers.insert(buffer))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let buffer = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        self.device.queue().write_buffer(buffer, offset, data);
        Ok(())
    }

    fn destroy_buffer(&mut self, buffer: BufferId) {
        if self.buffers.remove(buffer).is_some() {
            self.forget_native_groups(|resource| matches!(*resource, NativeResource::Buffer(id, ..) if id == buffer));
        }
    }

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
        let mut usage = wgpu::TextureUsages::empty();
        if desc.usage.sampled {
            usage |= wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        }
        if desc.usage.render_target {
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        if desc.usage.copy_src {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        let format = texture_format(desc.format);
        let required = format.required_features();
        if !self.device.device().features().contains(required) {
            return Err(GpuError::Unsupported(format!("{:?} textures need {required:?}", desc.format)));
        }
        let texture = self.validated(desc.label, |device| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(desc.label),
                size: wgpu::Extent3d { width: desc.size.width, height: desc.size.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        })?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(self.textures.insert(Texture { texture, view }))
    }

    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        let texture = &self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?.texture;
        let bytes_per_texel = texture
            .format()
            .block_copy_size(None)
            .ok_or_else(|| GpuError::Invalid(format!("{:?} textures take no writes", texture.format())))?;
        let expected = u64::from(region.size.width) * u64::from(region.size.height) * u64::from(bytes_per_texel);
        if data.len() as u64 != expected {
            return Err(GpuError::Invalid(format!("a write of {} bytes to a region of {expected}", data.len())));
        }
        self.device.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: region.x, y: region.y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(region.size.width * bytes_per_texel),
                rows_per_image: Some(region.size.height),
            },
            wgpu::Extent3d { width: region.size.width, height: region.size.height, depth_or_array_layers: 1 },
        );
        Ok(())
    }

    fn destroy_texture(&mut self, texture: TextureId) {
        if self.textures.remove(texture).is_some() {
            self.forget_native_groups(|resource| *resource == NativeResource::Texture(texture));
        }
    }

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError> {
        let filter = match state.filter {
            TextureFilterType::Linear => wgpu::FilterMode::Linear,
            TextureFilterType::Nearest => wgpu::FilterMode::Nearest,
        };
        let address_mode = |wrap| match wrap {
            TextureWrapType::Repeat => wgpu::AddressMode::Repeat,
            TextureWrapType::Clamp => wgpu::AddressMode::ClampToEdge,
        };
        let sampler = self.validated("sampler", |device| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: None,
                address_mode_u: address_mode(state.wrap_u),
                address_mode_v: address_mode(state.wrap_v),
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: if state.mipmapped {
                    wgpu::MipmapFilterMode::Linear
                } else {
                    wgpu::MipmapFilterMode::Nearest
                },
                anisotropy_clamp: u16::from(state.max_anisotropy.max(1)),
                ..Default::default()
            })
        })?;
        Ok(self.samplers.insert(sampler))
    }

    fn destroy_sampler(&mut self, sampler: SamplerId) {
        if self.samplers.remove(sampler).is_some() {
            self.forget_native_groups(|resource| *resource == NativeResource::Sampler(sampler));
        }
    }

    fn create_pipeline(&mut self, key: &PipelineKey) -> Result<PipelineId, GpuError> {
        let shader = if let Some(shader) = self.shaders.get(&key.shader) {
            Rc::clone(shader)
        } else {
            let shader = Rc::new(shader_group::compile(self.device.device(), self.index_table.layout(), &key.shader)?);
            self.shaders.insert(key.shader.clone(), Rc::clone(&shader));
            shader
        };
        let pipeline = shader.render_pipeline(
            &key.state,
            texture_format(key.targets.color),
            key.targets.depth_stencil.map(texture_format),
        )?;
        Ok(self.pipelines.insert(Pipeline { pipeline, shader }))
    }

    fn destroy_pipeline(&mut self, pipeline: PipelineId) {
        self.pipelines.remove(pipeline);
    }

    fn create_bind_group(&mut self, desc: &BindGroupDesc<'_>) -> Result<BindGroupId, GpuError> {
        for entry in desc.entries {
            match *entry {
                BindGroupEntry::Buffer { buffer, .. } if !self.buffers.contains_key(buffer) => {
                    return Err(GpuError::UnknownId("buffer"));
                }
                BindGroupEntry::Texture { texture, .. } if !self.textures.contains_key(texture) => {
                    return Err(GpuError::UnknownId("texture"));
                }
                BindGroupEntry::Texture { sampler, .. } if !self.samplers.contains_key(sampler) => {
                    return Err(GpuError::UnknownId("sampler"));
                }
                _ => {}
            }
        }
        Ok(self.bind_groups.insert(desc.entries.to_vec()))
    }

    fn destroy_bind_group(&mut self, group: BindGroupId) {
        self.bind_groups.remove(group);
    }

    fn submit(&mut self, passes: &[Pass]) -> Result<(), GpuError> {
        let mut encoder =
            self.device.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Gpu::submit") });
        for pass in passes {
            self.encode(&mut encoder, pass)?;
        }
        self.device.queue().submit([encoder.finish()]);
        Ok(())
    }
}
