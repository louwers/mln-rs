//! The [`Gpu`] interface on D3D12.

use std::any::Any;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;

use gpu_allocator::MemoryLocation;
use mln_gfx::gpu::validation::{
    self, BindingsTags, BufferInfo, GeometryTags, LayoutTags, Lookup, PassTarget, PipelineTags, ShaderBinding,
    TextureInfo,
};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferDesc, BufferId, BufferUsage, Caps, ClipDepthRange, Draw,
    DynamicState, GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PassDesc, PipelineDesc, PipelineId, ReadbackId,
    ResourceId, SamplerId, Scissor, ShaderId, ShaderKey, SlotMap, TextureDesc, TextureFormat, TextureId, TextureRegion,
    TextureUsage, VertexStep,
};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, LAYER_SSBO_START_ID};
use mln_gfx::texture2d::SamplerState;
use mln_glsl::TextureInfo as ShaderTexture;
use mln_glsl::layout::{DescriptorSetType, IMAGE_SAMPLER_START, LAYER_STORAGE_COUNT};
use mln_util::hash::{HashMap, HashSet};
use mln_util::size::Size;
use windows::Win32::Foundation::{HANDLE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_BOX, D3D12_CLEAR_FLAG_DEPTH, D3D12_CLEAR_FLAG_STENCIL, D3D12_CLEAR_VALUE, D3D12_CLEAR_VALUE_0,
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT, D3D12_CPU_DESCRIPTOR_HANDLE,
    D3D12_DEPTH_STENCIL_VALUE, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_FENCE_FLAG_NONE, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED, D3D12_INDEX_BUFFER_VIEW, D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
    D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_PRIMITIVE_TOPOLOGY_TYPE, D3D12_RANGE, D3D12_RENDER_PASS_BEGINNING_ACCESS,
    D3D12_RENDER_PASS_BEGINNING_ACCESS_0, D3D12_RENDER_PASS_BEGINNING_ACCESS_CLEAR_PARAMETERS,
    D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_CLEAR, D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_NO_ACCESS,
    D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_PRESERVE, D3D12_RENDER_PASS_DEPTH_STENCIL_DESC,
    D3D12_RENDER_PASS_ENDING_ACCESS, D3D12_RENDER_PASS_ENDING_ACCESS_TYPE_NO_ACCESS,
    D3D12_RENDER_PASS_ENDING_ACCESS_TYPE_PRESERVE, D3D12_RENDER_PASS_FLAG_NONE, D3D12_RENDER_PASS_RENDER_TARGET_DESC,
    D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION, D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
    D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET,
    D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_SHADER_BYTECODE, D3D12_SUBRESOURCE_FOOTPRINT,
    D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0, D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
    D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, D3D12_TEXTURE_DATA_PITCH_ALIGNMENT,
    D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT, D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12_VERTEX_BUFFER_VIEW, D3D12_VIEWPORT,
    ID3D12CommandAllocator, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12GraphicsCommandList4, ID3D12PipelineState,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R16_UINT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, s};

use crate::heaps::{CpuDescriptors, CpuSlot, SamplerHeap, TABLE_SIZE, TableHeap};
use crate::memory::{BufferMemory, Memory, NO_RANGE, TextureMemory};
use crate::root_signature::{self, ROOT_CONSTANTS, SAMPLER_INDICES, SAMPLER_TABLE, TEXTURE_TABLE, View};
use crate::shaders::{CompiledProgram, ShaderCompiler};
use crate::upload::UploadRing;
use crate::{CustomLayerEncoder, CustomLayerPass, Device, conv};

/// The alignment of a binding's offset: what a root CBV needs.
const BUFFER_ALIGNMENT: u64 = D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT as u64;
/// The widest and tallest texture of feature level 11.0.
const MAX_TEXTURE_SIZE: u32 = D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION;
/// The windows of each uniform and storage buffer: the batches in flight.
const WINDOWS: usize = 2;

/// A buffer: a uniform or storage buffer's windows in upload memory, or a
/// vertex or index buffer in GPU memory, which copies write.
struct Buffer {
    memory: Rc<BufferMemory>,
    usage: BufferUsage,
    size: u64,
    windows: Option<Windows>,
}

/// A uniform or storage buffer's contents, and a window of them for each
/// batch that may be in flight, so that the CPU writes the window of the
/// batch being recorded while the GPU reads the others.
struct Windows {
    contents: Vec<u8>,
    /// The bytes from the start of one window to the next.
    stride: u64,
    /// A bit for each window that holds `contents`.
    current: u32,
}

impl Windows {
    /// Copies `contents[start..end]` into `window` of `memory`.
    fn copy(&self, memory: &BufferMemory, window: usize, start: usize, end: usize) {
        let at = window * self.stride as usize;
        // SAFETY: the batches that read the window have finished
        // (`D3d12Gpu::open_window`).
        unsafe { memory.write(at + start, &self.contents[start..end]) };
    }
}

struct Texture {
    memory: Rc<TextureMemory>,
    info: TextureInfo,
    format: DXGI_FORMAT,
    /// The state the texture is in between passes and copies, which they
    /// return it to.
    rest: D3D12_RESOURCE_STATES,
    /// Its shader resource view, in a CPU-only heap, which bindings copy
    /// into their table, and its render target or depth-stencil view.
    srv: Option<CpuSlot>,
    rtv: Option<CpuSlot>,
    dsv: Option<CpuSlot>,
}

/// A compiled program, what it reads from its bindings, and where.
struct Shader {
    key: ShaderKey,
    program: Arc<CompiledProgram>,
    reads: Vec<ShaderBinding>,
    /// The buffers it reads, by id, with their root parameter and view.
    buffers: Vec<(u32, u32, View)>,
    /// The textures it reads, by id, with their index in the table.
    textures: Vec<(u32, u32)>,
}

struct Pipeline {
    state: ID3D12PipelineState,
    topology: D3D_PRIMITIVE_TOPOLOGY,
    tags: PipelineTags,
}

/// A buffer that bindings bind: its root parameter and view, the address of
/// the range in its first window, and the bytes to each next window.
#[derive(Clone, Copy)]
struct BoundBuffer {
    parameter: u32,
    view: View,
    address: u64,
    stride: u64,
}

/// What a draw sets in the root signature: each buffer's address, the
/// texture table and the address of the sampler indices.
struct Bindings {
    buffers: Vec<BoundBuffer>,
    textures: Option<(u32, u64)>,
    /// The memory they bind, which lives as long as the bindings.
    #[expect(dead_code, reason = "held, not read")]
    keep: (Vec<Rc<BufferMemory>>, Vec<Rc<TextureMemory>>),
    tags: BindingsTags,
}

struct Geometry {
    vertex_views: Vec<D3D12_VERTEX_BUFFER_VIEW>,
    index_view: D3D12_INDEX_BUFFER_VIEW,
    /// The buffers it reads, which live as long as the geometry.
    keep: Vec<Rc<BufferMemory>>,
    tags: GeometryTags,
}

/// A texture's texels, copied into readback memory, which the CPU reads
/// once the batch `serial` has finished.
struct Readback {
    memory: BufferMemory,
    /// The bytes in each row of `memory`, which D3D12 pads, and of the
    /// texels.
    row_pitch: usize,
    bytes_per_row: usize,
    rows: usize,
    serial: u64,
}

/// A command list and its allocator.
struct Recording {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
}

/// An attachment of a pass, which the end of the pass returns to its rest,
/// even if it was destroyed: bindings may still sample it.
struct Attachment {
    memory: Rc<TextureMemory>,
    rest: D3D12_RESOURCE_STATES,
    view: D3D12_CPU_DESCRIPTOR_HANDLE,
    format: DXGI_FORMAT,
}

