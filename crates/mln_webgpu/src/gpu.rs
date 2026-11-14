//! The [`Gpu`] interface on wgpu.

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use mln_gfx::gpu::validation::{self, BufferInfo, PipelineInfo, Resources, ShaderBinding, TextureInfo};
use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferDesc, BufferId, BufferUsage, Gpu, GpuError, Limits, LoadOp, Pass,
    PipelineId, PipelineKey, ReadbackId, RenderCommand, SamplerId, ShaderKey, SlotMap, TextureDesc, TextureFormat,
    TextureId, TextureRegion, TextureUsage,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_util::size::Size;

use crate::index_table::IndexTable;
use crate::shaders::shader_program::{self, BindingInfo, BindingType, ShaderProgram, validated};
use crate::writes::Writes;
use crate::{CustomLayerEncoder, CustomLayerPass, Device};

/// The size of the buffers that small uniform and storage buffers share.
const UNIFORM_PAGE: u64 = 16 << 10;

/// The alignment of a small uniform or storage buffer in its page: WebGPU's
/// largest allowed `minUniformBufferOffsetAlignment`, so an offset any
/// device binds.
const UNIFORM_ALIGNMENT: u64 = 256;

/// A buffer, or the part of a shared page that a small uniform or storage
/// buffer takes.
struct Buffer {
    buffer: wgpu::Buffer,
    usage: BufferUsage,
    /// Where the buffer starts in `buffer`.
    offset: u64,
    /// The bytes of `buffer` from `offset` that the buffer takes, which
    /// its writes and bindings stay in.
    size: u64,
    /// Of a buffer in a page, the end of the bytes written so far: those
    /// after it still hold the zeros it was made with. `None` for a buffer
    /// of its own.
    written: Option<u64>,
}

struct Texture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    shader: Rc<ShaderProgram>,
    key: PipelineKey,
    /// What the shader reads from the bind groups.
    bindings: Vec<ShaderBinding>,
}

/// A texture's texels, copied into a buffer that the CPU maps.
struct Readback {
    buffer: wgpu::Buffer,
    /// The bytes in each row of `buffer`, which WebGPU pads, and of the
    /// texels.
    padded_bytes_per_row: usize,
    bytes_per_row: usize,
    rows: usize,
    #[cfg_attr(target_family = "wasm", expect(dead_code, reason = "a browser can't wait for a submission"))]
    submission: wgpu::SubmissionIndex,
    /// How the map ended, once it has.
    mapped: Arc<OnceLock<Result<(), wgpu::BufferAsyncError>>>,
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
/// Small uniform and storage buffers share larger buffers, so that buffers
/// made together sit together and their writes join one copy. Writes are
/// gathered and copied into place ahead of the next submit's passes.
///
/// On a browser's WebGPU, a `create_*` call returns before the browser
/// validates it, so a rejected descriptor reaches the device's
/// uncaptured-error callback instead of the call's result.
pub struct WebGpu {
    device: Device,
    index_table: IndexTable,
    buffers: SlotMap<BufferId, Buffer>,
    /// The page that small uniform and storage buffers are taken from, and
    /// how many of its bytes are taken.
    uniform_page: Option<(wgpu::Buffer, u64)>,
    writes: Writes,
    textures: SlotMap<TextureId, Texture>,
    samplers: SlotMap<SamplerId, wgpu::Sampler>,
    shaders: HashMap<ShaderKey, Rc<ShaderProgram>>,
    pipelines: SlotMap<PipelineId, Pipeline>,
    bind_groups: SlotMap<BindGroupId, Vec<BindGroupEntry>>,
    native_groups: HashMap<NativeGroupKey, wgpu::BindGroup>,
    readbacks: SlotMap<ReadbackId, Readback>,
}

impl WebGpu {
    #[must_use]
    pub fn new(device: &Device) -> WebGpu {
        WebGpu {
            index_table: IndexTable::new(device.device()),
            device: device.clone(),
            buffers: SlotMap::with_key(),
            uniform_page: None,
            writes: Writes::default(),
            textures: SlotMap::with_key(),
            samplers: SlotMap::with_key(),
            shaders: HashMap::new(),
            pipelines: SlotMap::with_key(),
            bind_groups: SlotMap::with_key(),
            native_groups: HashMap::new(),
            readbacks: SlotMap::with_key(),
        }
    }

