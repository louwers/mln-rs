//! The [`Gpu`] interface on wgpu.

use std::any::Any;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use mln_gfx::gpu::validation::{
    self, BindingsTags, BufferInfo, GeometryTags, LayoutTags, Lookup, PassTarget, PipelineTags, ShaderBinding,
    TextureInfo,
};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferDesc, BufferId, BufferUsage, Caps, ClipDepth, Draw, DynamicState,
    GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PassDesc, PipelineDesc, PipelineId, ReadbackId, ResourceId,
    SamplerId, Scissor, ShaderId, ShaderKey, SlotMap, TextureDesc, TextureFormat, TextureId, TextureRegion,
    TextureUsage,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_util::Color;
use mln_util::size::Size;

use crate::index_table::IndexTable;
use crate::shaders::shader_program::{self, BindingType, ShaderProgram, validated};
use crate::writes::Writes;
use crate::{CustomLayerEncoder, CustomLayerPass, Device};

/// The size of the buffers that small buffers share: uniform and storage
/// buffers in one kind of page, vertex and index buffers in another.
const PAGE: u64 = 16 << 10;

/// The alignment of a small uniform or storage buffer in its page: WebGPU's
/// largest allowed `minUniformBufferOffsetAlignment`, so an offset any
/// device binds.
const UNIFORM_ALIGNMENT: u64 = 256;

/// A buffer, or the part of a shared page that a small buffer takes.
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

/// A compiled program, and what it reads from its bindings.
struct Shader {
    program: Rc<ShaderProgram>,
    reads: Vec<ShaderBinding>,
}

struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    tags: PipelineTags,
}

/// A shader's bind groups, by the pipeline layout's slot, and the slot of
/// its group of `globalIndex`.
struct Bindings {
    groups: Vec<(u32, wgpu::BindGroup)>,
    index_slot: Option<u32>,
    tags: BindingsTags,
}

/// The vertex buffers and the index buffer of a draw, each with the range
/// of its wgpu buffer that it binds: from its offset to the end of its
/// buffer, which in a page is short of the page's end.
struct Geometry {
    vertex_buffers: Vec<(wgpu::Buffer, Range<u64>)>,
    index_buffer: (wgpu::Buffer, Range<u64>),
    /// The ids and offsets of the vertex buffers and of the index buffer,
    /// which a draw compares with what the pass has bound.
    vertex_ids: Vec<(BufferId, u64)>,
    index_id: (BufferId, u64),
    tags: GeometryTags,
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

/// The pass draws record into, and what the draws so far set on it.
struct OpenPass {
    pass: wgpu::RenderPass<'static>,
    target: PassTarget,
    /// The wgpu formats of its attachments, which a custom layer's pass
    /// tells.
    color_format: Option<wgpu::TextureFormat>,
    depth_stencil_format: Option<wgpu::TextureFormat>,
    /// The pipeline, bindings, global index and geometry set, `None` where
    /// a draw sets them again.
    pipeline: Option<PipelineId>,
    bindings: Option<BindingsId>,
    global_index: Option<u32>,
    geometry: Option<GeometryId>,
    /// The vertex buffers bound, by slot, and the index buffer, as
    /// [`Geometry::vertex_ids`]: geometries of other indices or attributes often
    /// share vertex buffers.
    vertex_buffers: Vec<(BufferId, u64)>,
    index_buffer: Option<(BufferId, u64)>,
    dynamic: Option<DynamicState>,
}

impl OpenPass {
    /// Forgets what the draws set, as after a custom layer drew.
    fn forget_state(&mut self) {
        self.pipeline = None;
        self.bindings = None;
        self.global_index = None;
        self.geometry = None;
        self.vertex_buffers.clear();
        self.index_buffer = None;
        self.dynamic = None;
    }
}

/// A [`Gpu`] on a WebGPU [`Device`].
///
/// A shader's groups are WebGPU bind groups, made with the bindings.
/// `globalIndex` is a dynamic offset into a table of indices. Draws record
/// into a pass of the frame's command encoder at once, setting only what
/// changed since the draw before.
///
/// Small buffers share larger ones, so that buffers made together sit
/// together, their writes join one copy, and a frame that remakes many
/// small buffers makes few wgpu buffers. Writes are gathered and copied into
/// place ahead of the next flush's commands.
///
/// On a browser's WebGPU, a `create_*` call returns before the browser
/// validates it, so a rejected descriptor reaches the device's
/// uncaptured-error callback instead of the call's result.
pub struct WebGpu {
    device: Device,
    caps: Caps,
    index_table: IndexTable,
    buffers: SlotMap<BufferId, Buffer>,
    /// The page that small uniform and storage buffers are taken from, and
    /// how many of its bytes are taken.
    uniform_page: Option<(wgpu::Buffer, u64)>,
    /// The page that small vertex and index buffers are taken from, and how
    /// many of its bytes are taken.
    geometry_page: Option<(wgpu::Buffer, u64)>,
    writes: Writes,
    textures: SlotMap<TextureId, Texture>,
    samplers: SlotMap<SamplerId, wgpu::Sampler>,
    shaders: SlotMap<ShaderId, Shader>,
    layout_tags: LayoutTags,
    pipelines: SlotMap<PipelineId, Pipeline>,
    bindings: SlotMap<BindingsId, Bindings>,
    geometries: SlotMap<GeometryId, Geometry>,
    readbacks: SlotMap<ReadbackId, Readback>,
    /// The commands recorded since the last flush.
    encoder: Option<wgpu::CommandEncoder>,
    pass: Option<OpenPass>,
}

impl WebGpu {
    /// How many textures the `WebGpu` has, imports included.
    #[cfg(test)]
    pub(crate) fn texture_count(&self) -> usize {
        self.textures.len()
    }