/// The render pass draws record into, and what the draws so far set on it.
struct OpenPass {
    list: ID3D12GraphicsCommandList4,
    target: PassTarget,
    /// The window of uniform and storage buffers that the batch reads.
    window: usize,
    color: Option<Attachment>,
    depth_stencil: Option<Attachment>,
    pipeline: Option<PipelineId>,
    bindings: Option<BindingsId>,
    global_index: Option<u32>,
    geometry: Option<GeometryId>,
    /// The views of the vertex buffers bound, by slot, and of the index
    /// buffer: geometries of other indices or attributes often share them.
    vertex_buffers: Vec<D3D12_VERTEX_BUFFER_VIEW>,
    index_buffer: Option<D3D12_INDEX_BUFFER_VIEW>,
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

/// What a resource left behind when it was destroyed, held until the work
/// recorded until then has finished. Memory goes back when its last user
/// drops (`memory`).
enum Garbage {
    Texture(Texture),
    Bindings(Bindings),
    Held(#[expect(dead_code, reason = "held, not read")] Box<dyn Any>),
}

/// A [`Gpu`] on a D3D12 [`Device`].
///
/// Every pipeline has the device's root signature (`root_signature`), so a draw
/// binds its buffers as root CBVs and SRVs, its textures as a table that
/// its bindings made, and its samplers through the indices that its
/// bindings point at.
///
/// Memory comes from gpu-allocator, small buffers from slabs (`memory`).
/// Uniform and storage buffers have a window in upload memory for each batch
/// in flight, which the CPU writes. Vertex and index buffers live in GPU
/// memory, and their writes, and textures', are copies from upload memory in
/// a command list that runs ahead of each flush's. Textures rest in one
/// state, which passes and copies return them to.
///
/// A process compiles each program once, and a device makes each pipeline
/// state once for the backends on it.
///
/// Destroying a resource frees it once the work recorded until then has
/// finished, as each flush signals a fence.
pub struct D3d12Gpu {
    device: Device,
    compiler: ShaderCompiler,
    caps: Caps,
    fence: ID3D12Fence,
    /// The batch that the work being recorded will be.
    serial: u64,
    /// The batch whose window of uniform and storage buffers is ready, and
    /// its window.
    window: (u64, usize),
    buffers: SlotMap<BufferId, Buffer>,
    /// The buffers that some window doesn't hold the contents of.
    stale: Vec<BufferId>,
    /// The windows of destroyed buffers that bindings still bind, which
    /// each batch brings up to date until they all hold the last contents.
    retired: Vec<(Rc<BufferMemory>, Windows)>,
    upload: UploadRing,
    /// The resources of the vertex and index buffers that the copies of the
    /// batch write, which they transition to and from the copy state.
    copied: HashSet<usize>,
    copied_resources: Vec<ID3D12Resource>,
    textures: SlotMap<TextureId, Texture>,
    srv_heap: CpuDescriptors,
    rtv_heap: CpuDescriptors,
    dsv_heap: CpuDescriptors,
    tables: TableHeap,
    sampler_heap: SamplerHeap,
    samplers: SlotMap<SamplerId, u32>,
    /// The upload memory that holds each set of sampler indices bindings
    /// point at, kept as long as the `D3d12Gpu`, as there are few sets.
    sampler_indices: HashMap<[u32; TABLE_SIZE as usize], BufferMemory>,
    shaders: SlotMap<ShaderId, Shader>,
    layout_tags: LayoutTags,
    pipelines: SlotMap<PipelineId, Pipeline>,
    bindings: SlotMap<BindingsId, Bindings>,
    geometries: SlotMap<GeometryId, Geometry>,
    readbacks: SlotMap<ReadbackId, Readback>,
    /// The memory of the last readback taken, which the next readback of
    /// its size copies into, as a frame read back each time is.
    spare_readback: Option<BufferMemory>,
    /// The writes since the last flush, and the commands.
    copies: Option<Recording>,
    frame: Option<Recording>,
    pass: Option<OpenPass>,
    free_recordings: Vec<Recording>,
    in_flight: VecDeque<(u64, Recording)>,
    garbage: Vec<(u64, Garbage)>,
    /// The fences and values that the queue waits for before the next
    /// submit, and signals after it.
    waits: Vec<(ID3D12Fence, u64)>,
    signals: Vec<(ID3D12Fence, u64)>,
    /// The fences and values that the first pass to draw into a texture has
    /// the queue wait for: the host's reads of it.
    texture_waits: HashMap<TextureId, (ID3D12Fence, u64)>,
    /// Last, so that what holds memory drops before it.
    memory: Memory,
}

fn invalid(what: &str) -> impl Fn(windows::core::Error) -> GpuError + '_ {
    move |error| GpuError::Invalid(format!("{what}: {error}"))
}

/// A transition of `resource` from `before` to `after`.
fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: a copy of the pointer without a reference, which
                // the barrier's ManuallyDrop never releases.
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

/// The state of a texture between passes and copies.
fn rest_state(format: Option<TextureFormat>, usage: TextureUsage) -> D3D12_RESOURCE_STATES {
    let depth = format.is_some_and(conv::is_depth);
    if usage.sampled {
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE
    } else if usage.render_target && depth {
        D3D12_RESOURCE_STATE_DEPTH_WRITE
    } else if usage.render_target {
        D3D12_RESOURCE_STATE_RENDER_TARGET
    } else {
        D3D12_RESOURCE_STATE_COMMON
    }
}

fn color(color: mln_util::Color) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

/// How a render pass begins with an attachment's contents.
fn beginning(load: LoadOp<D3D12_CLEAR_VALUE>) -> D3D12_RENDER_PASS_BEGINNING_ACCESS {
    match load {
        LoadOp::Load => D3D12_RENDER_PASS_BEGINNING_ACCESS {
            Type: D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_PRESERVE,
            Anonymous: D3D12_RENDER_PASS_BEGINNING_ACCESS_0::default(),
        },
        LoadOp::Clear(value) => D3D12_RENDER_PASS_BEGINNING_ACCESS {
            Type: D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_CLEAR,
            Anonymous: D3D12_RENDER_PASS_BEGINNING_ACCESS_0 {
                Clear: D3D12_RENDER_PASS_BEGINNING_ACCESS_CLEAR_PARAMETERS { ClearValue: value },
            },
        },
    }
}

/// How a render pass ends with an attachment's contents: it keeps them.
fn preserve() -> D3D12_RENDER_PASS_ENDING_ACCESS {
    D3D12_RENDER_PASS_ENDING_ACCESS { Type: D3D12_RENDER_PASS_ENDING_ACCESS_TYPE_PRESERVE, ..Default::default() }
}

/// A resource of a shader's set and binding, by the shaders' ids: the
/// inverse of `mln_glsl::layout`'s `*_binding` functions.
enum Resource {
    Buffer(u32),
    /// A texture or its sampler, at its index in the table.
    Texture {
        id: u32,
        index: u32,
    },
}

fn resource_of(name: &str, textures: &[ShaderTexture], set: u32, binding: u32) -> Result<Resource, GpuError> {
    let texture = |index: u32| {
        textures
            .iter()
            .find(|texture| texture.index == index as usize)
            .map(|texture| Resource::Texture { id: texture.id as u32, index })
            .ok_or_else(|| GpuError::Invalid(format!("{name}: no texture at index {index} of set {set}")))
    };
    let start = LAYER_SSBO_START_ID as u32;
    let storage = LAYER_STORAGE_COUNT as u32;
    match DescriptorSetType::ALL.get(set as usize) {
        Some(DescriptorSetType::Global) => Ok(Resource::Buffer(binding)),
        Some(DescriptorSetType::Layer) if binding < storage => Ok(Resource::Buffer(binding + start)),
        Some(DescriptorSetType::Layer) => Ok(Resource::Buffer(binding - storage + DRAWABLE_RESERVED_UBO_COUNT as u32)),
        Some(DescriptorSetType::DrawableUniform) => Ok(Resource::Buffer(binding + start)),
        Some(DescriptorSetType::DrawableImage) if binding < IMAGE_SAMPLER_START as u32 => texture(binding),
        Some(DescriptorSetType::DrawableImage) => texture(binding - IMAGE_SAMPLER_START as u32),
        None => Err(GpuError::Invalid(format!("{name}: no descriptor set {set}"))),
    }
}

impl D3d12Gpu {
    /// A backend on `device`. Fails without DXC, or if the device can't
    /// make the root signature, the fence, the descriptor heaps or the
    /// allocator.
    pub fn new(device: &Device) -> Result<D3d12Gpu, GpuError> {
        let d3d = device.device();
        let compiler = ShaderCompiler::new()?;
        // SAFETY: a plain call.
        let fence: ID3D12Fence =
            unsafe { d3d.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.map_err(invalid("CreateFence"))?;
        let caps = Caps {
            buffer_offset_alignment: BUFFER_ALIGNMENT as u32,
            max_texture_size: MAX_TEXTURE_SIZE,
            half_float_render_targets: true,
            clip_depth: ClipDepthRange::ZeroToOne,
            render_target_depth: None,
            clear_stencil_with_pass: true,
            line_expressions: false,
            cpu_rasterizer: device.is_software(),
            max_line_width: 1.0,
        };
        Ok(D3d12Gpu {
            compiler,
            caps,
            fence,
            serial: 1,
            window: (0, 0),
            buffers: SlotMap::with_key(),
            stale: Vec::new(),
            retired: Vec::new(),
            upload: UploadRing::default(),
            copied: HashSet::default(),
            copied_resources: Vec::new(),
            textures: SlotMap::with_key(),
            srv_heap: CpuDescriptors::new(d3d, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            rtv_heap: CpuDescriptors::new(d3d, D3D12_DESCRIPTOR_HEAP_TYPE_RTV),
            dsv_heap: CpuDescriptors::new(d3d, D3D12_DESCRIPTOR_HEAP_TYPE_DSV),
            tables: TableHeap::new(d3d)?,
            sampler_heap: SamplerHeap::new(d3d)?,
            samplers: SlotMap::with_key(),
            sampler_indices: HashMap::default(),
            shaders: SlotMap::with_key(),
            layout_tags: LayoutTags::default(),
            pipelines: SlotMap::with_key(),
            bindings: SlotMap::with_key(),
            geometries: SlotMap::with_key(),
            readbacks: SlotMap::with_key(),
            spare_readback: None,
            copies: None,
            frame: None,
            pass: None,
            free_recordings: Vec::new(),
            in_flight: VecDeque::new(),
            garbage: Vec::new(),
            waits: Vec::new(),
            signals: Vec::new(),
            texture_waits: HashMap::default(),
            memory: Memory::new(d3d)?,
            device: device.clone(),
        })
    }

    /// The device the backend renders on.
    #[must_use]
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Takes `resource`, a texture made elsewhere that is in `state`, as a
    /// texture of this backend, which leaves it in `state` after each pass
    /// and copy. Destroying it drops this backend's reference.
    pub fn import_texture(
        &mut self,
        resource: &ID3D12Resource,
        state: D3D12_RESOURCE_STATES,
    ) -> Result<TextureId, GpuError> {
        // SAFETY: a plain query.
        let desc = unsafe { resource.GetDesc() };
        if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D || desc.SampleDesc.Count != 1 {
            return Err(GpuError::Unsupported("a texture that isn't 2D and single-sampled".to_owned()));
        }
        let format = conv::from_texture_format(desc.Format);
        let usage = TextureUsage {
            sampled: (desc.Flags & D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE).0 == 0,
            render_target: (desc.Flags
                & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL))
                .0
                != 0,
            copy_src: true,
        };
        let size = Size::new(desc.Width as u32, desc.Height);
        let memory = TextureMemory::imported(resource.clone(), self.memory.freed());
        self.insert_texture(memory, desc.Format, TextureInfo { format, size, usage }, state)
    }

    /// Each window of the uniform or storage buffer `buffer`.
    #[cfg(test)]
    pub(crate) fn windows(&self, buffer: BufferId) -> Vec<Vec<u8>> {
        let Some(Buffer { memory, windows: Some(windows), size, .. }) = self.buffers.get(buffer) else {
            return Vec::new();
        };
        let bytes = memory.read();
        (0..WINDOWS).map(|window| bytes[window * windows.stride as usize..][..*size as usize].to_vec()).collect()
    }

    /// How many pages the slabs of small buffers have.
    #[cfg(test)]
    pub(crate) fn slab_pages(&self) -> usize {
        self.memory.slab_pages()
    }

    /// The D3D12 resource of `texture`, and the state it is in between
    /// passes and copies.
    #[must_use]
    pub fn native_texture(&self, texture: TextureId) -> Option<(&ID3D12Resource, D3D12_RESOURCE_STATES)> {
        self.textures.get(texture).map(|texture| (&texture.memory.resource, texture.rest))
    }

    /// The GPU address of `buffer`'s memory, for a host that binds the
    /// buffer itself, such as an overlay's vertex and index buffers.
    #[must_use]
    pub fn buffer_address(&self, buffer: BufferId) -> Option<u64> {
        self.buffers.get(buffer).map(|buffer| buffer.memory.address())
    }

    /// Keeps `object` until the work recorded so far has finished, as a
    /// destroyed resource is kept: an overlay's own pipeline state, root
    /// signature or descriptor heap that its last frames still use.
    pub fn destroy_later(&mut self, object: Box<dyn Any>) {
        self.garbage.push((self.serial, Garbage::Held(object)));
    }

    /// Has the queue wait for `fence` to reach `value` before the submit of
    /// the next pass that draws into `texture`: the host's reads of it. It
    /// replaces what the texture waited for.
    pub fn wait_before_using(&mut self, texture: TextureId, fence: ID3D12Fence, value: u64) {
        self.texture_waits.insert(texture, (fence, value));
    }

    /// Has the next submit wait for what `texture` waits for, though no pass
    /// drew into it.
    pub fn take_waits_into_next_submit(&mut self, texture: TextureId) {
        self.waits.extend(self.texture_waits.remove(&texture));
    }

    /// Drops what `texture` waits for, which no submit takes.
    pub fn forget_waits(&mut self, texture: TextureId) {
        self.texture_waits.remove(&texture);
    }

    /// The fence that the backend's submits signal, and the value of the
    /// last one's: once the fence reaches it, the work submitted so far has
    /// finished.
    #[must_use]
    pub fn last_submit(&self) -> (ID3D12Fence, u64) {
        (self.fence.clone(), self.serial - 1)
    }

    /// Has the queue signal `fence` with `value` once the next flush's work
    /// has finished, for a host that waits for a frame.
    pub fn signal_after_next_submit(&mut self, fence: ID3D12Fence, value: u64) {
        self.signals.push((fence, value));
    }

    fn completed(&self) -> u64 {
        // SAFETY: a plain query.
        unsafe { self.fence.GetCompletedValue() }
    }

    /// The error of a failed D3D12 call, with why the device was removed
    /// if it was.
    fn failed(&self, what: &str, error: &windows::core::Error) -> GpuError {
        match self.device.removed_reason() {
            Some(reason) => GpuError::Invalid(format!("{what}: {reason}")),
            None => GpuError::Invalid(format!("{what}: {error}")),
        }
    }

    fn insert_texture(
        &mut self,
        memory: TextureMemory,
        format: DXGI_FORMAT,
        info: TextureInfo,
        rest: D3D12_RESOURCE_STATES,
    ) -> Result<TextureId, GpuError> {
        let depth = info.format.is_some_and(conv::is_depth);
        let device = self.device.device();
        let mut texture = Texture { memory: Rc::new(memory), info, format, rest, srv: None, rtv: None, dsv: None };
        let resource = &texture.memory.resource;
        // SAFETY: the views are of the texture, into descriptors no work
        // reads.
        unsafe {
            if info.usage.sampled && !depth {
                let slot = self.srv_heap.allocate(device)?;
                device.CreateShaderResourceView(resource, None, slot.handle);
                texture.srv = Some(slot);
            }
            if info.usage.render_target && depth {
                let slot = self.dsv_heap.allocate(device)?;
                device.CreateDepthStencilView(resource, None, slot.handle);
                texture.dsv = Some(slot);
            } else if info.usage.render_target {
                let slot = self.rtv_heap.allocate(device)?;
                device.CreateRenderTargetView(resource, None, slot.handle);
                texture.rtv = Some(slot);
            }
        }
        Ok(self.textures.insert(texture))
    }

    /// Clears the new render target `texture` to zeros in the copy list, as
    /// new memory holds: D3D12 can't use a placed render target until a
    /// discard, clear or copy, and the renderer reads a depth it didn't
    /// clear in `fill-extrusion-multiple/stencil-wrap`.
    fn clear_new_target(&mut self, texture: TextureId, depth: bool) -> Result<(), GpuError> {
        let list = self.copy_list()?;
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        let (resource, rest) = (&entry.memory.resource, entry.rest);
        let target = if depth { D3D12_RESOURCE_STATE_DEPTH_WRITE } else { D3D12_RESOURCE_STATE_RENDER_TARGET };
        // SAFETY: no work uses the new texture, its views are its own, and
        // the barriers return it to its rest.
        unsafe {
            if rest != target {
                list.ResourceBarrier(&[transition(resource, rest, target)]);
            }
            match (entry.rtv, entry.dsv) {
                (Some(rtv), _) => list.ClearRenderTargetView(rtv.handle, &[0.0; 4], None),
                (_, Some(dsv)) => {
                    let mut flags = D3D12_CLEAR_FLAG_DEPTH;
                    if entry.info.format.is_some_and(TextureFormat::has_stencil) {
                        flags |= D3D12_CLEAR_FLAG_STENCIL;
                    }
                    list.ClearDepthStencilView(dsv.handle, flags, 0.0, 0, None);
                }
                (None, None) => {}
            }
            if rest != target {
                list.ResourceBarrier(&[transition(resource, target, rest)]);
            }
        }
        Ok(())
    }

    fn info(&self) -> (impl Fn(BufferId) -> Option<BufferInfo> + '_, impl Fn(TextureId) -> Option<TextureInfo> + '_) {
        let buffer =
            |id| self.buffers.get(id).map(|buffer: &Buffer| BufferInfo { usage: buffer.usage, size: buffer.size });
        let texture = |id| self.textures.get(id).map(|texture: &Texture| texture.info);
        (buffer, texture)
    }

    /// The window of uniform and storage buffers that the batch being
    /// recorded reads. Its first call in a batch waits for the batch that
    /// read the window before, then brings the window up to date.
    fn open_window(&mut self) -> Result<usize, GpuError> {
        let serial = self.serial;
        if self.window.0 == serial {
            return Ok(self.window.1);
        }
        let count = WINDOWS as u64;
        if serial > count {
            self.wait_for(serial - count)?;
        }
        let window = (serial % count) as usize;
        let all = (1 << WINDOWS) - 1;
        let buffers = &mut self.buffers;
        self.stale.retain(|&id| {
            let Some(Buffer { memory, windows: Some(windows), .. }) = buffers.get_mut(id) else { return false };
            if windows.current & (1 << window) == 0 {
                windows.copy(memory, window, 0, windows.contents.len());
                windows.current |= 1 << window;
            }
            windows.current != all
        });
        for (memory, windows) in &mut self.retired {
            if windows.current & (1 << window) == 0 {
                windows.copy(memory, window, 0, windows.contents.len());
                windows.current |= 1 << window;
            }
        }
        // Up to date, they wait for the work that reads them as destroyed
        // buffers do.
        let (done, retired): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.retired).into_iter().partition(|(_, windows)| windows.current == all);
        self.retired = retired;
        self.garbage.extend(done.into_iter().map(|(memory, _)| (serial, Garbage::Held(Box::new(memory)))));
        self.window = (serial, window);
        Ok(window)
    }

