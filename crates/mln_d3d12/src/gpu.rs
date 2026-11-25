//! The [`Gpu`] interface on D3D12.

use std::any::Any;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
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
    TextureUsage, VertexStep, destroy_unused,
};
use mln_gfx::shaders::program::{Program, Sources};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, LAYER_SSBO_START_ID};
use mln_gfx::texture2d::SamplerState;
use mln_glsl::layout::{DescriptorSetType, IMAGE_SAMPLER_START, LAYER_STORAGE_COUNT};
use mln_glsl::{BUILT_INS, ShaderSource};
use mln_util::hash::{HashMap, HashSet};
use mln_util::size::Size;
use windows::Win32::Foundation::{HANDLE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_BOX, D3D12_CLEAR_FLAG_DEPTH, D3D12_CLEAR_FLAG_STENCIL, D3D12_CLEAR_VALUE, D3D12_CLEAR_VALUE_0,
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT, D3D12_CPU_DESCRIPTOR_HANDLE,
    D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING, D3D12_DEPTH_STENCIL_VALUE, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
    D3D12_DESCRIPTOR_HEAP_TYPE_DSV, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_FENCE_FLAG_NONE,
    D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED, D3D12_INDEX_BUFFER_VIEW,
    D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC,
    D3D12_INPUT_LAYOUT_DESC, D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_PRIMITIVE_TOPOLOGY_TYPE, D3D12_RANGE,
    D3D12_RENDER_PASS_BEGINNING_ACCESS, D3D12_RENDER_PASS_BEGINNING_ACCESS_0,
    D3D12_RENDER_PASS_BEGINNING_ACCESS_CLEAR_PARAMETERS, D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_CLEAR,
    D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_NO_ACCESS, D3D12_RENDER_PASS_BEGINNING_ACCESS_TYPE_PRESERVE,
    D3D12_RENDER_PASS_DEPTH_STENCIL_DESC, D3D12_RENDER_PASS_ENDING_ACCESS,
    D3D12_RENDER_PASS_ENDING_ACCESS_TYPE_NO_ACCESS, D3D12_RENDER_PASS_ENDING_ACCESS_TYPE_PRESERVE,
    D3D12_RENDER_PASS_FLAG_NONE, D3D12_RENDER_PASS_RENDER_TARGET_DESC, D3D12_RENDER_TARGET_VIEW_DESC,
    D3D12_RENDER_TARGET_VIEW_DESC_0, D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D,
    D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES,
    D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_RTV_DIMENSION_TEXTURE2D, D3D12_SHADER_BYTECODE,
    D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0, D3D12_SRV_DIMENSION_TEXTURE2D,
    D3D12_SUBRESOURCE_FOOTPRINT, D3D12_TEX2D_RTV, D3D12_TEX2D_SRV, D3D12_TEXTURE_COPY_LOCATION,
    D3D12_TEXTURE_COPY_LOCATION_0, D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_DATA_PITCH_ALIGNMENT, D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT, D3D12_TEXTURE_LAYOUT_UNKNOWN,
    D3D12_VERTEX_BUFFER_VIEW, D3D12_VIEWPORT, ID3D12CommandAllocator, ID3D12Fence, ID3D12GraphicsCommandList,
    ID3D12GraphicsCommandList4, ID3D12PipelineState, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R16_UINT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, s};

use crate::heaps::{CpuDescriptors, CpuSlot, SamplerHeap, TABLE_SIZE, TableHeap};
use crate::memory::{BufferMemory, Memory, NO_RANGE, TextureMemory};
use crate::mipmaps::Mipmapper;
use crate::root_signature::{
    self, ROOT_CONSTANTS, SAMPLER_INDICES, SAMPLER_TABLE, SPECIAL_CONSTANTS, TEXTURE_TABLE, View,
};
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
    memory: BufferMemory,
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
    memory: TextureMemory,
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
    mipmapped: bool,
    /// Of a texture with mips, for each level after the first, the shader
    /// resource view of the level before and the render target view of the
    /// level, which draw it.
    mips: Vec<(CpuSlot, CpuSlot)>,
}