    /// Takes a texture made elsewhere, such as a surface's frame, as a
    /// texture of this backend. Destroying it drops this backend's
    /// reference.
    pub fn import_texture(&mut self, texture: &wgpu::Texture) -> TextureId {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.textures.insert(Texture { texture: texture.clone(), view })
    }

    /// The wgpu texture of `texture`.
    #[cfg(test)]
    pub(crate) fn native_texture(&self, texture: TextureId) -> Option<&wgpu::Texture> {
        self.textures.get(texture).map(|texture| &texture.texture)
    }

    /// Submits `command_buffer` after the copies of the writes made so far.
    fn submit_native(&mut self, command_buffer: wgpu::CommandBuffer) -> wgpu::SubmissionIndex {
        let writes = self.writes.encode(self.device.device(), self.device.queue());
        self.device.queue().submit(writes.into_iter().chain([command_buffer]))
    }

    /// Blocks until the GPU has finished `submission`, or everything
    /// submitted with `None`.
    #[cfg(not(target_family = "wasm"))]
    fn wait_for(&self, submission: Option<&wgpu::SubmissionIndex>) -> Result<(), GpuError> {
        /// How long the wait goes on asking after waits that reported a
        /// timeout.
        const WAIT_AGAIN_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);
        let device = self.device.device();
        let wait = || device.poll(wgpu::PollType::Wait { submission_index: submission.cloned(), timeout: None });
        // A wait without a timeout can still report one: PowerVR's Vulkan
        // driver on Android ends the wait shortly before the fence value
        // that wgpu reads afterwards shows the submission finished. Waiting
        // again returns at once then, so this asks until the value caught up.
        let start = mln_util::time::Instant::now();
        let mut waited = wait();
        while matches!(waited, Err(wgpu::PollError::Timeout)) && start.elapsed() < WAIT_AGAIN_LIMIT {
            waited = wait();
        }
        waited.map(drop).map_err(|error| GpuError::Invalid(format!("waiting for the GPU: {error}")))
    }

    /// `size` bytes of a page that small uniform and storage buffers share,
    /// zero filled: the page and their offset in it.
    fn allocate_uniform(&mut self, size: u64) -> Result<(wgpu::Buffer, u64), GpuError> {
        let size = size.next_multiple_of(UNIFORM_ALIGNMENT);
        let (page, taken) = match &mut self.uniform_page {
            Some((page, taken)) if *taken + size <= UNIFORM_PAGE => (page.clone(), taken),
            _ => {
                let page = self.create_uniform_buffer("uniform page", UNIFORM_PAGE)?;
                let (page, taken) = self.uniform_page.insert((page, 0));
                (page.clone(), taken)
            }
        };
        let offset = *taken;
        *taken += size;
        Ok((page, offset))
    }