    /// A command list to record into: a finished one's again, or a new one.
    fn recording(&mut self) -> Result<Recording, GpuError> {
        let completed = self.completed();
        while let Some((_, recording)) = self.in_flight.pop_front_if(|(serial, _)| *serial <= completed) {
            self.free_recordings.push(recording);
        }
        let device = self.device.device();
        if let Some(recording) = self.free_recordings.pop() {
            // SAFETY: the work recorded in them has finished.
            unsafe {
                recording.allocator.Reset().map_err(invalid("ID3D12CommandAllocator::Reset"))?;
                recording
                    .list
                    .Reset(&recording.allocator, None)
                    .map_err(invalid("ID3D12GraphicsCommandList::Reset"))?;
            }
            return Ok(recording);
        }
        // SAFETY: plain calls.
        unsafe {
            let allocator: ID3D12CommandAllocator = device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .map_err(invalid("CreateCommandAllocator"))?;
            let list: ID3D12GraphicsCommandList = device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                .map_err(invalid("CreateCommandList"))?;
            Ok(Recording { allocator, list })
        }
    }

    /// Sets the descriptor heaps and the root signature on `list`, as each
    /// list begins and after a custom layer recorded.
    fn bind_root(&self, list: &ID3D12GraphicsCommandList) {
        // SAFETY: the heaps and the root signature live as long as the
        // backend, which keeps the list's work alive until it finishes.
        unsafe {
            list.SetDescriptorHeaps(&[Some(self.tables.heap.clone()), Some(self.sampler_heap.heap.clone())]);
            list.SetGraphicsRootSignature(self.device.root_signature());
            list.SetGraphicsRootDescriptorTable(
                SAMPLER_TABLE,
                self.sampler_heap.heap.GetGPUDescriptorHandleForHeapStart(),
            );
        }
    }