/// A compiled program, what it reads from its bindings, and where.
struct Shader {
    key: ShaderKey,
    program: Arc<CompiledProgram>,
    reads: Vec<ShaderBinding>,
    /// The buffers it reads, by id, with their root parameter and view.
    buffers: Vec<(u32, u32, View)>,
    /// The slots of the textures it reads, which are their indices in the
    /// table.
    textures: Vec<u32>,
}

struct Pipeline {
    state: ID3D12PipelineState,
    topology: D3D_PRIMITIVE_TOPOLOGY,
    /// Whether its vertex stage reads the base vertex in the special
    /// constants.
    reads_base_vertex: bool,
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
    tags: BindingsTags,
}

struct Geometry {
    vertex_views: Vec<D3D12_VERTEX_BUFFER_VIEW>,
    index_view: D3D12_INDEX_BUFFER_VIEW,
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
/// even if it was destroyed while the pass was open.
struct Attachment {
    resource: ID3D12Resource,
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
    /// The base vertex in the special constants.
    base_vertex: Option<i32>,
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
        self.base_vertex = None;
        self.geometry = None;
        self.vertex_buffers.clear();
        self.index_buffer = None;
        self.dynamic = None;
    }
}

/// What a resource left behind when it was destroyed, freed once the work
/// recorded until then has finished.
enum Garbage {
    Buffer(BufferMemory),
    /// A texture's memory and views.
    Texture(Texture),
    /// A texture table of bindings.
    Table(u32),
    /// A caller's object, of [`D3d12Gpu::destroy_later`].
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
/// state once for the backends on it, and keeps it.
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
    /// The source each program compiles from.
    sources: Sources<ShaderSource>,
    shaders: SlotMap<ShaderId, Shader>,
    layout_tags: LayoutTags,
    pipelines: SlotMap<PipelineId, Pipeline>,
    bindings: SlotMap<BindingsId, Bindings>,
    geometries: SlotMap<GeometryId, Geometry>,
    /// What the live pipelines, bindings and geometry were made of.
    dependents: validation::Dependents,
    readbacks: SlotMap<ReadbackId, Readback>,
    /// What draws mips, made for the first texture whose mips are made.
    mipmapper: Option<Mipmapper>,
    /// The memory of the last readback taken, which the next readback of
    /// its size copies into, as a frame read back each time is.
    spare_readback: Option<BufferMemory>,
    /// The writes since the last flush, and the commands.
    copies: Option<Recording>,
    frame: Option<Recording>,
    pass: Option<OpenPass>,
    free_recordings: Vec<Recording>,
    in_flight: VecDeque<(u64, Recording)>,
    /// What destroyed resources left, with the batch after which it goes.
    garbage: Vec<(u64, Garbage)>,
    /// The fences and values that the queue waits for before the next
    /// submit, and signals after it.
    waits: Vec<(ID3D12Fence, u64)>,
    signals: Vec<(ID3D12Fence, u64)>,
    /// The timestamps of each pass, where the process measures them.
    #[cfg(feature = "bench")]
    timer: Option<crate::bench::Timer>,
    /// The fences and values that the first pass to draw into a texture has
    /// the queue wait for: the host's reads of it.
    texture_waits: HashMap<TextureId, (ID3D12Fence, u64)>,
    /// The memory of buffers, textures and readbacks, which `drop` gives
    /// back.
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
    transition_subresource(resource, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, before, after)
}