    #[must_use]
    pub fn new(device: &Device) -> WebGpu {
        let limits = device.device().limits();
        let caps = Caps {
            buffer_offset_alignment: limits
                .min_uniform_buffer_offset_alignment
                .max(limits.min_storage_buffer_offset_alignment),
            max_texture_size: limits.max_texture_dimension_2d,
            half_float_render_targets: true,
            clip_depth: ClipDepth::ZeroToOne,
            render_target_depth: Some(TextureFormat::Depth32Float),
            clear_stencil_with_pass: false,
            line_expressions: false,
            cpu_rasterizer: device.adapter_info().device_type == wgpu::DeviceType::Cpu,
            max_line_width: 1.0,
        };
        WebGpu {
            index_table: IndexTable::new(device.device()),
            device: device.clone(),
            caps,
            buffers: SlotMap::with_key(),
            uniform_page: None,
            geometry_page: None,
            writes: Writes::default(),
            textures: SlotMap::with_key(),
            samplers: SlotMap::with_key(),
            shaders: SlotMap::with_key(),
            layout_tags: LayoutTags::default(),
            pipelines: SlotMap::with_key(),
            bindings: SlotMap::with_key(),
            geometries: SlotMap::with_key(),
            readbacks: SlotMap::with_key(),
            encoder: None,
            pass: None,
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

    /// Submits the commands since the last flush after the copies of the
    /// writes made so far, with `then` recorded after them.
    fn submit(&mut self, then: impl FnOnce(&mut wgpu::CommandEncoder)) -> Result<wgpu::SubmissionIndex, GpuError> {
        let mut encoder = self.encoder.take().unwrap_or_else(|| {
            self.device.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") })
        });
        then(&mut encoder);
        // wgpu reports what it rejects in the passes when they finish. The
        // writes stay for the next flush.
        let command_buffer = self.validated("flush", |_| encoder.finish())?;
        let writes = self.writes.encode(self.device.device(), self.device.queue());
        Ok(self.device.queue().submit(writes.into_iter().chain([command_buffer])))
    }

    fn info(&self) -> (impl Fn(BufferId) -> Option<BufferInfo> + '_, impl Fn(TextureId) -> Option<TextureInfo> + '_) {
        let buffer =
            |id| self.buffers.get(id).map(|buffer: &Buffer| BufferInfo { usage: buffer.usage, size: buffer.size });
        let texture = |id| self.textures.get(id).map(|texture: &Texture| texture_info(&texture.texture));
        (buffer, texture)
    }

    /// Begins a pass of `desc`, which the checks passed, on the frame's
    /// encoder. Returns it, and the formats of its color and depth-stencil
    /// attachments.
    fn begin_render_pass(
        &mut self,
        desc: &PassDesc<'_>,
    ) -> Result<(wgpu::RenderPass<'static>, Option<wgpu::TextureFormat>, Option<wgpu::TextureFormat>), GpuError> {
        let texture = |id| self.textures.get(id).ok_or(GpuError::UnknownId("texture"));
        let color = desc
            .color
            .map(|attachment| Ok::<_, GpuError>((texture(attachment.texture)?, attachment.load)))
            .transpose()?;
        let depth_stencil = desc
            .depth_stencil
            .map(|attachment| Ok::<_, GpuError>((texture(attachment.texture)?, attachment)))
            .transpose()?;
        let color_format = color.map(|(texture, _)| texture.texture.format());
        let depth_stencil_format = depth_stencil.map(|(texture, _)| texture.texture.format());
        let color_attachment = color.map(|(texture, load)| wgpu::RenderPassColorAttachment {
            view: &texture.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: match load {
                    LoadOp::Load => wgpu::LoadOp::Load,
                    LoadOp::Clear(color) => wgpu::LoadOp::Clear(wgpu_color(color)),
                },
                store: wgpu::StoreOp::Store,
            },
        });
        let depth_stencil_attachment =
            depth_stencil.map(|(texture, attachment)| wgpu::RenderPassDepthStencilAttachment {
                view: &texture.view,
                depth_ops: Some(wgpu::Operations {
                    load: match attachment.depth {
                        LoadOp::Load => wgpu::LoadOp::Load,
                        LoadOp::Clear(depth) => wgpu::LoadOp::Clear(depth),
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: texture.texture.format().has_stencil_aspect().then_some(wgpu::Operations {
                    load: match attachment.stencil {
                        LoadOp::Load => wgpu::LoadOp::Load,
                        LoadOp::Clear(stencil) => wgpu::LoadOp::Clear(stencil),
                    },
                    store: wgpu::StoreOp::Store,
                }),
            });
        let device = self.device.device();
        let encoder = self.encoder.get_or_insert_with(|| {
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") })
        });
        let pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(desc.label),
                color_attachments: &[color_attachment],
                depth_stencil_attachment,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        Ok((pass, color_format, depth_stencil_format))
    }

    fn no_pass(what: &str) -> GpuError {
        GpuError::Invalid(format!("{what} outside a pass"))
    }

    fn open_pass(what: &str) -> GpuError {
        GpuError::Invalid(format!("{what} while a pass is open"))
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
        let mut page = self.uniform_page.take();
        let allocated = allocate_in_page(&mut page, size.next_multiple_of(UNIFORM_ALIGNMENT), || {
            self.create_uniform_buffer("uniform page", PAGE)
        });
        self.uniform_page = page;
        allocated
    }

    /// `size` bytes of a page that small vertex and index buffers share,
    /// zero filled: the page and their offset in it.
    fn allocate_geometry(&mut self, size: u64) -> Result<(wgpu::Buffer, u64), GpuError> {
        let mut page = self.geometry_page.take();
        let allocated = allocate_in_page(&mut page, size, || {
            self.validated("geometry page", |device| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("geometry page"),
                    size: PAGE,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
        });
        self.geometry_page = page;
        allocated
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
}

/// `size` bytes of `page`, or of a new page that `create` makes when it has
/// too few left: the page and the offset in it.
fn allocate_in_page(
    page: &mut Option<(wgpu::Buffer, u64)>,
    size: u64,
    create: impl FnOnce() -> Result<wgpu::Buffer, GpuError>,
) -> Result<(wgpu::Buffer, u64), GpuError> {
    let (buffer, taken) = match page {
        Some((buffer, taken)) if *taken + size <= PAGE => (buffer, taken),
        _ => {
            let (buffer, taken) = page.insert((create()?, 0));
            (buffer, taken)
        }
    };
    let offset = *taken;
    *taken += size;
    Ok((buffer.clone(), offset))
}

/// What the checks read of `texture`.
fn texture_info(texture: &wgpu::Texture) -> TextureInfo {
    let usage = texture.usage();
    TextureInfo {
        format: from_wgpu_format(texture.format()),
        size: Size { width: texture.width(), height: texture.height() },
        usage: TextureUsage {
            sampled: usage.contains(wgpu::TextureUsages::TEXTURE_BINDING),
            render_target: usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
            copy_src: usage.contains(wgpu::TextureUsages::COPY_SRC),
        },
    }
}

fn wgpu_color(color: Color) -> wgpu::Color {
    wgpu::Color { r: f64::from(color.r), g: f64::from(color.g), b: f64::from(color.b), a: f64::from(color.a) }
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

/// What `shader` reads from its bindings, by the shaders' ids.
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
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        let size = desc.size.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        let usage = match desc.usage {
            BufferUsage::Vertex | BufferUsage::Index if size <= PAGE / 4 => {
                let (buffer, offset) = self.allocate_geometry(size)?;
                let written = Some(0);
                return Ok(self.buffers.insert(Buffer { buffer, usage: desc.usage, offset, size, written }));
            }
            BufferUsage::Vertex => wgpu::BufferUsages::VERTEX,
            BufferUsage::Index => wgpu::BufferUsages::INDEX,
            BufferUsage::Uniform | BufferUsage::Storage => {
                let (buffer, offset, written) = if size <= PAGE / 4 {
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
        let entry = self.buffers.get_mut(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let len = data.len() as u64;
        let end = offset + len.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if end > entry.size {
            return Err(GpuError::Invalid(format!("a write to bytes {offset}..{end} of a buffer of {}", entry.size)));
        }
        // A later write past these bytes pads only past them.
        if let Some(written) = &mut entry.written {
            *written = (*written).max(offset + len);
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
        if !texture.usage().contains(wgpu::TextureUsages::COPY_DST) {
            return Err(GpuError::Invalid("a write to a texture that isn't sampled".to_owned()));
        }
        let format = from_wgpu_format(texture.format())
            .ok_or_else(|| GpuError::Invalid(format!("{:?} textures take no writes", texture.format())))?;
        let size = Size::new(texture.width(), texture.height());
        let bytes_per_row = region.write_bytes_per_row(size, format, data)?;
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
                bytes_per_row: Some(bytes_per_row as u32),
                rows_per_image: Some(region.size.height),
            },
            wgpu::Extent3d { width: region.size.width, height: region.size.height, depth_or_array_layers: 1 },
        );
        Ok(())
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

    fn create_shader(&mut self, key: &ShaderKey) -> Result<ShaderId, GpuError> {
        let program = shader_program::compile(self.device.device(), self.index_table.layout(), key)?;
        let reads = shader_bindings(&program)?;
        Ok(self.shaders.insert(Shader { program: Rc::new(program), reads }))
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<PipelineId, GpuError> {
        let shader = self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?;
        let pipeline = shader.program.render_pipeline(
            desc.vertex_buffers,
            &desc.state,
            desc.targets.color.map(texture_format),
            desc.targets.depth_stencil.map(texture_format),
        )?;
        let layout = self.layout_tags.tag(desc.shader, desc.vertex_buffers);
        let tags = PipelineTags { shader: desc.shader, layout, targets: desc.targets };
        Ok(self.pipelines.insert(Pipeline { pipeline, tags }))
    }

    fn create_bindings(&mut self, desc: &BindingsDesc<'_>) -> Result<BindingsId, GpuError> {
        let shader = self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?;
        let elements = {
            let (buffer, texture) = self.info();
            let sampler = |id| self.samplers.contains_key(id);
            let lookup = Lookup { buffer: &buffer, texture: &texture, sampler: &sampler };
            validation::check_bindings(desc, &shader.reads, self.caps.buffer_offset_alignment, &lookup)?
        };
        let program = &shader.program;
        let mut groups = Vec::new();
        let mut index_slot = None;
        for (slot, &group) in program.bind_group_order().iter().enumerate() {
            if Some(group) == program.index_group() {
                index_slot = Some(slot as u32);
                continue;
            }
            let layout = program.bind_group_layout(group).ok_or(GpuError::UnknownId("bind group layout"))?;
            let mut entries = Vec::new();
            for info in program.binding_infos(group) {
                let missing = || {
                    GpuError::Invalid(format!(
                        "{} reads binding {} of group {}, which no entry binds",
                        program.name(),
                        info.binding,
                        info.group
                    ))
                };
                let resource = match info.type_ {
                    BindingType::UniformBuffer | BindingType::ReadOnlyStorageBuffer | BindingType::StorageBuffer => {
                        let (buffer, offset, size) = desc
                            .entries
                            .iter()
                            .find_map(|entry| match *entry {
                                BindGroupEntry::Buffer { id, buffer, offset, size } if id == info.binding => {
                                    Some((buffer, offset, size))
                                }
                                _ => None,
                            })
                            .ok_or_else(missing)?;
                        let buffer = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
                        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer.buffer,
                            offset: buffer.offset + offset,
                            size: wgpu::BufferSize::new(size),
                        })
                    }
                    BindingType::Sampler | BindingType::Texture | BindingType::UnfilterableTexture => {
                        let sampler = info.type_ == BindingType::Sampler;
                        let texture_id = program.texture_at(info.binding, sampler).ok_or_else(missing)? as u32;
                        let (texture, texture_sampler) = desc
                            .entries
                            .iter()
                            .find_map(|entry| match *entry {
                                BindGroupEntry::Texture { id, texture, sampler } if id == texture_id => {
                                    Some((texture, sampler))
                                }
                                _ => None,
                            })
                            .ok_or_else(missing)?;
                        if sampler {
                            wgpu::BindingResource::Sampler(
                                self.samplers.get(texture_sampler).ok_or(GpuError::UnknownId("sampler"))?,
                            )
                        } else {
                            wgpu::BindingResource::TextureView(
                                &self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?.view,
                            )
                        }
                    }
                };
                entries.push(wgpu::BindGroupEntry { binding: info.binding, resource });
            }
            let group = self.validated(desc.label, |device| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(desc.label),
                    layout,
                    entries: &entries,
                })
            })?;
            groups.push((slot as u32, group));
        }
        let tags = BindingsTags { shader: desc.shader, elements };
        Ok(self.bindings.insert(Bindings { groups, index_slot, tags }))
    }

    fn create_geometry(&mut self, desc: &GeometryDesc<'_>) -> Result<GeometryId, GpuError> {
        if !self.shaders.contains_key(desc.shader) {
            return Err(GpuError::UnknownId("shader"));
        }
        let ranges = {
            let (buffer, texture) = self.info();
            let lookup = Lookup { buffer: &buffer, texture: &texture, sampler: &|_| false };
            validation::check_geometry(desc, &lookup)?
        };
        let native = |id: BufferId, offset: u64| {
            let buffer = self.buffers.get(id).ok_or(GpuError::UnknownId("buffer"))?;
            Ok::<_, GpuError>((buffer.buffer.clone(), buffer.offset + offset..buffer.offset + buffer.size))
        };
        let vertex_buffers =
            desc.vertex_buffers.iter().map(|&(id, offset)| native(id, offset)).collect::<Result<Vec<_>, _>>()?;
        let index_buffer = native(desc.index_buffer, 0)?;
        let tags = GeometryTags { layout: self.layout_tags.tag(desc.shader, desc.layouts), ranges };
        Ok(self.geometries.insert(Geometry {
            vertex_buffers,
            index_buffer,
            vertex_ids: desc.vertex_buffers.to_vec(),
            index_id: (desc.index_buffer, 0),
            tags,
        }))
    }

    /// Drops this backend's references: wgpu keeps a resource while
    /// bindings, geometry or recorded commands hold it.
    fn destroy(&mut self, resource: ResourceId) {
        match resource {
            ResourceId::Buffer(id) => drop(self.buffers.remove(id)),
            ResourceId::Texture(id) => drop(self.textures.remove(id)),
            ResourceId::Sampler(id) => drop(self.samplers.remove(id)),
            ResourceId::Shader(id) => drop(self.shaders.remove(id)),
            ResourceId::Pipeline(id) => drop(self.pipelines.remove(id)),
            ResourceId::Bindings(id) => drop(self.bindings.remove(id)),
            ResourceId::Geometry(id) => drop(self.geometries.remove(id)),
            ResourceId::Readback(id) => drop(self.readbacks.remove(id)),
        }
    }

    fn begin_pass(&mut self, desc: &PassDesc<'_>) -> Result<(), GpuError> {
        if self.pass.is_some() {
            return Err(Self::open_pass("a pass"));
        }
        let target = {
            let (_, texture) = self.info();
            validation::check_pass(desc, &texture)?
        };
        let (pass, color_format, depth_stencil_format) = self.begin_render_pass(desc)?;
        self.pass = Some(OpenPass {
            pass,
            target,
            color_format,
            depth_stencil_format,
            pipeline: None,
            bindings: None,
            global_index: None,
            geometry: None,
            vertex_buffers: Vec::new(),
            index_buffer: None,
            dynamic: Some(DynamicState {
                depth_range: (0.0, 1.0),
                scissor: Scissor { x: 0, y: 0, width: target.size.width, height: target.size.height },
                stencil_reference: 0,
                blend_constant: Color::new_unchecked(0.0, 0.0, 0.0, 0.0),
                line_width: 1.0,
            }),
        });
        Ok(())
    }

    fn draw(&mut self, draw: &Draw) -> Result<(), GpuError> {
        let open = self.pass.as_mut().ok_or_else(|| Self::no_pass("a draw"))?;
        let pipeline = self.pipelines.get(draw.pipeline).ok_or(GpuError::UnknownId("pipeline"))?;
        let bindings = self.bindings.get(draw.bindings).ok_or(GpuError::UnknownId("bindings"))?;
        let geometry = self.geometries.get(draw.geometry).ok_or(GpuError::UnknownId("geometry"))?;
        validation::check_draw(draw, open.target.targets, &pipeline.tags, &bindings.tags, &geometry.tags)?;
        let pass = &mut open.pass;
        if open.pipeline != Some(draw.pipeline) {
            pass.set_pipeline(&pipeline.pipeline);
            open.pipeline = Some(draw.pipeline);
        }
        let bindings_changed = open.bindings != Some(draw.bindings);
        if bindings_changed {
            for (slot, group) in &bindings.groups {
                pass.set_bind_group(*slot, group, &[]);
            }
            open.bindings = Some(draw.bindings);
        }
        if let Some(slot) = bindings.index_slot
            && (bindings_changed || open.global_index != Some(draw.global_index))
        {
            self.index_table.reserve(self.device.device(), self.device.queue(), draw.global_index);
            let (group, offset) = self
                .index_table
                .get(draw.global_index)
                .ok_or_else(|| GpuError::Invalid(format!("global index {}", draw.global_index)))?;
            pass.set_bind_group(slot, group, &[offset]);
            open.global_index = Some(draw.global_index);
        }
        if open.geometry != Some(draw.geometry) {
            for (slot, ((buffer, range), id)) in geometry.vertex_buffers.iter().zip(&geometry.vertex_ids).enumerate() {
                if open.vertex_buffers.get(slot) != Some(id) {
                    pass.set_vertex_buffer(slot as u32, buffer.slice(range.clone()));
                    match open.vertex_buffers.get_mut(slot) {
                        Some(bound) => *bound = *id,
                        None => open.vertex_buffers.push(*id),
                    }
                }
            }
            if open.index_buffer != Some(geometry.index_id) {
                let (buffer, range) = &geometry.index_buffer;
                pass.set_index_buffer(buffer.slice(range.clone()), wgpu::IndexFormat::Uint16);
                open.index_buffer = Some(geometry.index_id);
            }
            open.geometry = Some(draw.geometry);
        }
        let dynamic = draw.dynamic;
        let last = open.dynamic.replace(dynamic);
        let changed = |same: fn(&DynamicState, &DynamicState) -> bool| last.is_none_or(|last| !same(&last, &dynamic));
        if changed(|a, b| a.depth_range == b.depth_range) {
            let size = open.target.size;
            let (min, max) = dynamic.depth_range;
            pass.set_viewport(0.0, 0.0, size.width as f32, size.height as f32, min, max);
        }
        if changed(|a, b| a.scissor == b.scissor) {
            let Scissor { x, y, width, height } = dynamic.scissor;
            pass.set_scissor_rect(x, y, width, height);
        }
        if changed(|a, b| a.stencil_reference == b.stencil_reference) {
            pass.set_stencil_reference(dynamic.stencil_reference);
        }
        if changed(|a, b| a.blend_constant == b.blend_constant) {
            pass.set_blend_constant(wgpu_color(dynamic.blend_constant));
        }
        // WebGPU draws lines 1 pixel wide.
        let indices = draw.first_index..draw.first_index + draw.index_count;
        let instances = draw.first_instance..draw.first_instance + draw.instance_count;
        pass.draw_indexed(indices, draw.base_vertex, instances);
        Ok(())
    }

    /// Lends the open pass, or the frame's command encoder. A pass that a
    /// custom layer draws into has a color attachment.
    fn lend(&mut self, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError> {
        let (device, queue) = (self.device.device().clone(), self.device.queue().clone());
        if let Some(mut open) = self.pass.take() {
            let Some(color_format) = open.color_format else {
                self.pass = Some(open);
                return Err(GpuError::Unsupported("a custom layer's pass without a color attachment".to_owned()));
            };
            let depth_stencil_format = open.depth_stencil_format;
            let mut lent =
                CustomLayerPass { device, queue, pass: open.pass, color_format, depth_stencil_format, sample_count: 1 };
            f(&mut lent);
            open.pass = lent.pass;
            open.forget_state();
            self.pass = Some(open);
            return Ok(());
        }
        let encoder = self
            .encoder
            .take()
            .unwrap_or_else(|| device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") }));
        let mut lent = CustomLayerEncoder { device, queue, encoder };
        f(&mut lent);
        self.encoder = Some(lent.encoder);
        Ok(())
    }

    fn end_pass(&mut self) -> Result<(), GpuError> {
        self.pass.take().map(drop).ok_or_else(|| Self::no_pass("the end of a pass"))
    }

    fn flush(&mut self, wait: bool) -> Result<(), GpuError> {
        if self.pass.is_some() {
            return Err(Self::open_pass("a flush"));
        }
        let submission = self.submit(|_| {})?;
        #[cfg(not(target_family = "wasm"))]
        if wait {
            return self.wait_for(Some(&submission));
        }
        let _ = (wait, submission);
        Ok(())
    }

    /// Copies into a buffer that is mapped once the copy is done.
    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError> {
        if self.pass.is_some() {
            return Err(Self::open_pass("a readback"));
        }
        let texture = self.textures.get(texture).map(|texture| texture.texture.clone());
        let readback = validation::check_readback(texture.as_ref().map(texture_info), region)?;
        let texture = texture.ok_or(GpuError::UnknownId("texture"))?;
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
        let submission = self.submit(|encoder| {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
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
        })?;
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
            if padded_bytes_per_row == bytes_per_row {
                range[..bytes_per_row * rows].to_vec()
            } else {
                let mut texels = Vec::with_capacity(bytes_per_row * rows);
                for row in range.chunks(padded_bytes_per_row).take(rows) {
                    texels.extend_from_slice(&row[..bytes_per_row]);
                }
                texels
            }
        };
        buffer.unmap();
        Ok(Some(texels))
    }
}