    /// The command list of the frame's commands.
    fn frame_list(&mut self) -> Result<ID3D12GraphicsCommandList, GpuError> {
        if let Some(frame) = &self.frame {
            return Ok(frame.list.clone());
        }
        let recording = self.recording()?;
        self.bind_root(&recording.list);
        let list = recording.list.clone();
        self.frame = Some(recording);
        Ok(list)
    }

    /// The command list of the copies since the last flush.
    fn copy_list(&mut self) -> Result<ID3D12GraphicsCommandList, GpuError> {
        if let Some(copies) = &self.copies {
            return Ok(copies.list.clone());
        }
        let recording = self.recording()?;
        let list = recording.list.clone();
        self.copies = Some(recording);
        Ok(list)
    }

    /// Submits the copies since the last flush and the frame's commands, and
    /// signals the fence. The copies wait for the work before them, which
    /// may read the buffers they write, and the frame for the copies: their
    /// buffers go from the common state to the copy state and back.
    /// Returns the batch's serial.
    fn submit(&mut self) -> Result<u64, GpuError> {
        let mut lists = Vec::new();
        if let Some(copies) = self.copies.take() {
            let barriers: Vec<_> = self
                .copied_resources
                .drain(..)
                .map(|resource| transition(&resource, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON))
                .collect();
            self.copied.clear();
            if !barriers.is_empty() {
                // SAFETY: the barriers return the buffers the copies wrote.
                unsafe { copies.list.ResourceBarrier(&barriers) };
            }
            lists.push(copies);
        }
        lists.extend(self.frame.take());
        let mut submitted = Vec::with_capacity(lists.len());
        for recording in &lists {
            // SAFETY: the list holds commands of resources the backend
            // keeps until the fence passes the batch.
            unsafe { recording.list.Close() }
                .map_err(|error| self.failed("ID3D12GraphicsCommandList::Close", &error))?;
            submitted.push(Some(recording.list.cast().map_err(invalid("ID3D12CommandList"))?));
        }
        let queue = self.device.queue();
        // SAFETY: as above, and the fences live until the calls return: the
        // queue holds what it waits on and signals.
        unsafe {
            for (fence, value) in std::mem::take(&mut self.waits) {
                queue.Wait(&fence, value).map_err(|error| self.failed("ID3D12CommandQueue::Wait", &error))?;
            }
            if !submitted.is_empty() {
                queue.ExecuteCommandLists(&submitted);
            }
            for (fence, value) in std::mem::take(&mut self.signals) {
                queue.Signal(&fence, value).map_err(|error| self.failed("ID3D12CommandQueue::Signal", &error))?;
            }
            queue
                .Signal(&self.fence, self.serial)
                .map_err(|error| self.failed("ID3D12CommandQueue::Signal", &error))?;
        }
        let serial = self.serial;
        self.in_flight.extend(lists.into_iter().map(|recording| (serial, recording)));
        self.serial += 1;
        Ok(serial)
    }

    /// Blocks until the batch `serial` has finished.
    fn wait_for(&self, serial: u64) -> Result<(), GpuError> {
        if self.completed() >= serial {
            return Ok(());
        }
        // SAFETY: without an event, the call returns once the fence has
        // reached the value.
        unsafe { self.fence.SetEventOnCompletion(serial, HANDLE::default()) }
            .map_err(|error| self.failed("waiting for the GPU", &error))
    }

    /// Frees what destroyed resources left behind, once their work has
    /// finished, and takes back the memory nothing uses any more.
    fn collect_garbage(&mut self) {
        let completed = self.completed();
        let (done, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.garbage).into_iter().partition(|(serial, _)| *serial <= completed);
        self.garbage = waiting;
        for (_, garbage) in done {
            match garbage {
                Garbage::Texture(texture) => {
                    let heaps = [
                        (texture.srv, &mut self.srv_heap),
                        (texture.rtv, &mut self.rtv_heap),
                        (texture.dsv, &mut self.dsv_heap),
                    ];
                    for (slot, heap) in heaps {
                        if let Some(slot) = slot {
                            heap.free(slot);
                        }
                    }
                }
                Garbage::Bindings(bindings) => {
                    if let Some((table, _)) = bindings.textures {
                        self.tables.free(table);
                    }
                }
                Garbage::Held(_) => {}
            }
        }
        self.memory.reclaim();
    }

