//! The [`Gpu`] interface on D3D12.

use std::any::Any;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use mln_gfx::gpu::validation::{
    self, BindingsTags, BufferInfo, GeometryTags, LayoutTags, Lookup, PassTarget, PipelineTags, ShaderBinding,
    TextureInfo,
};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferDesc, BufferId, BufferUsage, Caps, ClipDepth, Draw, DynamicState,
    GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PassDesc, PipelineDesc, PipelineId, ReadbackId, ResourceId,
    SamplerId, Scissor, ShaderId, ShaderKey, SlotMap, TextureDesc, TextureId, TextureRegion, TextureUsage, VertexStep,
};
use mln_gfx::shaders::shader_defines::{DRAWABLE_RESERVED_UBO_COUNT, LAYER_SSBO_START_ID};
use mln_gfx::texture2d::SamplerState;
use mln_glsl::TextureInfo as ShaderTexture;
use mln_glsl::layout::{DescriptorSetType, IMAGE_SAMPLER_START, LAYER_STORAGE_COUNT};
use mln_util::hash::HashMap;
use mln_util::size::Size;
use windows::Win32::Foundation::{HANDLE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_BOX, D3D12_CLEAR_FLAG_DEPTH, D3D12_CLEAR_FLAG_STENCIL, D3D12_CLEAR_FLAGS, D3D12_COMMAND_LIST_TYPE_DIRECT,
    D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT, D3D12_CPU_DESCRIPTOR_HANDLE,
    D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
    D3D12_FENCE_FLAG_NONE, D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_TYPE, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK, D3D12_HEAP_TYPE_UPLOAD,
    D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED, D3D12_INDEX_BUFFER_VIEW, D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
    D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RANGE, D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER,
    D3D12_SHADER_BYTECODE, D3D12_SUBRESOURCE_FOOTPRINT, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_DATA_PITCH_ALIGNMENT, D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT, D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
    D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12_VERTEX_BUFFER_VIEW, D3D12_VIEWPORT, ID3D12CommandAllocator, ID3D12Device,
    ID3D12Fence, ID3D12GraphicsCommandList, ID3D12PipelineState, ID3D12Resource, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R16_UINT, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, s};

use crate::heaps::{CpuDescriptors, CpuSlot, SamplerHeap, TABLE_SIZE, TableHeap};
use crate::root_signature::{self, ROOT_CONSTANTS, SAMPLER_INDICES, SAMPLER_TABLE, TEXTURE_TABLE, View};
use crate::shaders::{CompiledProgram, ShaderCompiler};
use crate::upload::UploadRing;
use crate::{CustomLayerEncoder, CustomLayerPass, Device, conv};

/// The size of the buffers that small buffers share.
const PAGE: u64 = 64 << 10;
/// The alignment of a buffer, in its page or alone: what a root CBV needs.
const BUFFER_ALIGNMENT: u64 = D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT as u64;
/// The widest and tallest texture of feature level 11.0.
const MAX_TEXTURE_SIZE: u32 = D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION;

/// A buffer, or the part of a shared page that a small buffer takes.
struct Buffer {
    resource: ID3D12Resource,
    usage: BufferUsage,
    /// Where the buffer starts in `resource`, and its bytes from there.
    offset: u64,
    size: u64,
}

impl Buffer {
    fn address(&self) -> u64 {
        // SAFETY: a plain query.
        unsafe { self.resource.GetGPUVirtualAddress() + self.offset }
    }
}