    /// A buffer that binds as a uniform or a storage buffer.
    fn create_uniform_buffer(&self, label: &str, size: u64) -> Result<wgpu::Buffer, GpuError> {
        self.validated(label, |device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
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
                NativeResource::Buffer(buffer, offset, size) => {
                    let entry = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
                    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &entry.buffer,
                        offset: entry.offset + offset,
                        size: wgpu::BufferSize::new(size),
                    })
                }
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
        let mut render_pass = self.begin_pass(encoder, pass)?;
        self.encode_commands(&mut render_pass, &pass.label, &pass.commands)
    }

    /// Begins `pass` on `encoder`, without its commands.
    fn begin_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pass: &Pass,
    ) -> Result<wgpu::RenderPass<'static>, GpuError> {
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
        let render_pass = encoder
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
        Ok(render_pass)
    }

    /// Records `commands`, which the checks passed, into `render_pass`.
    fn encode_commands(
        &mut self,
        render_pass: &mut wgpu::RenderPass<'static>,
        label: &str,
        commands: &[RenderCommand],
    ) -> Result<(), GpuError> {
        let mut pipeline: Option<PipelineId> = None;
        let mut bound: Vec<Option<BindGroupId>> = Vec::new();
        let mut global_index = 0u32;
        // Whether the next draw sets its bind groups.
        let mut groups_dirty = true;
        for command in commands {
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
                    let entry = self.buffers.get(*buffer).ok_or(GpuError::UnknownId("buffer"))?;
                    render_pass.set_vertex_buffer(*slot, entry.buffer.slice(entry.offset + offset..));
                }
                RenderCommand::SetIndexBuffer { buffer, offset } => {
                    let entry = self.buffers.get(*buffer).ok_or(GpuError::UnknownId("buffer"))?;
                    let slice = entry.buffer.slice(entry.offset + offset..);
                    render_pass.set_index_buffer(slice, wgpu::IndexFormat::Uint16);
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
                    let id = pipeline.ok_or_else(|| GpuError::Invalid(format!("{label}: a draw before a pipeline")))?;
                    if groups_dirty {
                        self.set_groups(render_pass, id, &bound, global_index)?;
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
        // WGSL reads an alpha texture's one channel as red.
        TextureFormat::R8Unorm | TextureFormat::A8Unorm => wgpu::TextureFormat::R8Unorm,
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

/// What `shader` reads from the bind groups, by the shaders' ids.
fn shader_bindings(shader: &ShaderProgram) -> Result<Vec<ShaderBinding>, GpuError> {
    let mut bindings = Vec::new();
    for &group in shader.bind_group_order() {
        if Some(group) == shader.index_group() {
            continue;
        }
        for info in shader.binding_infos(group) {
            let binding = match info.type_ {
                BindingType::UniformBuffer | BindingType::ReadOnlyStorageBuffer | BindingType::StorageBuffer => {
                    ShaderBinding::Buffer { id: info.binding, min_size: shader.buffer_size(group, info.binding) }
                }
                BindingType::Sampler | BindingType::Texture | BindingType::UnfilterableTexture => {
                    let sampler = info.type_ == BindingType::Sampler;
                    let id = shader.texture_at(info.binding, sampler).ok_or_else(|| {
                        GpuError::Invalid(format!(
                            "{} reads binding {} with no texture id",
                            shader.name(),
                            info.binding
                        ))
                    })?;
                    ShaderBinding::Texture { id: id as u32 }
                }
            };
            if !bindings.contains(&binding) {
                bindings.push(binding);
            }
        }
    }
    Ok(bindings)
}

impl Resources for WebGpu {
    fn buffer_info(&self, id: BufferId) -> Option<BufferInfo> {
        // A buffer in a shared page, not the page.
        let buffer = self.buffers.get(id)?;
        Some(BufferInfo { usage: buffer.usage, size: buffer.size })
    }

    fn texture_info(&self, id: TextureId) -> Option<TextureInfo> {
        let texture = &self.textures.get(id)?.texture;
        let usage = texture.usage();
        Some(TextureInfo {
            format: from_wgpu_format(texture.format()),
            size: Size { width: texture.width(), height: texture.height() },
            usage: TextureUsage {
                sampled: usage.contains(wgpu::TextureUsages::TEXTURE_BINDING),
                render_target: usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
                copy_src: usage.contains(wgpu::TextureUsages::COPY_SRC),
            },
        })
    }

    fn has_sampler(&self, id: SamplerId) -> bool {
        self.samplers.contains_key(id)
    }

    fn pipeline_info(&self, id: PipelineId) -> Option<PipelineInfo<'_>> {
        let pipeline = self.pipelines.get(id)?;
        Some(PipelineInfo {
            targets: pipeline.key.targets,
            vertex_buffers: &pipeline.key.state.vertex_buffers,
            bindings: &pipeline.bindings,
        })
    }

    fn bind_group_entries(&self, id: BindGroupId) -> Option<&[BindGroupEntry]> {
        self.bind_groups.get(id).map(Vec::as_slice)
    }
}

/// The format of `format`, `None` for a format the interface has no name
/// for.
pub(crate) fn from_wgpu_format(format: wgpu::TextureFormat) -> Option<TextureFormat> {
    Some(match format {
        wgpu::TextureFormat::R8Unorm => TextureFormat::R8Unorm,
        wgpu::TextureFormat::R16Float => TextureFormat::R16Float,
        wgpu::TextureFormat::R32Float => TextureFormat::R32Float,
        wgpu::TextureFormat::Rgba8Unorm => TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb => TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm => TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb => TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgb10a2Unorm => TextureFormat::Rgb10a2Unorm,
        wgpu::TextureFormat::Rgba16Float => TextureFormat::Rgba16Float,
        wgpu::TextureFormat::Rgba32Float => TextureFormat::Rgba32Float,
        wgpu::TextureFormat::Depth32Float => TextureFormat::Depth32Float,
        wgpu::TextureFormat::Depth24PlusStencil8 => TextureFormat::Depth24PlusStencil8,
        wgpu::TextureFormat::Depth32FloatStencil8 => TextureFormat::Depth32FloatStencil8,
        _ => return None,
    })
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
        let size = desc.size.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        let usage = match desc.usage {
            BufferUsage::Vertex => wgpu::BufferUsages::VERTEX,
            BufferUsage::Index => wgpu::BufferUsages::INDEX,
            BufferUsage::Uniform | BufferUsage::Storage => {
                let (buffer, offset, written) = if size <= UNIFORM_PAGE / 4 {
                    let (page, offset) = self.allocate_uniform(size)?;
                    (page, offset, Some(0))
                } else {
                    (self.create_uniform_buffer(desc.label, size)?, 0, None)
                };
                return Ok(self.buffers.insert(Buffer { buffer, usage: desc.usage, offset, size, written }));
            }
        };
        let buffer = self.validated(desc.label, |device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(desc.label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;
        Ok(self.buffers.insert(Buffer { buffer, usage: desc.usage, offset: 0, size, written: None }))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.buffers.get_mut(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let end = offset + data.len() as u64;
        if end > entry.size {
            return Err(GpuError::Invalid(format!("a write to bytes {offset}..{end} of a buffer of {}", entry.size)));
        }
        // Where the rest of a buffer in a page still holds its zeros, they
        // can be written with the data, so that the copy joins the next
        // buffer's.
        let padded = match &mut entry.written {
            Some(written) if end >= *written => {
                *written = end;
                entry.size - offset
            }
            _ => data.len() as u64,
        };
        let (device, queue) = (self.device.device(), self.device.queue());
        self.writes.write_padded(device, queue, &entry.buffer, entry.offset + offset, data, padded);
        Ok(())
    }

    /// Writes from the JS array with `GPUQueue.writeBuffer`
    /// (`Writes::write_from_host`).
    #[cfg(target_family = "wasm")]
    fn write_buffer_from_host(
        &mut self,
        buffer: BufferId,
        offset: u64,
        data: &mln_util::host_bytes::HostBytes,
    ) -> Result<(), GpuError> {
        let entry = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let len = data.len() as u64;
        let end = offset + len.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if end > entry.size {
            return Err(GpuError::Invalid(format!("a write to bytes {offset}..{end} of a buffer of {}", entry.size)));
        }
        let (device, queue) = (self.device.device(), self.device.queue());
        let at = entry.offset + offset;
        let aligned = len & !(wgpu::COPY_BUFFER_ALIGNMENT - 1);
        self.writes.write_from_host(device, queue, &entry.buffer, at, data, aligned);
        if aligned < len
            && let Some(array) = data.array()
        {
            // writeBuffer takes whole 4-byte words.
            let mut tail = [0u8; wgpu::COPY_BUFFER_ALIGNMENT as usize];
            array.subarray(aligned as u32, len as u32).copy_to(&mut tail[..(len - aligned) as usize]);
            self.writes.write_padded(device, queue, &entry.buffer, at + aligned, &tail, wgpu::COPY_BUFFER_ALIGNMENT);
        }
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
            let shader =
                Rc::new(shader_program::compile(self.device.device(), self.index_table.layout(), &key.shader)?);
            self.shaders.insert(key.shader.clone(), Rc::clone(&shader));
            shader
        };
        let pipeline = shader.render_pipeline(
            &key.state,
            key.targets.color.map(texture_format),
            key.targets.depth_stencil.map(texture_format),
        )?;
        let bindings = shader_bindings(&shader)?;
        Ok(self.pipelines.insert(Pipeline { pipeline, shader, key: key.clone(), bindings }))
    }

    fn destroy_pipeline(&mut self, pipeline: PipelineId) {
        self.pipelines.remove(pipeline);
    }

    fn create_bind_group(&mut self, desc: &BindGroupDesc<'_>) -> Result<BindGroupId, GpuError> {
        validation::check_bind_group(self, self.limits(), desc)?;
        Ok(self.bind_groups.insert(desc.entries.to_vec()))
    }

    fn destroy_bind_group(&mut self, group: BindGroupId) {
        self.bind_groups.remove(group);
    }

    fn submit(&mut self, passes: &[Pass]) -> Result<(), GpuError> {
        for pass in passes {
            validation::check_pass(self, pass)?;
        }
        let mut encoder =
            self.device.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Gpu::submit") });
        for pass in passes {
            self.encode(&mut encoder, pass)?;
        }
        // wgpu reports what it rejects in the passes when they finish. The
        // writes stay for the next submit.
        let command_buffer = self.validated("submit", |_| encoder.finish())?;
        self.submit_native(command_buffer);
        Ok(())
    }

    /// Copies into a buffer that is mapped once the copy is done.
    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError> {
        let readback = validation::check_readback(self, texture, region)?;
        let texture = &self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?.texture;
        let padded_bytes_per_row = readback.bytes_per_row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let (width, rows) = (region.size.width, region.size.height);
        let buffer = self.validated("readback", |device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: (padded_bytes_per_row * rows as usize) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        })?;
        let mut encoder =
            self.device.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: region.x, y: region.y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row as u32),
                    rows_per_image: Some(rows),
                },
            },
            wgpu::Extent3d { width, height: rows, depth_or_array_layers: 1 },
        );
        let submission = self.submit_native(encoder.finish());
        let mapped = Arc::new(OnceLock::new());
        let map_ended = Arc::clone(&mapped);
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = map_ended.set(result);
        });
        Ok(self.readbacks.insert(Readback {
            buffer,
            padded_bytes_per_row,
            bytes_per_row: readback.bytes_per_row,
            rows: rows as usize,
            submission,
            mapped,
        }))
    }

    /// Polls the device for the map, or on a browser leaves it to the
    /// browser's event loop, which can't wait.
    fn take_readback(&mut self, readback: ReadbackId, wait: bool) -> Result<Option<Vec<u8>>, GpuError> {
        let entry = self.readbacks.get(readback).ok_or(GpuError::UnknownId("readback"))?;
        let mapped = Arc::clone(&entry.mapped);
        if mapped.get().is_none() {
            #[cfg(not(target_family = "wasm"))]
            if wait {
                if let Err(error) = self.wait_for(Some(&entry.submission)) {
                    self.readbacks.remove(readback);
                    return Err(error);
                }
            } else {
                let _ = self.device.device().poll(wgpu::PollType::Poll);
            }
            #[cfg(target_family = "wasm")]
            let _ = wait;
        }
        let Some(mapped) = mapped.get() else {
            return Ok(None);
        };
        let Readback { buffer, padded_bytes_per_row, bytes_per_row, rows, .. } =
            self.readbacks.remove(readback).ok_or(GpuError::UnknownId("readback"))?;
        if let Err(error) = mapped {
            return Err(GpuError::Invalid(format!("mapping a readback: {error}")));
        }
        let texels = {
            let range = buffer
                .get_mapped_range(..)
                .map_err(|error| GpuError::Invalid(format!("a readback's range: {error}")))?;
            let mut texels = Vec::with_capacity(bytes_per_row * rows);
            for row in range.chunks(padded_bytes_per_row).take(rows) {
                texels.extend_from_slice(&row[..bytes_per_row]);
            }
            texels
        };
        buffer.unmap();
        Ok(Some(texels))
    }

    fn destroy_readback(&mut self, readback: ReadbackId) {
        self.readbacks.remove(readback);
    }

    fn finish(&mut self) -> Result<(), GpuError> {
        #[cfg(not(target_family = "wasm"))]
        return self.wait_for(None);
        #[cfg(target_family = "wasm")]
        Ok(())
    }

    /// Lends a command encoder of its own, which is submitted after `f`
    /// returns. A pass that a custom layer draws into has a color
    /// attachment.
    fn lend_to_custom_layer(&mut self, pass: Option<&Pass>, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError> {
        if let Some(pass) = pass {
            validation::check_pass(self, pass)?;
        }
        let (device, queue) = (self.device.device().clone(), self.device.queue().clone());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("custom layer") });
        if let Some(pass) = pass {
            let format = |texture| self.textures.get(texture).map(|texture: &Texture| texture.texture.format());
            let color_format = pass.color.and_then(|attachment| format(attachment.texture)).ok_or_else(|| {
                GpuError::Unsupported(format!("{}: a custom layer's pass without a color attachment", pass.label))
            })?;
            let depth_stencil_format = pass.depth_stencil.and_then(|attachment| format(attachment.texture));
            let mut render_pass = self.begin_pass(&mut encoder, pass)?;
            self.encode_commands(&mut render_pass, &pass.label, &pass.commands)?;
            let mut lent = CustomLayerPass {
                device,
                queue,
                pass: render_pass,
                color_format,
                depth_stencil_format,
                sample_count: 1,
            };
            f(&mut lent);
        } else {
            let mut lent = CustomLayerEncoder { device, queue, encoder };
            f(&mut lent);
            encoder = lent.encoder;
        }
        let command_buffer = self.validated("custom layer", |_| encoder.finish())?;
        self.submit_native(command_buffer);
        Ok(())
    }
}