    /// The address of upload memory that holds `indices`, which the sampler
    /// index buffers of bindings point at.
    fn sampler_index_address(&mut self, indices: [u32; TABLE_SIZE as usize]) -> Result<u64, GpuError> {
        if let Some(memory) = self.sampler_indices.get(&indices) {
            return Ok(memory.address());
        }
        let memory = self.memory.buffer(MemoryLocation::CpuToGpu, size_of_val(&indices) as u64)?;
        let bytes: Vec<u8> = indices.iter().flat_map(|index| index.to_ne_bytes()).collect();
        // SAFETY: no work reads the new memory.
        unsafe { memory.write(0, &bytes) };
        let address = memory.address();
        self.sampler_indices.insert(indices, memory);
        Ok(address)
    }

    fn no_pass(what: &str) -> GpuError {
        GpuError::Invalid(format!("{what} outside a pass"))
    }

    fn open_pass(what: &str) -> GpuError {
        GpuError::Invalid(format!("{what} while a pass is open"))
    }
}

impl Gpu for D3d12Gpu {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        let size = desc.size.max(4).next_multiple_of(4);
        let (memory, windows) = match desc.usage {
            BufferUsage::Uniform | BufferUsage::Storage => {
                let stride = size.next_multiple_of(BUFFER_ALIGNMENT);
                let memory = self.memory.buffer(MemoryLocation::CpuToGpu, stride * WINDOWS as u64)?;
                // Every window holds the contents, zeros, as reused memory
                // holds another buffer's bytes.
                // SAFETY: no work reads the new memory.
                unsafe { memory.write(0, &vec![0; (stride * WINDOWS as u64) as usize]) };
                let windows = Windows { contents: vec![0; size as usize], stride, current: (1 << WINDOWS) - 1 };
                (memory, Some(windows))
            }
            BufferUsage::Vertex | BufferUsage::Index => (self.memory.buffer(MemoryLocation::GpuOnly, size)?, None),
        };
        Ok(self.buffers.insert(Buffer { memory: Rc::new(memory), usage: desc.usage, size, windows }))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        validation::check_write(offset, data.len(), entry.size)?;
        let length = data.len() as u64;
        if data.is_empty() {
            return Ok(());
        }
        let (start, end) = (offset as usize, (offset + length) as usize);
        if let Some(windows) = &entry.windows {
            if windows.contents[start..end] == *data {
                return Ok(());
            }
            let window = self.open_window()?;
            let all = (1 << WINDOWS) - 1;
            let Some(Buffer { memory, windows: Some(windows), .. }) = self.buffers.get_mut(buffer) else {
                return Err(GpuError::UnknownId("buffer"));
            };
            windows.contents[start..end].copy_from_slice(data);
            // The window holds the rest of the contents already, or gets
            // all of them.
            let (start, end) =
                if windows.current & (1 << window) != 0 { (start, end) } else { (0, windows.contents.len()) };
            windows.copy(memory, window, start, end);
            if windows.current == all {
                self.stale.push(buffer);
            }
            windows.current = 1 << window;
            return Ok(());
        }
        let (resource, at) = (entry.memory.resource.clone(), entry.memory.offset + offset);
        let completed = self.completed();
        let upload = self.upload.write(&mut self.memory, self.serial, completed, length, 4, |bytes| {
            bytes.copy_from_slice(data);
        })?;
        let list = self.copy_list()?;
        let first = self.copied.insert(resource.as_raw() as usize);
        // SAFETY: both buffers live until the copy finishes: the upload page
        // until its batch does, and the buffer's memory as destroyed
        // buffers' does. The barrier waits for the work before, which reads
        // the buffer in the common state it decayed to.
        unsafe {
            if first {
                list.ResourceBarrier(&[transition(
                    &resource,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )]);
            }
            list.CopyBufferRegion(&resource, at, &upload.resource, upload.offset, length);
        }
        if first {
            self.copied_resources.push(resource);
        }
        Ok(())
    }

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
        let Size { width, height } = desc.size;
        if width == 0 || height == 0 || width > MAX_TEXTURE_SIZE || height > MAX_TEXTURE_SIZE {
            return Err(GpuError::Invalid(format!("a texture of {width}x{height}")));
        }
        let format = conv::texture_format(desc.format);
        let depth = conv::is_depth(desc.format);
        let mut flags = D3D12_RESOURCE_FLAG_NONE;
        if desc.usage.render_target {
            flags |=
                if depth { D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL } else { D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET };
        }
        if depth && !desc.usage.sampled {
            flags |= D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE;
        }
        let resource_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: u64::from(width),
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: flags,
        };
        let info = TextureInfo { format: Some(desc.format), size: desc.size, usage: desc.usage };
        let rest = rest_state(info.format, desc.usage);
        let bytes = u64::from(width) * u64::from(height) * u64::from(desc.format.bytes_per_texel().unwrap_or(8));
        let memory = self.memory.texture(self.device.device(), &resource_desc, bytes, rest)?;
        let id = self.insert_texture(memory, format, info, rest)?;
        if desc.usage.render_target
            && let Err(error) = self.clear_new_target(id, depth)
        {
            self.destroy(ResourceId::Texture(id));
            return Err(error);
        }
        Ok(id)
    }

    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        if !entry.info.usage.sampled {
            return Err(GpuError::Invalid("a write to a texture that isn't sampled".to_owned()));
        }
        let format = entry.info.format.ok_or_else(|| GpuError::Invalid("a texture of no known format".to_owned()))?;
        let bytes_per_row = region.write_bytes_per_row(entry.info.size, format, data)?;
        if data.is_empty() {
            return Ok(());
        }
        let row_pitch = bytes_per_row.next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT as usize);
        let rows = region.size.height as usize;
        let (memory, dxgi_format, rest) = (Rc::clone(&entry.memory), entry.format, entry.rest);
        let completed = self.completed();
        let upload = self.upload.write(
            &mut self.memory,
            self.serial,
            completed,
            (row_pitch * rows) as u64,
            u64::from(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT),
            |bytes| {
                for (row, source) in bytes.chunks_mut(row_pitch).zip(data.chunks(bytes_per_row)) {
                    row[..bytes_per_row].copy_from_slice(source);
                }
            },
        )?;
        let list = self.copy_list()?;
        let resource = &memory.resource;
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: copies of the pointers without references, which the
            // locations' ManuallyDrop never releases.
            pResource: unsafe { std::mem::transmute_copy(resource) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above.
            pResource: unsafe { std::mem::transmute_copy(&upload.resource) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: upload.offset,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: dxgi_format,
                        Width: region.size.width,
                        Height: region.size.height,
                        Depth: 1,
                        RowPitch: row_pitch as u32,
                    },
                },
            },
        };
        // SAFETY: the texture and the upload page live until the copy
        // finishes, and the barriers return the texture to its rest.
        unsafe {
            list.ResourceBarrier(&[transition(resource, rest, D3D12_RESOURCE_STATE_COPY_DEST)]);
            list.CopyTextureRegion(&raw const dst, region.x, region.y, 0, &raw const src, None);
            list.ResourceBarrier(&[transition(resource, D3D12_RESOURCE_STATE_COPY_DEST, rest)]);
        }
        Ok(())
    }

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError> {
        let slot = self.sampler_heap.slot(self.device.device(), state)?;
        Ok(self.samplers.insert(slot))
    }

    fn create_shader(&mut self, key: &ShaderKey) -> Result<ShaderId, GpuError> {
        let program = self.compiler.compile(key)?;
        let source_textures = mln_glsl::shader_source(key.program()).map_or(&[][..], |source| source.textures);
        let mut reads: Vec<ShaderBinding> = Vec::new();
        let mut buffers = Vec::new();
        let mut textures = Vec::new();
        for resource in &program.resources {
            match resource_of(program.name, source_textures, resource.set, resource.binding)? {
                Resource::Buffer(id) => {
                    let (parameter, view) = root_signature::buffer_parameter(resource.set, resource.binding)
                        .ok_or_else(|| {
                            GpuError::Invalid(format!(
                                "{}: no buffer at set {} binding {}",
                                program.name, resource.set, resource.binding
                            ))
                        })?;
                    if let Some(ShaderBinding::Buffer { min_size, .. }) = reads
                        .iter_mut()
                        .find(|read| matches!(read, ShaderBinding::Buffer { id: read_id, .. } if *read_id == id))
                    {
                        *min_size = (*min_size).max(resource.size);
                    } else {
                        reads.push(ShaderBinding::Buffer { id, min_size: resource.size });
                        buffers.push((id, parameter, view));
                    }
                }
                Resource::Texture { id, index } => {
                    if !textures.contains(&(id, index)) {
                        textures.push((id, index));
                        reads.push(ShaderBinding::Texture { id });
                    }
                }
            }
        }
        Ok(self.shaders.insert(Shader { key: key.clone(), program, reads, buffers, textures }))
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<PipelineId, GpuError> {
        let shader = self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?;
        let program = &shader.program;
        let (topology_type, topology) = conv::topology(desc.state.primitive)
            .ok_or_else(|| GpuError::Unsupported(format!("{:?} primitives", desc.state.primitive)))?;
        let key = (shader.key.clone(), desc.vertex_buffers.to_vec(), desc.state, desc.targets);
        let state = self.device.pipeline(key, || create_pipeline_state(&self.device, program, desc, topology_type))?;
        let layout = self.layout_tags.tag(desc.shader, desc.vertex_buffers);
        let tags = PipelineTags { shader: desc.shader, layout, targets: desc.targets };
        Ok(self.pipelines.insert(Pipeline { state, topology, tags }))
    }

    fn create_bindings(&mut self, desc: &BindingsDesc<'_>) -> Result<BindingsId, GpuError> {
        let shader = self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?;
        let elements = {
            let (buffer, texture) = self.info();
            let sampler = |id| self.samplers.contains_key(id);
            let lookup = Lookup { buffer: &buffer, texture: &texture, sampler: &sampler };
            validation::check_bindings(desc, &shader.reads, self.caps.buffer_offset_alignment, &lookup)?
        };
        let missing = |what: &str, id: u32| {
            GpuError::Invalid(format!("{} reads {what} {id}, which no entry binds", shader.program.name))
        };
        let (mut kept_buffers, mut kept_textures) = (Vec::new(), Vec::new());
        let mut buffers = Vec::new();
        for &(id, parameter, view) in &shader.buffers {
            let (buffer, offset) = desc
                .entries
                .iter()
                .find_map(|entry| match *entry {
                    BindGroupEntry::Buffer { id: entry_id, buffer, offset, .. } if entry_id == id => {
                        Some((buffer, offset))
                    }
                    _ => None,
                })
                .ok_or_else(|| missing("buffer", id))?;
            let buffer = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
            let stride = buffer.windows.as_ref().map_or(0, |windows| windows.stride);
            buffers.push(BoundBuffer { parameter, view, address: buffer.memory.address() + offset, stride });
            kept_buffers.push(Rc::clone(&buffer.memory));
        }
        let textures = if shader.textures.is_empty() {
            None
        } else {
            let mut views = [None; TABLE_SIZE as usize];
            let mut indices = [0; TABLE_SIZE as usize];
            for &(id, index) in &shader.textures {
                let (texture, sampler) = desc
                    .entries
                    .iter()
                    .find_map(|entry| match *entry {
                        BindGroupEntry::Texture { id: entry_id, texture, sampler } if entry_id == id => {
                            Some((texture, sampler))
                        }
                        _ => None,
                    })
                    .ok_or_else(|| missing("texture", id))?;
                let texture = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
                let srv =
                    texture.srv.ok_or_else(|| GpuError::Invalid("a texture that shaders can't sample".to_owned()))?;
                views[index as usize] = Some(srv.handle);
                indices[index as usize] = *self.samplers.get(sampler).ok_or(GpuError::UnknownId("sampler"))?;
                kept_textures.push(Rc::clone(&texture.memory));
            }
            let table = self.tables.allocate(self.device.device(), &views)?;
            match self.sampler_index_address(indices) {
                Ok(address) => Some((table, address)),
                Err(error) => {
                    self.tables.free(table);
                    return Err(error);
                }
            }
        };
        let tags = BindingsTags { shader: desc.shader, elements };
        Ok(self.bindings.insert(Bindings { buffers, textures, keep: (kept_buffers, kept_textures), tags }))
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
        let mut keep = Vec::new();
        let mut vertex_views = Vec::new();
        for (&(id, offset), layout) in desc.vertex_buffers.iter().zip(desc.layouts) {
            let buffer = self.buffers.get(id).ok_or(GpuError::UnknownId("buffer"))?;
            vertex_views.push(D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: buffer.memory.address() + offset,
                SizeInBytes: (buffer.size - offset) as u32,
                StrideInBytes: layout.stride,
            });
            keep.push(Rc::clone(&buffer.memory));
        }
        let index = self.buffers.get(desc.index_buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let index_view = D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: index.memory.address(),
            SizeInBytes: index.size as u32,
            Format: DXGI_FORMAT_R16_UINT,
        };
        keep.push(Rc::clone(&index.memory));
        let tags = GeometryTags { layout: self.layout_tags.tag(desc.shader, desc.layouts), ranges };
        Ok(self.geometries.insert(Geometry { vertex_views, index_view, keep, tags }))
    }

    fn destroy(&mut self, resource: ResourceId) {
        fn held<T: Any>(value: Option<T>) -> Option<Garbage> {
            value.map(|value| Garbage::Held(Box::new(value)))
        }
        let garbage = match resource {
            ResourceId::Buffer(id) => match self.buffers.remove(id) {
                // Bindings may still read the buffer in each window.
                Some(Buffer { memory, windows: Some(windows), .. }) if windows.current != (1 << WINDOWS) - 1 => {
                    self.retired.push((memory, windows));
                    None
                }
                buffer => held(buffer.map(|buffer| buffer.memory)),
            },
            ResourceId::Texture(id) => {
                self.texture_waits.remove(&id);
                self.textures.remove(id).map(Garbage::Texture)
            }
            ResourceId::Sampler(id) => {
                self.samplers.remove(id);
                None
            }
            ResourceId::Shader(id) => {
                self.shaders.remove(id);
                None
            }
            ResourceId::Pipeline(id) => held(self.pipelines.remove(id).map(|pipeline| pipeline.state)),
            ResourceId::Bindings(id) => self.bindings.remove(id).map(Garbage::Bindings),
            ResourceId::Geometry(id) => held(self.geometries.remove(id).map(|geometry| geometry.keep)),
            ResourceId::Readback(id) => held(self.readbacks.remove(id).map(|readback| readback.memory)),
        };
        if let Some(garbage) = garbage {
            self.garbage.push((self.serial, garbage));
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
        let window = self.open_window()?;
        let list = self.frame_list()?;
        let list = list.cast::<ID3D12GraphicsCommandList4>().map_err(invalid("ID3D12GraphicsCommandList4"))?;
        // The pass's submit waits for the host's reads of its attachments.
        for texture in
            desc.color.map(|color| color.texture).into_iter().chain(desc.depth_stencil.map(|depth| depth.texture))
        {
            self.take_waits_into_next_submit(texture);
        }
        let mut barriers = Vec::new();
        let mut color = None;
        if let Some(attachment) = desc.color {
            let texture = self.textures.get(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
            let rtv = texture
                .rtv
                .ok_or_else(|| GpuError::Invalid("a color attachment that isn't a render target".to_owned()))?;
            if texture.rest != D3D12_RESOURCE_STATE_RENDER_TARGET {
                barriers.push(transition(&texture.memory.resource, texture.rest, D3D12_RESOURCE_STATE_RENDER_TARGET));
            }
            let load = match attachment.load {
                LoadOp::Load => LoadOp::Load,
                LoadOp::Clear(value) => LoadOp::Clear(D3D12_CLEAR_VALUE {
                    Format: texture.format,
                    Anonymous: D3D12_CLEAR_VALUE_0 { Color: self::color(value) },
                }),
            };
            let render_target = D3D12_RENDER_PASS_RENDER_TARGET_DESC {
                cpuDescriptor: rtv.handle,
                BeginningAccess: beginning(load),
                EndingAccess: preserve(),
            };
            let pass_attachment = Attachment {
                memory: Rc::clone(&texture.memory),
                rest: texture.rest,
                view: rtv.handle,
                format: texture.format,
            };
            color = Some((pass_attachment, render_target));
        }
        let mut depth_stencil = None;
        if let Some(attachment) = desc.depth_stencil {
            let texture = self.textures.get(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
            let dsv = texture
                .dsv
                .ok_or_else(|| GpuError::Invalid("a depth attachment that isn't a render target".to_owned()))?;
            if texture.rest != D3D12_RESOURCE_STATE_DEPTH_WRITE {
                barriers.push(transition(&texture.memory.resource, texture.rest, D3D12_RESOURCE_STATE_DEPTH_WRITE));
            }
            let clear = |depth, stencil| D3D12_CLEAR_VALUE {
                Format: texture.format,
                Anonymous: D3D12_CLEAR_VALUE_0 {
                    DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: depth, Stencil: stencil },
                },
            };
            let depth_load = match attachment.depth {
                LoadOp::Load => LoadOp::Load,
                LoadOp::Clear(depth) => LoadOp::Clear(clear(depth, 0)),
            };
            let has_stencil = texture.info.format.is_some_and(TextureFormat::has_stencil);
            let (stencil_beginning, stencil_ending) = if has_stencil {
                let stencil_load = match attachment.stencil {
                    LoadOp::Load => LoadOp::Load,
                    LoadOp::Clear(stencil) => LoadOp::Clear(clear(0.0, stencil as u8)),
                };
                (beginning(stencil_load), preserve())
            } else {
                (
                    D3D12_RENDER_PASS_BEGINNING_ACCESS {
                        Type: D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_NO_ACCESS,
                        ..Default::default()
                    },
                    D3D12_RENDER_PASS_ENDING_ACCESS {
                        Type: D3D12_RENDER_PASS_ENDING_ACCESS_TYPE_NO_ACCESS,
                        ..Default::default()
                    },
                )
            };
            let pass_depth_stencil = D3D12_RENDER_PASS_DEPTH_STENCIL_DESC {
                cpuDescriptor: dsv.handle,
                DepthBeginningAccess: beginning(depth_load),
                StencilBeginningAccess: stencil_beginning,
                DepthEndingAccess: preserve(),
                StencilEndingAccess: stencil_ending,
            };
            let pass_attachment = Attachment {
                memory: Rc::clone(&texture.memory),
                rest: texture.rest,
                view: dsv.handle,
                format: texture.format,
            };
            depth_stencil = Some((pass_attachment, pass_depth_stencil));
        }
        // SAFETY: the attachments live as long as the pass's work, and the
        // barriers move them from their rest into the states the pass draws
        // in.
        unsafe {
            if !barriers.is_empty() {
                list.ResourceBarrier(&barriers);
            }
            let render_targets = color.as_ref().map(|(_, desc)| std::slice::from_ref(desc));
            let depth = depth_stencil.as_ref().map(|(_, desc)| std::ptr::from_ref(desc));
            list.BeginRenderPass(render_targets, depth, D3D12_RENDER_PASS_FLAG_NONE);
        }
        self.pass = Some(OpenPass {
            list,
            target,
            window,
            color: color.map(|(attachment, _)| attachment),
            depth_stencil: depth_stencil.map(|(attachment, _)| attachment),
            pipeline: None,
            bindings: None,
            global_index: None,
            geometry: None,
            vertex_buffers: Vec::new(),
            index_buffer: None,
            dynamic: None,
        });
        Ok(())
    }

    fn draw(&mut self, draw: &Draw) -> Result<(), GpuError> {
        let open = self.pass.as_mut().ok_or_else(|| Self::no_pass("a draw"))?;
        let pipeline = self.pipelines.get(draw.pipeline).ok_or(GpuError::UnknownId("pipeline"))?;
        let bindings = self.bindings.get(draw.bindings).ok_or(GpuError::UnknownId("bindings"))?;
        let geometry = self.geometries.get(draw.geometry).ok_or(GpuError::UnknownId("geometry"))?;
        validation::check_draw(draw, open.target.targets, &pipeline.tags, &bindings.tags, &geometry.tags)?;
        let list = &open.list;
        // SAFETY: what the draw binds lives until its work finishes: the
        // pipeline, bindings and geometry as destroyed ones do, and they
        // keep their memory.
        unsafe {
            if open.pipeline != Some(draw.pipeline) {
                list.SetPipelineState(&pipeline.state);
                list.IASetPrimitiveTopology(pipeline.topology);
                open.pipeline = Some(draw.pipeline);
            }
            if open.bindings != Some(draw.bindings) {
                for buffer in &bindings.buffers {
                    let address = buffer.address + buffer.stride * open.window as u64;
                    match buffer.view {
                        View::Constant => list.SetGraphicsRootConstantBufferView(buffer.parameter, address),
                        View::Shader => list.SetGraphicsRootShaderResourceView(buffer.parameter, address),
                    }
                }
                if let Some((table, indices)) = bindings.textures {
                    list.SetGraphicsRootDescriptorTable(TEXTURE_TABLE, self.tables.gpu(table));
                    list.SetGraphicsRootShaderResourceView(SAMPLER_INDICES, indices);
                }
                open.bindings = Some(draw.bindings);
            }
            if open.global_index != Some(draw.global_index) {
                list.SetGraphicsRoot32BitConstant(ROOT_CONSTANTS, draw.global_index, 0);
                open.global_index = Some(draw.global_index);
            }
            if open.geometry != Some(draw.geometry) {
                for (slot, view) in geometry.vertex_views.iter().enumerate() {
                    if open.vertex_buffers.get(slot) != Some(view) {
                        list.IASetVertexBuffers(slot as u32, Some(std::slice::from_ref(view)));
                        match open.vertex_buffers.get_mut(slot) {
                            Some(bound) => *bound = *view,
                            None => open.vertex_buffers.push(*view),
                        }
                    }
                }
                if open.index_buffer != Some(geometry.index_view) {
                    list.IASetIndexBuffer(Some(&raw const geometry.index_view));
                    open.index_buffer = Some(geometry.index_view);
                }
                open.geometry = Some(draw.geometry);
            }
            let dynamic = draw.dynamic;
            let last = open.dynamic.replace(dynamic);
            let changed =
                |same: fn(&DynamicState, &DynamicState) -> bool| last.is_none_or(|last| !same(&last, &dynamic));
            if changed(|a, b| a.depth_range == b.depth_range) {
                let size = open.target.size;
                let (min, max) = dynamic.depth_range;
                list.RSSetViewports(&[D3D12_VIEWPORT {
                    TopLeftX: 0.0,
                    TopLeftY: 0.0,
                    Width: size.width as f32,
                    Height: size.height as f32,
                    MinDepth: min,
                    MaxDepth: max,
                }]);
            }
            if changed(|a, b| a.scissor == b.scissor) {
                let Scissor { x, y, width, height } = dynamic.scissor;
                list.RSSetScissorRects(&[RECT {
                    left: x as i32,
                    top: y as i32,
                    right: (x + width) as i32,
                    bottom: (y + height) as i32,
                }]);
            }
            if changed(|a, b| a.stencil_reference == b.stencil_reference) {
                list.OMSetStencilRef(dynamic.stencil_reference);
            }
            if changed(|a, b| a.blend_constant == b.blend_constant) {
                list.OMSetBlendFactor(Some(&color(dynamic.blend_constant)));
            }
            // D3D12 draws lines 1 pixel wide.
            list.DrawIndexedInstanced(
                draw.index_count,
                draw.instance_count,
                draw.first_index,
                draw.base_vertex,
                draw.first_instance,
            );
        }
        Ok(())
    }

    /// Lends the open render pass's command list, or the frame's. A pass
    /// that a custom layer draws into has a color attachment. The backend
    /// sets its heaps and root signature again after the layer.
    fn lend(&mut self, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError> {
        let device = self.device.device().clone();
        if let Some(mut open) = self.pass.take() {
            let Some((render_target, color_format)) = open.color.as_ref().map(|color| (color.view, color.format))
            else {
                self.pass = Some(open);
                return Err(GpuError::Unsupported("a custom layer's pass without a color attachment".to_owned()));
            };
            let depth_stencil = open.depth_stencil.as_ref().map(|depth| (depth.view, depth.format));
            let list: ID3D12GraphicsCommandList = open.list.cast().map_err(invalid("ID3D12GraphicsCommandList"))?;
            let mut lent = CustomLayerPass {
                device,
                command_list: list.clone(),
                render_target,
                color_format,
                depth_stencil: depth_stencil.map(|(view, _)| view),
                depth_stencil_format: depth_stencil.map_or(DXGI_FORMAT_UNKNOWN, |(_, format)| format),
            };
            f(&mut lent);
            self.bind_root(&list);
            open.forget_state();
            self.pass = Some(open);
            return Ok(());
        }
        let list = self.frame_list()?;
        let mut lent = CustomLayerEncoder { device, command_list: list.clone() };
        f(&mut lent);
        self.bind_root(&list);
        Ok(())
    }

    fn end_pass(&mut self) -> Result<(), GpuError> {
        let open = self.pass.take().ok_or_else(|| Self::no_pass("the end of a pass"))?;
        let attachments = [
            open.color.as_ref().map(|color| (color, D3D12_RESOURCE_STATE_RENDER_TARGET)),
            open.depth_stencil.as_ref().map(|depth| (depth, D3D12_RESOURCE_STATE_DEPTH_WRITE)),
        ];
        let barriers: Vec<_> = attachments
            .into_iter()
            .flatten()
            .filter(|(attachment, state)| attachment.rest != *state)
            .map(|(attachment, state)| transition(&attachment.memory.resource, state, attachment.rest))
            .collect();
        // SAFETY: the pass is open on the list, and the barriers return the
        // attachments to their rest.
        unsafe {
            open.list.EndRenderPass();
            if !barriers.is_empty() {
                open.list.ResourceBarrier(&barriers);
            }
        }
        Ok(())
    }

    fn flush(&mut self, wait: bool) -> Result<(), GpuError> {
        if self.pass.is_some() {
            return Err(Self::open_pass("a flush"));
        }
        let serial = self.submit()?;
        if wait {
            self.wait_for(serial)?;
        }
        self.collect_garbage();
        Ok(())
    }

    /// Copies into readback memory, with D3D12's row pitch: the last
    /// readback's where it's as big.
    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError> {
        if self.pass.is_some() {
            return Err(Self::open_pass("a readback"));
        }
        let entry = self.textures.get(texture);
        let readback = validation::check_readback(entry.map(|texture| texture.info), region)?;
        let entry = entry.ok_or(GpuError::UnknownId("texture"))?;
        let (memory, format, rest) = (Rc::clone(&entry.memory), entry.format, entry.rest);
        let row_pitch = readback.bytes_per_row.next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT as usize);
        let rows = region.size.height as usize;
        let size = (row_pitch * rows) as u64;
        let buffer = match self.spare_readback.take() {
            Some(spare) if spare.size >= size && spare.size <= size.next_power_of_two().max(512) => spare,
            // A copy from a texture writes from an offset of 512 bytes.
            _ => self
                .memory
                .buffer(MemoryLocation::GpuToCpu, size.max(u64::from(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT)))?,
        };
        debug_assert!(buffer.offset.is_multiple_of(u64::from(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT)));
        let list = self.frame_list()?;
        let resource = &memory.resource;
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: copies of the pointers without references, which the
            // locations' ManuallyDrop never releases.
            pResource: unsafe { std::mem::transmute_copy(&buffer.resource) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: buffer.offset,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: format,
                        Width: region.size.width,
                        Height: region.size.height,
                        Depth: 1,
                        RowPitch: row_pitch as u32,
                    },
                },
            },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above.
            pResource: unsafe { std::mem::transmute_copy(resource) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let area = D3D12_BOX {
            left: region.x,
            top: region.y,
            front: 0,
            right: region.x + region.size.width,
            bottom: region.y + region.size.height,
            back: 1,
        };
        // SAFETY: the texture and the buffer live until the copy finishes,
        // and the barriers return the texture to its rest.
        unsafe {
            list.ResourceBarrier(&[transition(resource, rest, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            list.CopyTextureRegion(&raw const dst, 0, 0, 0, &raw const src, Some(&raw const area));
            list.ResourceBarrier(&[transition(resource, D3D12_RESOURCE_STATE_COPY_SOURCE, rest)]);
        }
        let serial = self.submit()?;
        Ok(self.readbacks.insert(Readback {
            memory: buffer,
            row_pitch,
            bytes_per_row: readback.bytes_per_row,
            rows,
            serial,
        }))
    }

    fn take_readback(&mut self, readback: ReadbackId, wait: bool) -> Result<Option<Vec<u8>>, GpuError> {
        let serial = self.readbacks.get(readback).ok_or(GpuError::UnknownId("readback"))?.serial;
        if self.completed() < serial {
            if !wait {
                return Ok(None);
            }
            if let Err(error) = self.wait_for(serial) {
                self.readbacks.remove(readback);
                return Err(error);
            }
        }
        let Readback { memory, row_pitch, bytes_per_row, rows, .. } =
            self.readbacks.remove(readback).ok_or(GpuError::UnknownId("readback"))?;
        let start = memory.offset as usize;
        let range = D3D12_RANGE { Begin: start, End: start + row_pitch * rows };
        let mut data = std::ptr::null_mut();
        // SAFETY: the copy into the memory has finished, and the mapping
        // covers its rows.
        let texels = unsafe {
            memory
                .resource
                .Map(0, Some(&raw const range), Some(&raw mut data))
                .map_err(invalid("mapping a readback"))?;
            let mapped = std::slice::from_raw_parts(data.cast::<u8>().add(start), row_pitch * rows);
            let mut texels = Vec::with_capacity(bytes_per_row * rows);
            for row in mapped.chunks(row_pitch).take(rows) {
                texels.extend_from_slice(&row[..bytes_per_row]);
            }
            memory.resource.Unmap(0, Some(&raw const NO_RANGE));
            texels
        };
        self.spare_readback = Some(memory);
        Ok(Some(texels))
    }
}

impl Drop for D3d12Gpu {
    /// Waits for the work submitted so far, which reads what the backend
    /// frees.
    fn drop(&mut self) {
        let _ = self.wait_for(self.serial - 1);
    }
}

/// A pipeline state of `program` for `desc`, with the device's root signature.
fn create_pipeline_state(
    device: &Device,
    program: &CompiledProgram,
    desc: &PipelineDesc<'_>,
    topology_type: D3D12_PRIMITIVE_TOPOLOGY_TYPE,
) -> Result<ID3D12PipelineState, GpuError> {
    let mut elements = Vec::new();
    for (slot, layout) in desc.vertex_buffers.iter().enumerate() {
        let (table, class, rate) = match layout.step {
            VertexStep::Vertex => (program.attributes, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0),
            VertexStep::Instance => (program.instance_attributes, D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA, 1),
        };
        for attribute in &layout.attributes {
            let format = conv::vertex_format(attribute.format)
                .ok_or_else(|| GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format)))?;
            let location = table
                .iter()
                .find(|info| info.id == attribute.id as usize)
                .ok_or_else(|| GpuError::Invalid(format!("{} has no attribute {}", program.name, attribute.id)))?
                .index;
            elements.push(D3D12_INPUT_ELEMENT_DESC {
                // naga's HLSL names a vertex input of location N `LOC`N.
                SemanticName: s!("LOC"),
                SemanticIndex: location as u32,
                Format: format,
                InputSlot: slot as u32,
                AlignedByteOffset: attribute.offset,
                InputSlotClass: class,
                InstanceDataStepRate: rate,
            });
        }
    }
    let mut rtv_formats = [DXGI_FORMAT_UNKNOWN; 8];
    if let Some(color) = desc.targets.color {
        rtv_formats[0] = conv::texture_format(color);
    }
    let bytecode =
        |code: &[u8]| D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() };
    let pso_desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        // SAFETY: a copy of the pointer without a reference, which the
        // description's ManuallyDrop never releases.
        pRootSignature: unsafe { std::mem::transmute_copy(device.root_signature()) },
        VS: bytecode(&program.vertex),
        PS: bytecode(&program.fragment),
        BlendState: conv::blend(&desc.state),
        SampleMask: u32::MAX,
        RasterizerState: conv::rasterizer(&desc.state),
        DepthStencilState: conv::depth_stencil(&desc.state, desc.targets.depth_stencil.is_some()),
        InputLayout: D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: elements.as_ptr(),
            NumElements: elements.len() as u32,
        },
        IBStripCutValue: D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
        PrimitiveTopologyType: topology_type,
        NumRenderTargets: u32::from(desc.targets.color.is_some()),
        RTVFormats: rtv_formats,
        DSVFormat: desc.targets.depth_stencil.map_or(DXGI_FORMAT_UNKNOWN, conv::texture_format),
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    // SAFETY: the description, its bytecode and its elements outlive
    // the call.
    unsafe { device.device().CreateGraphicsPipelineState(&raw const pso_desc) }
        .map_err(|error| GpuError::Invalid(format!("{}: CreateGraphicsPipelineState: {error}", program.name)))
}