struct Texture {
    resource: ID3D12Resource,
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
    program: Rc<CompiledProgram>,
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

/// What a draw sets in the root signature: each buffer's address, the
/// texture table and the address of the sampler indices.
struct Bindings {
    buffers: Vec<(u32, View, u64)>,
    textures: Option<(u32, u64)>,
    /// The resources they bind, which live as long as the bindings.
    #[expect(dead_code, reason = "held, not read")]
    keep: Vec<ID3D12Resource>,
    tags: BindingsTags,
}

struct Geometry {
    vertex_views: Vec<D3D12_VERTEX_BUFFER_VIEW>,
    index_view: D3D12_INDEX_BUFFER_VIEW,
    /// The buffers it reads, which live as long as the geometry.
    #[expect(dead_code, reason = "held, not read")]
    keep: Vec<ID3D12Resource>,
    tags: GeometryTags,
}

/// A texture's texels, copied into a buffer that the CPU maps once the
/// submission `serial` has finished.
struct Readback {
    resource: ID3D12Resource,
    /// The bytes in each row of `resource`, which D3D12 pads, and of the
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
    resource: ID3D12Resource,
    rest: D3D12_RESOURCE_STATES,
    view: D3D12_CPU_DESCRIPTOR_HANDLE,
    format: DXGI_FORMAT,
}

/// The pass draws record into, and what the draws so far set on it.
struct OpenPass {
    list: ID3D12GraphicsCommandList,
    target: PassTarget,
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

/// What a resource left behind when it was destroyed, freed once the work
/// recorded until then has finished.
enum Garbage {
    Buffer(#[expect(dead_code, reason = "held until the GPU is done with it")] Buffer),
    Texture(Texture),
    Pipeline(#[expect(dead_code, reason = "held until the GPU is done with it")] Pipeline),
    Bindings(Bindings),
    Geometry(#[expect(dead_code, reason = "held until the GPU is done with it")] Geometry),
    Readback(#[expect(dead_code, reason = "held until the GPU is done with it")] Readback),
}

/// A [`Gpu`] on a D3D12 [`Device`].
///
/// Every pipeline has one root signature (`root_signature`), so a draw
/// binds its buffers as root CBVs and SRVs, its textures as a table that
/// its bindings made, and its samplers through the indices that its
/// bindings point at.
///
/// Buffers live in GPU memory, small ones in shared pages. Writes go through
/// persistently mapped upload memory, into a command list that runs ahead of
/// the next flush's commands. Buffers take the states their use asks for on
/// their own; textures rest in one state, which passes and copies leave them
/// in again.
///
/// Destroying a resource frees it once the work recorded until then has
/// finished, as each flush signals a fence.
pub struct D3d12Gpu {
    device: Device,
    compiler: ShaderCompiler,
    caps: Caps,
    root_signature: ID3D12RootSignature,
    fence: ID3D12Fence,
    /// The submission that the work being recorded will be.
    serial: u64,
    /// The fence that each flush's writes signal, and how many it signaled.
    copy_fence: ID3D12Fence,
    copies_submitted: u64,
    buffers: SlotMap<BufferId, Buffer>,
    /// The page that small buffers are taken from, and how many of its
    /// bytes are taken.
    buffer_page: Option<(ID3D12Resource, u64)>,
    upload: UploadRing,
    textures: SlotMap<TextureId, Texture>,
    srv_heap: CpuDescriptors,
    rtv_heap: CpuDescriptors,
    dsv_heap: CpuDescriptors,
    tables: TableHeap,
    sampler_heap: SamplerHeap,
    samplers: SlotMap<SamplerId, u32>,
    /// The upload memory that holds each set of sampler indices bindings
    /// point at, kept as long as the `D3d12Gpu`, as there are few sets.
    sampler_indices: HashMap<[u32; TABLE_SIZE as usize], u64>,
    sampler_index_pages: Vec<(ID3D12Resource, *mut u8, u64)>,
    shaders: SlotMap<ShaderId, Shader>,
    layout_tags: LayoutTags,
    pipelines: SlotMap<PipelineId, Pipeline>,
    bindings: SlotMap<BindingsId, Bindings>,
    geometries: SlotMap<GeometryId, Geometry>,
    readbacks: SlotMap<ReadbackId, Readback>,
    /// The writes since the last flush, and the commands.
    copies: Option<Recording>,
    frame: Option<Recording>,
    pass: Option<OpenPass>,
    free_recordings: Vec<Recording>,
    in_flight: VecDeque<(u64, Recording)>,
    garbage: Vec<(u64, Garbage)>,
}

fn invalid(what: &str) -> impl Fn(windows::core::Error) -> GpuError + '_ {
    move |error| GpuError::Invalid(format!("{what}: {error}"))
}

/// The range of a mapping that the CPU reads or writes none of.
pub(crate) static NO_RANGE: D3D12_RANGE = D3D12_RANGE { Begin: 0, End: 0 };

/// The description of a buffer of `size` bytes.
pub(crate) fn buffer_desc(size: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    }
}

/// A committed resource of `desc` in a heap of `heap`.
pub(crate) fn create_resource(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource, GpuError> {
    let properties = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
    let mut resource: Option<ID3D12Resource> = None;
    // SAFETY: the descriptions outlive the call.
    unsafe {
        device.CreateCommittedResource(
            &raw const properties,
            D3D12_HEAP_FLAG_NONE,
            desc,
            state,
            None,
            &raw mut resource,
        )
    }
    .map_err(invalid("CreateCommittedResource"))?;
    resource.ok_or_else(|| GpuError::Invalid("CreateCommittedResource made no resource".to_owned()))
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
fn rest_state(format: Option<mln_gfx::gpu::TextureFormat>, usage: TextureUsage) -> D3D12_RESOURCE_STATES {
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
    /// make the root signature, the fence or the descriptor heaps.
    pub fn new(device: &Device) -> Result<D3d12Gpu, GpuError> {
        let d3d = device.device();
        let compiler = ShaderCompiler::new()?;
        let root_signature = root_signature::create(d3d)?;
        // SAFETY: a plain call.
        let fence: ID3D12Fence =
            unsafe { d3d.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.map_err(invalid("CreateFence"))?;
        // SAFETY: a plain call.
        let copy_fence: ID3D12Fence =
            unsafe { d3d.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.map_err(invalid("CreateFence"))?;
        let caps = Caps {
            buffer_offset_alignment: BUFFER_ALIGNMENT as u32,
            max_texture_size: MAX_TEXTURE_SIZE,
            half_float_render_targets: true,
            clip_depth: ClipDepth::ZeroToOne,
            render_target_depth: None,
            clear_stencil_with_pass: true,
            line_expressions: false,
            cpu_rasterizer: device.is_software(),
            max_line_width: 1.0,
        };
        Ok(D3d12Gpu {
            compiler,
            caps,
            root_signature,
            fence,
            serial: 1,
            copy_fence,
            copies_submitted: 0,
            buffers: SlotMap::with_key(),
            buffer_page: None,
            upload: UploadRing::default(),
            textures: SlotMap::with_key(),
            srv_heap: CpuDescriptors::new(d3d, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            rtv_heap: CpuDescriptors::new(d3d, D3D12_DESCRIPTOR_HEAP_TYPE_RTV),
            dsv_heap: CpuDescriptors::new(d3d, D3D12_DESCRIPTOR_HEAP_TYPE_DSV),
            tables: TableHeap::new(d3d)?,
            sampler_heap: SamplerHeap::new(d3d)?,
            samplers: SlotMap::with_key(),
            sampler_indices: HashMap::default(),
            sampler_index_pages: Vec::new(),
            shaders: SlotMap::with_key(),
            layout_tags: LayoutTags::default(),
            pipelines: SlotMap::with_key(),
            bindings: SlotMap::with_key(),
            geometries: SlotMap::with_key(),
            readbacks: SlotMap::with_key(),
            copies: None,
            frame: None,
            pass: None,
            free_recordings: Vec::new(),
            in_flight: VecDeque::new(),
            garbage: Vec::new(),
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
        self.insert_texture(resource.clone(), desc.Format, TextureInfo { format, size, usage }, state)
    }

    /// The D3D12 resource of `texture`, and the state it is in between
    /// passes and copies.
    #[must_use]
    pub fn native_texture(&self, texture: TextureId) -> Option<(&ID3D12Resource, D3D12_RESOURCE_STATES)> {
        self.textures.get(texture).map(|texture| (&texture.resource, texture.rest))
    }

    fn d3d(&self) -> &ID3D12Device {
        self.device.device()
    }

    fn completed(&self) -> u64 {
        // SAFETY: a plain query.
        unsafe { self.fence.GetCompletedValue() }
    }

    fn insert_texture(
        &mut self,
        resource: ID3D12Resource,
        format: DXGI_FORMAT,
        info: TextureInfo,
        rest: D3D12_RESOURCE_STATES,
    ) -> Result<TextureId, GpuError> {
        let depth = info.format.is_some_and(conv::is_depth);
        let device = self.device.device();
        let mut texture = Texture { resource, info, format, rest, srv: None, rtv: None, dsv: None };
        // SAFETY: the views are of the texture, into descriptors no work
        // reads.
        unsafe {
            if info.usage.sampled && !depth {
                let slot = self.srv_heap.allocate(device)?;
                device.CreateShaderResourceView(&texture.resource, None, slot.handle);
                texture.srv = Some(slot);
            }
            if info.usage.render_target && depth {
                let slot = self.dsv_heap.allocate(device)?;
                device.CreateDepthStencilView(&texture.resource, None, slot.handle);
                texture.dsv = Some(slot);
            } else if info.usage.render_target {
                let slot = self.rtv_heap.allocate(device)?;
                device.CreateRenderTargetView(&texture.resource, None, slot.handle);
                texture.rtv = Some(slot);
            }
        }
        Ok(self.textures.insert(texture))
    }

    fn info(&self) -> (impl Fn(BufferId) -> Option<BufferInfo> + '_, impl Fn(TextureId) -> Option<TextureInfo> + '_) {
        let buffer =
            |id| self.buffers.get(id).map(|buffer: &Buffer| BufferInfo { usage: buffer.usage, size: buffer.size });
        let texture = |id| self.textures.get(id).map(|texture: &Texture| texture.info);
        (buffer, texture)
    }

    /// A command list to record into: a finished one's again, or a new one.
    fn recording(&mut self) -> Result<Recording, GpuError> {
        let completed = self.completed();
        while let Some((serial, _)) = self.in_flight.front()
            && *serial <= completed
        {
            let (_, recording) = self.in_flight.pop_front().ok_or(GpuError::Invalid("no recording".to_owned()))?;
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
            list.SetGraphicsRootSignature(&self.root_signature);
            list.SetGraphicsRootDescriptorTable(
                SAMPLER_TABLE,
                self.sampler_heap.heap.GetGPUDescriptorHandleForHeapStart(),
            );
        }
    }

    /// The command list of the frame's commands.
    fn frame_list(&mut self) -> Result<ID3D12GraphicsCommandList, GpuError> {
        if self.frame.is_none() {
            let recording = self.recording()?;
            self.bind_root(&recording.list);
            self.frame = Some(recording);
        }
        self.frame.as_ref().map(|frame| frame.list.clone()).ok_or(GpuError::Invalid("no frame".to_owned()))
    }

    /// The command list of the writes since the last flush.
    fn copy_list(&mut self) -> Result<ID3D12GraphicsCommandList, GpuError> {
        if self.copies.is_none() {
            self.copies = Some(self.recording()?);
        }
        self.copies.as_ref().map(|copies| copies.list.clone()).ok_or(GpuError::Invalid("no copies".to_owned()))
    }

    /// Submits the writes since the last flush, then the frame's commands,
    /// and signals the fence. Returns the submission's serial.
    fn submit(&mut self) -> Result<u64, GpuError> {
        let queue = self.device.queue();
        let execute = |recording: &Recording| -> Result<(), GpuError> {
            // SAFETY: the list holds commands of resources the backend
            // keeps until the fence passes the submission.
            unsafe {
                recording.list.Close().map_err(invalid("ID3D12GraphicsCommandList::Close"))?;
                queue.ExecuteCommandLists(&[Some(recording.list.cast().map_err(invalid("ID3D12CommandList"))?)]);
            }
            Ok(())
        };
        if let Some(copies) = self.copies.take() {
            // The writes wait for the work submitted before, which reads
            // what they overwrite, and the frame for the writes. Each is a
            // submission of its own, so that the buffers that the writes
            // promoted decay to the common state before the frame reads
            // them.
            self.copies_submitted += 1;
            // SAFETY: plain calls.
            unsafe { queue.Wait(&self.fence, self.serial - 1) }.map_err(invalid("ID3D12CommandQueue::Wait"))?;
            execute(&copies)?;
            // SAFETY: plain calls.
            unsafe {
                queue.Signal(&self.copy_fence, self.copies_submitted).map_err(invalid("ID3D12CommandQueue::Signal"))?;
                queue.Wait(&self.copy_fence, self.copies_submitted).map_err(invalid("ID3D12CommandQueue::Wait"))?;
            }
            self.in_flight.push_back((self.serial, copies));
        }
        if let Some(frame) = self.frame.take() {
            execute(&frame)?;
            self.in_flight.push_back((self.serial, frame));
        }
        // SAFETY: a plain call.
        unsafe { queue.Signal(&self.fence, self.serial) }.map_err(invalid("ID3D12CommandQueue::Signal"))?;
        let serial = self.serial;
        self.serial += 1;
        Ok(serial)
    }

    /// Blocks until the submission `serial` has finished.
    fn wait_for(&self, serial: u64) -> Result<(), GpuError> {
        if self.completed() >= serial {
            return Ok(());
        }
        // SAFETY: without an event, the call returns once the fence has
        // reached the value.
        unsafe { self.fence.SetEventOnCompletion(serial, HANDLE::default()) }
            .map_err(|error| GpuError::Invalid(format!("waiting for the GPU: {error}")))
    }

    /// Frees what destroyed resources left behind, once their work has
    /// finished.
    fn collect_garbage(&mut self) {
        let completed = self.completed();
        let (done, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.garbage).into_iter().partition(|(serial, _)| *serial <= completed);
        self.garbage = waiting;
        for (_, garbage) in done {
            match garbage {
                Garbage::Texture(texture) => {
                    for (slot, heap) in [
                        (texture.srv, &mut self.srv_heap),
                        (texture.rtv, &mut self.rtv_heap),
                        (texture.dsv, &mut self.dsv_heap),
                    ] {
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
                Garbage::Buffer(_) | Garbage::Pipeline(_) | Garbage::Geometry(_) | Garbage::Readback(_) => {}
            }
        }
    }

    /// `size` bytes of GPU memory: of the page small buffers share, or of
    /// a buffer of its own. The resource and the offset in it.
    fn allocate_buffer(&mut self, size: u64) -> Result<(ID3D12Resource, u64), GpuError> {
        let size = size.next_multiple_of(BUFFER_ALIGNMENT);
        let create = |device: &ID3D12Device, size| {
            create_resource(device, D3D12_HEAP_TYPE_DEFAULT, &buffer_desc(size), D3D12_RESOURCE_STATE_COMMON)
        };
        if size > PAGE / 4 {
            return Ok((create(self.d3d(), size)?, 0));
        }
        let (page, taken) = match &mut self.buffer_page {
            Some((page, taken)) if *taken + size <= PAGE => (page, taken),
            _ => {
                let page = create(self.d3d(), PAGE)?;
                let (page, taken) = self.buffer_page.insert((page, 0));
                (page, taken)
            }
        };
        let offset = *taken;
        *taken += size;
        Ok((page.clone(), offset))
    }

    /// The address of upload memory that holds `indices`, which the sampler
    /// index buffers of bindings point at.
    fn sampler_index_address(&mut self, indices: [u32; TABLE_SIZE as usize]) -> Result<u64, GpuError> {
        const SIZE: u64 = 16;
        if let Some(&address) = self.sampler_indices.get(&indices) {
            return Ok(address);
        }
        let room = self.sampler_index_pages.last().is_some_and(|(_, _, taken)| taken + SIZE <= PAGE);
        if !room {
            let resource = create_resource(
                self.d3d(),
                D3D12_HEAP_TYPE_UPLOAD,
                &buffer_desc(PAGE),
                D3D12_RESOURCE_STATE_GENERIC_READ,
            )?;
            let mut data = std::ptr::null_mut();
            // SAFETY: the buffer stays mapped as long as it lives; the CPU
            // reads none of it.
            unsafe { resource.Map(0, Some(&raw const NO_RANGE), Some(&raw mut data)) }
                .map_err(invalid("mapping sampler indices"))?;
            self.sampler_index_pages.push((resource, data.cast(), 0));
        }
        let (resource, data, taken) =
            self.sampler_index_pages.last_mut().ok_or(GpuError::Invalid("no sampler index page".to_owned()))?;
        // SAFETY: the bytes are inside the mapped page, and past every set
        // that work reads.
        unsafe {
            std::ptr::copy_nonoverlapping(
                indices.as_ptr().cast::<u8>(),
                data.add(*taken as usize),
                size_of_val(&indices),
            );
        }
        // SAFETY: a plain query.
        let address = unsafe { resource.GetGPUVirtualAddress() } + *taken;
        *taken += SIZE;
        self.sampler_indices.insert(indices, address);
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
        let (resource, offset) = self.allocate_buffer(size)?;
        Ok(self.buffers.insert(Buffer { resource, usage: desc.usage, offset, size }))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let entry = self.buffers.get(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let end = offset + data.len() as u64;
        if end > entry.size {
            return Err(GpuError::Invalid(format!("a write to bytes {offset}..{end} of a buffer of {}", entry.size)));
        }
        if data.is_empty() {
            return Ok(());
        }
        let (resource, at) = (entry.resource.clone(), entry.offset + offset);
        let completed = self.completed();
        let upload =
            self.upload.write(self.device.device(), self.serial, completed, data.len() as u64, 4, |bytes| {
                bytes.copy_from_slice(data);
            })?;
        let list = self.copy_list()?;
        // SAFETY: both buffers live until the copy finishes: the upload page
        // until its serial does, and the buffer as destroyed buffers do.
        unsafe { list.CopyBufferRegion(&resource, at, &upload.resource, upload.offset, data.len() as u64) };
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
        let resource = create_resource(self.d3d(), D3D12_HEAP_TYPE_DEFAULT, &resource_desc, rest)?;
        self.insert_texture(resource, format, info, rest)
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
        let (resource, dxgi_format, rest) = (entry.resource.clone(), entry.format, entry.rest);
        let completed = self.completed();
        let upload = self.upload.write(
            self.device.device(),
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
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: copies of the pointers without references, which the
            // locations' ManuallyDrop never releases.
            pResource: unsafe { std::mem::transmute_copy(&resource) },
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
            list.ResourceBarrier(&[transition(&resource, rest, D3D12_RESOURCE_STATE_COPY_DEST)]);
            list.CopyTextureRegion(&raw const dst, region.x, region.y, 0, &raw const src, None);
            list.ResourceBarrier(&[transition(&resource, D3D12_RESOURCE_STATE_COPY_DEST, rest)]);
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
        Ok(self.shaders.insert(Shader { program: Rc::new(program), reads, buffers, textures }))
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<PipelineId, GpuError> {
        let shader = self.shaders.get(desc.shader).ok_or(GpuError::UnknownId("shader"))?;
        let program = &shader.program;
        let (topology_type, topology) = conv::topology(desc.state.primitive)
            .ok_or_else(|| GpuError::Unsupported(format!("{:?} primitives", desc.state.primitive)))?;
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
            pRootSignature: unsafe { std::mem::transmute_copy(&self.root_signature) },
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
        let state: ID3D12PipelineState = unsafe {
            self.device.device().CreateGraphicsPipelineState(&raw const pso_desc)
        }
        .map_err(|error| GpuError::Invalid(format!("{}: CreateGraphicsPipelineState: {error}", program.name)))?;
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
        let mut keep = Vec::new();
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
            buffers.push((parameter, view, buffer.address() + offset));
            keep.push(buffer.resource.clone());
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
                keep.push(texture.resource.clone());
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
        Ok(self.bindings.insert(Bindings { buffers, textures, keep, tags }))
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
                BufferLocation: buffer.address() + offset,
                SizeInBytes: (buffer.size - offset) as u32,
                StrideInBytes: layout.stride,
            });
            keep.push(buffer.resource.clone());
        }
        let index = self.buffers.get(desc.index_buffer).ok_or(GpuError::UnknownId("buffer"))?;
        let index_view = D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: index.address(),
            SizeInBytes: index.size as u32,
            Format: DXGI_FORMAT_R16_UINT,
        };
        keep.push(index.resource.clone());
        let tags = GeometryTags { layout: self.layout_tags.tag(desc.shader, desc.layouts), ranges };
        Ok(self.geometries.insert(Geometry { vertex_views, index_view, keep, tags }))
    }

    fn destroy(&mut self, resource: ResourceId) {
        let garbage = match resource {
            ResourceId::Buffer(id) => self.buffers.remove(id).map(Garbage::Buffer),
            ResourceId::Texture(id) => self.textures.remove(id).map(Garbage::Texture),
            ResourceId::Sampler(id) => {
                self.samplers.remove(id);
                None
            }
            ResourceId::Shader(id) => {
                self.shaders.remove(id);
                None
            }
            ResourceId::Pipeline(id) => self.pipelines.remove(id).map(Garbage::Pipeline),
            ResourceId::Bindings(id) => self.bindings.remove(id).map(Garbage::Bindings),
            ResourceId::Geometry(id) => self.geometries.remove(id).map(Garbage::Geometry),
            ResourceId::Readback(id) => self.readbacks.remove(id).map(Garbage::Readback),
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
        let list = self.frame_list()?;
        let mut barriers = Vec::new();
        let mut color = None;
        if let Some(attachment) = desc.color {
            let texture = self.textures.get(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
            let rtv = texture
                .rtv
                .ok_or_else(|| GpuError::Invalid("a color attachment that isn't a render target".to_owned()))?;
            if texture.rest != D3D12_RESOURCE_STATE_RENDER_TARGET {
                barriers.push(transition(&texture.resource, texture.rest, D3D12_RESOURCE_STATE_RENDER_TARGET));
            }
            let pass_attachment = Attachment {
                resource: texture.resource.clone(),
                rest: texture.rest,
                view: rtv.handle,
                format: texture.format,
            };
            color = Some((pass_attachment, attachment.load));
        }
        let mut depth_stencil = None;
        if let Some(attachment) = desc.depth_stencil {
            let texture = self.textures.get(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
            let dsv = texture
                .dsv
                .ok_or_else(|| GpuError::Invalid("a depth attachment that isn't a render target".to_owned()))?;
            if texture.rest != D3D12_RESOURCE_STATE_DEPTH_WRITE {
                barriers.push(transition(&texture.resource, texture.rest, D3D12_RESOURCE_STATE_DEPTH_WRITE));
            }
            let stencil = texture.info.format.is_some_and(mln_gfx::gpu::TextureFormat::has_stencil);
            let pass_attachment = Attachment {
                resource: texture.resource.clone(),
                rest: texture.rest,
                view: dsv.handle,
                format: texture.format,
            };
            depth_stencil = Some((pass_attachment, attachment, stencil));
        }
        // SAFETY: the attachments live as long as the pass's work, and the
        // barriers move them from their rest into the states the pass draws
        // in.
        unsafe {
            if !barriers.is_empty() {
                list.ResourceBarrier(&barriers);
            }
            let rtv = color.as_ref().map(|(attachment, _)| attachment.view);
            let dsv = depth_stencil.as_ref().map(|(attachment, ..)| attachment.view);
            list.OMSetRenderTargets(
                u32::from(rtv.is_some()),
                rtv.as_ref().map(std::ptr::from_ref),
                false,
                dsv.as_ref().map(std::ptr::from_ref),
            );
            if let (Some(handle), Some((_, LoadOp::Clear(value)))) = (rtv, &color) {
                list.ClearRenderTargetView(handle, &self::color(*value), None);
            }
            if let (Some(handle), Some((_, attachment, stencil))) = (dsv, &depth_stencil) {
                let (attachment, stencil) = (*attachment, *stencil);
                let mut flags = D3D12_CLEAR_FLAGS(0);
                let mut depth = 1.0;
                let mut stencil_value = 0;
                if let LoadOp::Clear(value) = attachment.depth {
                    flags |= D3D12_CLEAR_FLAG_DEPTH;
                    depth = value;
                }
                if let (true, LoadOp::Clear(value)) = (stencil, attachment.stencil) {
                    flags |= D3D12_CLEAR_FLAG_STENCIL;
                    stencil_value = value as u8;
                }
                if flags.0 != 0 {
                    list.ClearDepthStencilView(handle, flags, depth, stencil_value, None);
                }
            }
        }
        self.pass = Some(OpenPass {
            list,
            target,
            color: color.map(|(attachment, _)| attachment),
            depth_stencil: depth_stencil.map(|(attachment, ..)| attachment),
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
        // keep their resources.
        unsafe {
            if open.pipeline != Some(draw.pipeline) {
                list.SetPipelineState(&pipeline.state);
                list.IASetPrimitiveTopology(pipeline.topology);
                open.pipeline = Some(draw.pipeline);
            }
            if open.bindings != Some(draw.bindings) {
                for &(parameter, view, address) in &bindings.buffers {
                    match view {
                        View::Constant => list.SetGraphicsRootConstantBufferView(parameter, address),
                        View::Shader => list.SetGraphicsRootShaderResourceView(parameter, address),
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

    /// Lends the open pass's command list, or the frame's. A pass that a
    /// custom layer draws into has a color attachment. The backend sets its
    /// heaps, root signature and render targets again after the layer.
    fn lend(&mut self, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError> {
        let device = self.device.device().clone();
        if let Some(mut open) = self.pass.take() {
            let Some((rtv, color_format)) = open.color.as_ref().map(|color| (color.view, color.format)) else {
                self.pass = Some(open);
                return Err(GpuError::Unsupported("a custom layer's pass without a color attachment".to_owned()));
            };
            let depth_stencil = open.depth_stencil.as_ref().map(|depth| (depth.view, depth.format));
            let mut lent = CustomLayerPass {
                device,
                command_list: open.list.clone(),
                render_target: rtv,
                color_format,
                depth_stencil: depth_stencil.map(|(dsv, _)| dsv),
                depth_stencil_format: depth_stencil.map_or(DXGI_FORMAT_UNKNOWN, |(_, format)| format),
            };
            f(&mut lent);
            self.bind_root(&open.list);
            // SAFETY: the attachments' views live as long as the pass.
            unsafe {
                let dsv = depth_stencil.map(|(dsv, _)| dsv);
                open.list.OMSetRenderTargets(1, Some(&raw const rtv), false, dsv.as_ref().map(std::ptr::from_ref));
            }
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
        if !barriers.is_empty() {
            // SAFETY: the barriers return the attachments to their rest.
            unsafe { open.list.ResourceBarrier(&barriers) };
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

    /// Copies into a readback buffer, with D3D12's row pitch.
    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError> {
        if self.pass.is_some() {
            return Err(Self::open_pass("a readback"));
        }
        let entry = self.textures.get(texture);
        let readback = validation::check_readback(entry.map(|texture| texture.info), region)?;
        let entry = entry.ok_or(GpuError::UnknownId("texture"))?;
        let (resource, format, rest) = (entry.resource.clone(), entry.format, entry.rest);
        let row_pitch = readback.bytes_per_row.next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT as usize);
        let rows = region.size.height as usize;
        let buffer = create_resource(
            self.d3d(),
            D3D12_HEAP_TYPE_READBACK,
            &buffer_desc((row_pitch * rows) as u64),
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let list = self.frame_list()?;
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: copies of the pointers without references, which the
            // locations' ManuallyDrop never releases.
            pResource: unsafe { std::mem::transmute_copy(&buffer) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
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
            pResource: unsafe { std::mem::transmute_copy(&resource) },
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
            list.ResourceBarrier(&[transition(&resource, rest, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            list.CopyTextureRegion(&raw const dst, 0, 0, 0, &raw const src, Some(&raw const area));
            list.ResourceBarrier(&[transition(&resource, D3D12_RESOURCE_STATE_COPY_SOURCE, rest)]);
        }
        let serial = self.submit()?;
        Ok(self.readbacks.insert(Readback {
            resource: buffer,
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
        let Readback { resource, row_pitch, bytes_per_row, rows, .. } =
            self.readbacks.remove(readback).ok_or(GpuError::UnknownId("readback"))?;
        let mut data = std::ptr::null_mut();
        let range = D3D12_RANGE { Begin: 0, End: row_pitch * rows };
        // SAFETY: the copy into the buffer has finished, and the mapping
        // covers its rows.
        unsafe {
            resource.Map(0, Some(&raw const range), Some(&raw mut data)).map_err(invalid("mapping a readback"))?;
            let mapped = std::slice::from_raw_parts(data.cast::<u8>(), row_pitch * rows);
            let mut texels = Vec::with_capacity(bytes_per_row * rows);
            for row in mapped.chunks(row_pitch).take(rows) {
                texels.extend_from_slice(&row[..bytes_per_row]);
            }
            resource.Unmap(0, Some(&raw const NO_RANGE));
            Ok(Some(texels))
        }
    }
}

impl Drop for D3d12Gpu {
    /// Waits for the work submitted so far, which reads what the backend
    /// frees.
    fn drop(&mut self) {
        let _ = self.wait_for(self.serial - 1);
    }
}