/// A transition of `subresource` of `resource` from `before` to `after`.
fn transition_subresource(
    resource: &ID3D12Resource,
    subresource: u32,
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
                Subresource: subresource,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

/// The state of a texture between passes and copies: a sampled one's lets
/// both stages read it.
fn rest_state(format: Option<TextureFormat>, usage: TextureUsage) -> D3D12_RESOURCE_STATES {
    let depth = format.is_some_and(conv::is_depth);
    if usage.sampled {
        D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE
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

/// A resource of a shader's set and binding: the inverse of
/// `mln_glsl::layout`'s `*_binding` functions.
enum Resource {
    /// A buffer, by its id.
    Buffer(u32),
    /// A texture or its sampler, by the texture's slot.
    Texture(u32),
}

fn resource_of(program: &Program, set: u32, binding: u32) -> Result<Resource, GpuError> {
    let name = program.name;
    let texture = |slot: u32| match program.textures.get(slot as usize) {
        Some(texture) if !texture.is_unused() => Ok(Resource::Texture(slot)),
        _ => Err(GpuError::Invalid(format!("{name}: no texture at slot {slot} of set {set}"))),
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
            // With the debug layer on (`MLN_D3D12_VALIDATION`), Qualcomm's
            // Adreno driver loses this clear, and
            // `fill-extrusion-multiple/stencil-wrap` fails. It isn't ours to
            // fix: the debug layer reports no error, WARP draws the test right
            // under it, and so does the Adreno without it.
            clear_stencil_with_pass: true,
            cpu_rasterizer: device.is_software(),
            max_line_width: 1.0,
            // A vertex buffer is a buffer in GPU memory that shaders may
            // read: a root SRV reads it once the common state it rests in
            // is promoted, as the input assembler does.
            binds_vertex_buffers: true,
        };
        Ok(D3d12Gpu {
            compiler,
            caps,
            fence,
            serial: 1,
            window: (0, 0),
            buffers: SlotMap::with_key(),
            stale: Vec::new(),
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
            sources: Sources::new(BUILT_INS),
            shaders: SlotMap::with_key(),
            layout_tags: LayoutTags::default(),
            pipelines: SlotMap::with_key(),
            bindings: SlotMap::with_key(),
            geometries: SlotMap::with_key(),
            dependents: validation::Dependents::default(),
            readbacks: SlotMap::with_key(),
            mipmapper: None,
            spare_readback: None,
            copies: None,
            frame: None,
            pass: None,
            free_recordings: Vec::new(),
            in_flight: VecDeque::new(),
            garbage: Vec::new(),
            waits: Vec::new(),
            signals: Vec::new(),
            #[cfg(feature = "bench")]
            timer: crate::bench::Timer::new(device.device(), device.queue()),
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
        let memory = TextureMemory::imported(resource.clone());
        self.insert_texture(memory, desc.Format, TextureInfo { format, size, usage }, state, false)
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

    /// Drops what `texture` waits for, which no submit takes. Returns
    /// whether it waited for something, which no pass took.
    pub fn forget_waits(&mut self, texture: TextureId) -> bool {
        self.texture_waits.remove(&texture).is_some()
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
        mipmapped: bool,
    ) -> Result<TextureId, GpuError> {
        let mut texture =
            Texture { memory, info, format, rest, srv: None, rtv: None, dsv: None, mipmapped, mips: Vec::new() };
        if let Err(error) = self.make_views(&mut texture) {
            self.free(Garbage::Texture(texture));
            return Err(error);
        }
        Ok(self.textures.insert(texture))
    }

    /// Makes the views of `texture` that its usage needs.
    fn make_views(&mut self, texture: &mut Texture) -> Result<(), GpuError> {
        let (info, device) = (texture.info, self.device.device());
        let depth = info.format.is_some_and(conv::is_depth);
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
            if texture.mipmapped {
                for level in 1..u32::from(resource.GetDesc().MipLevels) {
                    let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                        Format: texture.format,
                        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                            Texture2D: D3D12_TEX2D_SRV {
                                MostDetailedMip: level - 1,
                                MipLevels: 1,
                                ..Default::default()
                            },
                        },
                    };
                    let rtv_desc = D3D12_RENDER_TARGET_VIEW_DESC {
                        Format: texture.format,
                        ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                        Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 {
                            Texture2D: D3D12_TEX2D_RTV { MipSlice: level, PlaneSlice: 0 },
                        },
                    };
                    let srv = self.srv_heap.allocate(device)?;
                    let rtv = match self.rtv_heap.allocate(device) {
                        Ok(rtv) => rtv,
                        Err(error) => {
                            self.srv_heap.free(srv);
                            return Err(error);
                        }
                    };
                    device.CreateShaderResourceView(resource, Some(&raw const srv_desc), srv.handle);
                    device.CreateRenderTargetView(resource, Some(&raw const rtv_desc), rtv.handle);
                    texture.mips.push((srv, rtv));
                }
            }
        }
        Ok(())
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
            for &(_, rtv) in &entry.mips {
                list.ClearRenderTargetView(rtv.handle, &[0.0; 4], None);
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
        #[cfg(feature = "bench")]
        if let (Some(timer), Some(frame)) = (&mut self.timer, &self.frame) {
            timer.submit(&frame.list, self.serial);
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
    /// finished.
    fn collect_garbage(&mut self) {
        let completed = self.completed();
        #[cfg(feature = "bench")]
        if let Some(timer) = &mut self.timer {
            timer.finished(completed);
        }
        let done: Vec<_> = self.garbage.extract_if(.., |(serial, _)| *serial <= completed).collect();
        for (_, garbage) in done {
            self.free(garbage);
        }
    }

    /// Frees `garbage`, which no work that hasn't finished uses.
    fn free(&mut self, garbage: Garbage) {
        match garbage {
            Garbage::Buffer(memory) => self.memory.free_buffer(memory),
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
                for (srv, rtv) in texture.mips {
                    self.srv_heap.free(srv);
                    self.rtv_heap.free(rtv);
                }
                self.memory.free_texture(texture.memory);
            }
            Garbage::Table(table) => self.tables.free(table),
            Garbage::Held(_) => {}
        }
    }

    /// Keeps `memory` for the next readback of its size, and frees the one
    /// kept before. No work that hasn't finished uses either.
    fn spare(&mut self, memory: BufferMemory) {
        if let Some(spare) = self.spare_readback.replace(memory) {
            self.memory.free_buffer(spare);
        }
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
        Ok(self.buffers.insert(Buffer { memory, usage: desc.usage, size, windows }))
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
        let mip_levels = validation::check_mips(desc)?;
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
            MipLevels: mip_levels as u16,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: flags,
        };
        let info = TextureInfo { format: Some(desc.format), size: desc.size, usage: desc.usage };
        let rest = rest_state(info.format, desc.usage);
        let bytes = u64::from(width) * u64::from(height) * u64::from(desc.format.bytes_per_texel().unwrap_or(8));
        let memory = self.memory.texture(self.device.device(), &resource_desc, bytes, rest)?;
        let id = self.insert_texture(memory, format, info, rest, desc.mipmapped)?;
        if desc.usage.render_target
            && let Err(error) = self.clear_new_target(id, depth)
        {
            destroy_unused(self, [ResourceId::Texture(id)]);
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
        let (dxgi_format, rest) = (entry.format, entry.rest);
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
        let resource = &self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?.memory.resource;
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

    /// Draws each level from the one before, in the frame's commands.
    fn generate_mipmaps(&mut self, texture: TextureId) -> Result<(), GpuError> {
        if self.pass.is_some() {
            return Err(Self::open_pass("generating mips"));
        }
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        if !entry.mipmapped {
            return Err(GpuError::Invalid("generating the mips of a texture without them".to_owned()));
        }
        if entry.mips.is_empty() {
            return Ok(());
        }
        if self.mipmapper.is_none() {
            self.mipmapper = Some(Mipmapper::new(self.device.device(), &self.compiler)?);
        }
        let list = self.frame_list()?;
        let entry = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
        let (resource, rest, size) = (&entry.memory.resource, entry.rest, entry.info.size);
        let device = self.device.device();
        let mipmapper = self.mipmapper.as_mut().ok_or(GpuError::UnknownId("mipmapper"))?;
        let pipeline = mipmapper.pipeline(device, entry.format)?.clone();
        let mut tables = Vec::with_capacity(entry.mips.len());
        for &(srv, _) in &entry.mips {
            let mut views = [None; TABLE_SIZE as usize];
            views[0] = Some(srv.handle);
            match self.tables.allocate(device, &views) {
                Ok(table) => tables.push(table),
                Err(error) => {
                    for table in tables {
                        self.tables.free(table);
                    }
                    return Err(error);
                }
            }
        }
        // SAFETY: the texture and the tables live until the work finishes,
        // as destroying them waits for it, and the barriers return each
        // level that a draw draws into to the texture's rest.
        unsafe {
            list.SetGraphicsRootSignature(&mipmapper.root_signature);
            list.SetPipelineState(&pipeline);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            for ((level, &(_, rtv)), &table) in (1u32..).zip(&entry.mips).zip(&tables) {
                let (width, height) = ((size.width >> level).max(1), (size.height >> level).max(1));
                let target = D3D12_RESOURCE_STATE_RENDER_TARGET;
                list.ResourceBarrier(&[transition_subresource(resource, level, rest, target)]);
                list.SetGraphicsRootDescriptorTable(0, self.tables.gpu(table));
                list.OMSetRenderTargets(1, Some(&raw const rtv.handle), false, None);
                list.RSSetViewports(&[D3D12_VIEWPORT {
                    TopLeftX: 0.0,
                    TopLeftY: 0.0,
                    Width: width as f32,
                    Height: height as f32,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                }]);
                list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 }]);
                list.DrawInstanced(3, 1, 0, 0);
                list.ResourceBarrier(&[transition_subresource(resource, level, target, rest)]);
            }
        }
        self.bind_root(&list);
        let serial = self.serial;
        self.garbage.extend(tables.into_iter().map(|table| (serial, Garbage::Table(table))));
        Ok(())
    }

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError> {
        let slot = self.sampler_heap.slot(self.device.device(), state)?;
        Ok(self.samplers.insert(slot))
    }

    fn create_shader(&mut self, key: &ShaderKey) -> Result<ShaderId, GpuError> {
        let source = self.sources.get(key.program())?;
        let program = self.compiler.compile(key, source)?;
        let mut reads: Vec<ShaderBinding> = Vec::new();
        let mut buffers = Vec::new();
        let mut textures = Vec::new();
        for resource in &program.resources {
            match resource_of(key.program(), resource.set, resource.binding)? {
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
                Resource::Texture(slot) => {
                    if !textures.contains(&slot) {
                        textures.push(slot);
                        reads.push(ShaderBinding::Texture { slot });
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
        let reads_base_vertex = program.reads_vertex_index;
        let id = self.pipelines.insert(Pipeline { state, topology, reads_base_vertex, tags });
        self.dependents.pipeline(id, desc.shader);
        Ok(id)
    }

    fn create_bindings(&mut self, desc: &BindingsDesc<'_>) -> Result<BindingsId, GpuError> {
        let shader = self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?;
        let elements = {
            let (buffer, texture) = self.info();
            let sampler = |id| self.samplers.contains_key(id);
            let lookup = Lookup { buffer: &buffer, texture: &texture, sampler: &sampler };
            validation::check_bindings(
                desc,
                &shader.reads,
                self.caps.buffer_offset_alignment,
                self.caps.binds_vertex_buffers,
                &lookup,
            )?
        };
        let missing = |what: &str, id: u32| {
            GpuError::Invalid(format!("{} reads {what} {id}, which no entry binds", shader.program.name))
        };
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
        }
        let textures = if shader.textures.is_empty() {
            None
        } else {
            let mut views = [None; TABLE_SIZE as usize];
            let mut indices = [0; TABLE_SIZE as usize];
            for &slot in &shader.textures {
                let (texture, sampler) = desc
                    .entries
                    .iter()
                    .find_map(|entry| match *entry {
                        BindGroupEntry::Texture { slot: entry_slot, texture, sampler } if entry_slot == slot => {
                            Some((texture, sampler))
                        }
                        _ => None,
                    })
                    .ok_or_else(|| missing("texture", slot))?;
                let texture = self.textures.get(texture).ok_or(GpuError::UnknownId("texture"))?;
                let srv =
                    texture.srv.ok_or_else(|| GpuError::Invalid("a texture that shaders can't sample".to_owned()))?;
                views[slot as usize] = Some(srv.handle);
                indices[slot as usize] = *self.samplers.get(sampler).ok_or(GpuError::UnknownId("sampler"))?;
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
        let id = self.bindings.insert(Bindings { buffers, textures, tags });
        self.dependents.bindings(id, desc);
        Ok(id)
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
        let mut vertex_views = Vec::new();
        for (&(id, offset), layout) in desc.vertex_buffers.iter().zip(desc.layouts) {
            let buffer = self.buffers.get(id).ok_or(GpuError::UnknownId("buffer"))?;
            vertex_views.push(D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: buffer.memory.address() + offset,
                SizeInBytes: (buffer.size - offset) as u32,
                StrideInBytes: layout.stride,
            });
        }
        let index = self.buffers.get(desc.index_buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let index_view = D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: index.memory.address(),
            SizeInBytes: index.size as u32,
            Format: DXGI_FORMAT_R16_UINT,
        };
        let tags = GeometryTags { layout: self.layout_tags.tag(desc.shader, desc.layouts), ranges };
        let id = self.geometries.insert(Geometry { vertex_views, index_view, tags });
        self.dependents.geometry(id, desc);
        Ok(id)
    }

    /// Forgets `resource`. What it leaves in memory and descriptor heaps
    /// goes once the work recorded until then has finished. A pipeline's
    /// state stays with the device, and samplers, shaders and geometry
    /// leave nothing.
    fn destroy(&mut self, resource: ResourceId) -> Result<(), GpuError> {
        self.dependents.destroy(resource)?;
        let garbage = match resource {
            ResourceId::Buffer(id) => self.buffers.remove(id).map(|buffer| Garbage::Buffer(buffer.memory)),
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
            ResourceId::Pipeline(id) => {
                self.pipelines.remove(id);
                None
            }
            ResourceId::Bindings(id) => {
                self.bindings.remove(id).and_then(|bindings| bindings.textures).map(|(table, _)| Garbage::Table(table))
            }
            ResourceId::Geometry(id) => {
                self.geometries.remove(id);
                None
            }
            ResourceId::Readback(id) => self.readbacks.remove(id).map(|readback| Garbage::Buffer(readback.memory)),
        };
        if let Some(garbage) = garbage {
            self.garbage.push((self.serial, garbage));
        }
        Ok(())
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
                resource: texture.memory.resource.clone(),
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
                resource: texture.memory.resource.clone(),
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
            #[cfg(feature = "bench")]
            if let Some(timer) = &mut self.timer {
                timer.begin(self.device.device(), &list);
            }
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
            base_vertex: None,
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
        // memory and tables that the bindings and geometry point at are
        // destroyed after them, and freed once the work recorded before has
        // finished, and the device keeps the pipeline's state.
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
            if pipeline.reads_base_vertex && open.base_vertex != Some(draw.base_vertex) {
                list.SetGraphicsRoot32BitConstant(SPECIAL_CONSTANTS, draw.base_vertex as u32, 0);
                open.base_vertex = Some(draw.base_vertex);
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
            .map(|(attachment, state)| transition(&attachment.resource, state, attachment.rest))
            .collect();
        // SAFETY: the pass is open on the list, and the barriers return the
        // attachments to their rest.
        unsafe {
            open.list.EndRenderPass();
            #[cfg(feature = "bench")]
            if let Some(timer) = &mut self.timer {
                timer.end(&open.list);
            }
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
        let (format, rest) = (entry.format, entry.rest);
        let row_pitch = readback.bytes_per_row.next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT as usize);
        let rows = region.size.height as usize;
        let size = (row_pitch * rows) as u64;
        let buffer = match self.spare_readback.take() {
            Some(spare) if spare.size >= size && spare.size <= size.next_power_of_two().max(512) => spare,
            spare => {
                if let Some(spare) = spare {
                    self.memory.free_buffer(spare);
                }
                // A copy from a texture writes from an offset of 512 bytes.
                let size = size.max(u64::from(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT));
                self.memory.buffer(MemoryLocation::GpuToCpu, size)?
            }
        };
        debug_assert!(buffer.offset.is_multiple_of(u64::from(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT)));
        let list = match self.frame_list() {
            Ok(list) => list,
            Err(error) => {
                self.spare(buffer);
                return Err(error);
            }
        };
        let Some(entry) = self.textures.get(texture) else {
            self.spare(buffer);
            return Err(GpuError::UnknownId("texture"));
        };
        let resource = &entry.memory.resource;
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
        let serial = match self.submit() {
            Ok(serial) => serial,
            Err(error) => {
                self.garbage.push((self.serial, Garbage::Buffer(buffer)));
                return Err(error);
            }
        };
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
                if let Some(readback) = self.readbacks.remove(readback) {
                    self.garbage.push((serial, Garbage::Buffer(readback.memory)));
                }
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
            memory.resource.Map(0, Some(&raw const range), Some(&raw mut data)).map(|()| {
                let mapped = std::slice::from_raw_parts(data.cast::<u8>().add(start), row_pitch * rows);
                let mut texels = Vec::with_capacity(bytes_per_row * rows);
                for row in mapped.chunks(row_pitch).take(rows) {
                    texels.extend_from_slice(&row[..bytes_per_row]);
                }
                memory.resource.Unmap(0, Some(&raw const NO_RANGE));
                texels
            })
        };
        self.spare(memory);
        texels.map(Some).map_err(invalid("mapping a readback"))
    }
}

impl Drop for D3d12Gpu {
    /// Waits for the work submitted so far, which reads what the backend
    /// frees, then gives its memory back.
    fn drop(&mut self) {
        let _ = self.wait_for(self.serial - 1);
        for (_, garbage) in std::mem::take(&mut self.garbage) {
            self.free(garbage);
        }
        let memory = &mut self.memory;
        for (_, buffer) in self.buffers.drain() {
            memory.free_buffer(buffer.memory);
        }
        for (_, texture) in self.textures.drain() {
            memory.free_texture(texture.memory);
        }
        for (_, readback) in self.readbacks.drain() {
            memory.free_buffer(readback.memory);
        }
        for (_, indices) in self.sampler_indices.drain() {
            memory.free_buffer(indices);
        }
        if let Some(spare) = self.spare_readback.take() {
            memory.free_buffer(spare);
        }
        self.upload.free(memory);
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
        let (class, rate) = match layout.step {
            VertexStep::Vertex => (D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0),
            VertexStep::Instance => (D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA, 1),
        };
        for attribute in &layout.attributes {
            let format = conv::vertex_format(attribute.format)
                .ok_or_else(|| GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format)))?;
            let location = attribute.location;
            if program.program.attribute_at(location as usize).is_none_or(|(step, _)| step != layout.step) {
                return Err(GpuError::Invalid(format!(
                    "{} has no {:?} attribute at {location}",
                    program.name, layout.step
                )));
            }
            elements.push(D3D12_INPUT_ELEMENT_DESC {
                // naga's HLSL names a vertex input of location N `LOC`N.
                SemanticName: s!("LOC"),
                SemanticIndex: program.source.location(location as usize),
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
