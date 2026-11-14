//! The [`Gpu`] interface on Metal.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::ops::Deref;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use block2::RcBlock;
use mln_gfx::backend::Type as BackendType;
use mln_gfx::color_mode::{BlendFunction, Mask};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::validation::{self, Resources, ShaderBinding};
use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferDesc, BufferId, BufferUsage, DepthState, Gpu, GpuError, Limits,
    LoadOp, Pass, PipelineId, PipelineKey, ReadbackId, RenderCommand, SamplerId, ShaderKey, SlotMap, StencilState,
    TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage, VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType, TextureFilterType, TextureWrapType,
};
use mln_util::size::Size;
use objc2::Message;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, ProtocolObject};
use objc2_foundation::{NSDictionary, NSString};
use objc2_metal::{
    MTLBinding, MTLBindingType, MTLBlendFactor, MTLBlendOperation, MTLBlitCommandEncoder, MTLBuffer, MTLBufferBinding,
    MTLClearColor, MTLColorWriteMask, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCompareFunction,
    MTLCompileOptions, MTLCullMode, MTLDepthStencilDescriptor, MTLDevice, MTLDrawable, MTLEvent, MTLGPUFamily,
    MTLIndexType, MTLLanguageVersion, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPipelineOption, MTLPixelFormat,
    MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineReflection, MTLResource, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerDescriptor,
    MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLScissorRect, MTLSize, MTLStencilDescriptor, MTLStencilOperation,
    MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage, MTLVertexDescriptor,
    MTLVertexFormat, MTLVertexStepFunction, MTLViewport, MTLWinding,
};
use parking_lot::{Condvar, Mutex};

use crate::Device;
use crate::custom_layer::{AttachmentFormats, CustomLayerEncoder, CustomLayerPass};
use crate::mtl_fwd::{
    MTLBufferPtr, MTLCommandBufferPtr, MTLDepthStencilStatePtr, MTLFunctionPtr, MTLRenderCommandEncoderPtr,
    MTLRenderPipelineStatePtr, MTLSamplerStatePtr, MTLTexturePtr, MTLVertexDescriptorPtr,
};
use crate::readback::{buffer_bytes, encode_region_copy};
use crate::shaders::msl_sources::PRELUDE;
use crate::shaders::shader_source::{ShaderSource, shader_source};

/// The entries of a stage's buffer argument table.
const MAX_BUFFERS: usize = 31;

/// The alignment Metal asks of a constant buffer's offset on a Mac with an
/// Intel or AMD GPU, which Apple GPUs' 4 bytes divide.
const BUFFER_OFFSET_ALIGNMENT: u32 = 256;

/// The formats that [`TextureFormat`]s map to, for an imported texture's.
const TEXTURE_FORMATS: [TextureFormat; 14] = [
    TextureFormat::R8Unorm,
    TextureFormat::A8Unorm,
    TextureFormat::R16Float,
    TextureFormat::R32Float,
    TextureFormat::Rgba8Unorm,
    TextureFormat::Rgba8UnormSrgb,
    TextureFormat::Bgra8Unorm,
    TextureFormat::Bgra8UnormSrgb,
    TextureFormat::Rgb10a2Unorm,
    TextureFormat::Rgba16Float,
    TextureFormat::Rgba32Float,
    TextureFormat::Depth32Float,
    TextureFormat::Depth24PlusStencil8,
    TextureFormat::Depth32FloatStencil8,
];

/// A buffer, and what the shared checks read of it.
struct Buffer {
    buffer: MTLBufferPtr,
    usage: BufferUsage,
    /// The size asked for, rounded up to a multiple of 4.
    size: u64,
}

impl Deref for Buffer {
    type Target = ProtocolObject<dyn MTLBuffer>;

    fn deref(&self) -> &ProtocolObject<dyn MTLBuffer> {
        &self.buffer
    }
}

struct Texture {
    texture: MTLTexturePtr,
    format: TextureFormat,
    /// The events that the next command buffer that draws into it, or reads
    /// it back, waits for: of [`wait_before_using`](MetalGpu::wait_before_using).
    waits: Vec<(Retained<ProtocolObject<dyn MTLEvent>>, u64)>,
}

/// A program's library functions.
struct Shader {
    source: &'static ShaderSource,
    vertex: MTLFunctionPtr,
    fragment: Option<MTLFunctionPtr>,
}

/// What a pipeline's shader reads from the bound bind groups.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resource {
    /// `globalIndex`, set with `setVertexBytes`.
    GlobalIndex,
    /// The buffer at a uniform or storage buffer id.
    Buffer(u32),
    /// The texture at a texture id.
    Texture(u32),
    /// The sampler of the texture at a texture id.
    Sampler(u32),
}

/// A resource that a pipeline's shader reads at argument table index
/// `index` of its vertex stage, its fragment stage or both.
struct Binding {
    resource: Resource,
    index: usize,
    vertex: bool,
    fragment: bool,
}

struct Pipeline {
    name: &'static str,
    state: MTLRenderPipelineStatePtr,
    depth_stencil: MTLDepthStencilStatePtr,
    primitive: MTLPrimitiveType,
    cull: MTLCullMode,
    winding: MTLWinding,
    targets: TargetFormats,
    vertex_buffers: Vec<VertexBufferLayout>,
    /// The buffer index of each vertex buffer slot, `None` for a slot with
    /// no attributes.
    vertex_buffer_indices: Vec<Option<usize>>,
    bindings: Vec<Binding>,
    /// What the shader reads from the bind groups, for the shared checks.
    reads: Vec<ShaderBinding>,
}

/// A texture's texels, copied into shared memory.
struct Readback {
    buffer: MTLBufferPtr,
    /// The bytes of the texels, at the start of `buffer`.
    len: usize,
    /// The number of the command buffer that copies them.
    number: u64,
}

/// A copy out of the staging buffer of a submit.
enum Upload {
    Buffer { from: usize, to: MTLBufferPtr, offset: usize, size: usize },
    Texture { from: usize, to: MTLTexturePtr, region: TextureRegion, bytes_per_row: usize },
}

/// The writes since the last submit, which the next one copies in before
/// its passes.
#[derive(Default)]
struct Uploads {
    data: Vec<u8>,
    copies: Vec<Upload>,
}

impl Uploads {
    /// Appends `data` at an offset that every texel size divides, and
    /// returns the offset.
    fn push(&mut self, data: &[u8]) -> usize {
        let from = self.data.len().next_multiple_of(16);
        self.data.resize(from, 0);
        self.data.extend_from_slice(data);
        from
    }

    /// Encodes the copies out of `staging`, which holds `data`.
    fn encode(&self, command_buffer: &ProtocolObject<dyn MTLCommandBuffer>, staging: &ProtocolObject<dyn MTLBuffer>) {
        let Some(blit) = command_buffer.blitCommandEncoder() else {
            return;
        };
        for upload in &self.copies {
            // SAFETY: the write checked each copy's range against its
            // destination, and `push` put its data at `from`.
            unsafe {
                match upload {
                    Upload::Buffer { from, to, offset, size } => {
                        blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(
                            staging, *from, to, *offset, *size,
                        );
                    }
                    Upload::Texture { from, to, region, bytes_per_row } => {
                        let (width, height) = (region.size.width as usize, region.size.height as usize);
                        blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                            staging,
                            *from,
                            *bytes_per_row,
                            bytes_per_row * height,
                            MTLSize { width, height, depth: 1 },
                            to,
                            0,
                            0,
                            MTLOrigin { x: region.x as usize, y: region.y as usize, z: 0 },
                        );
                    }
                }
            }
        }
        blit.endEncoding();
    }
}

/// Whether a committed command buffer has finished, which its completed
/// handler sets.
///
/// The handler, not `MTLCommandBuffer::status`, says when what the command
/// buffer uses may be freed: the status reads `Completed` before the
/// handlers run, and Metal's API validation counts the command buffer's
/// unretained references until it runs its own handler, so a resource
/// freed on the status alone can fail validation.
#[derive(Default)]
struct Completion {
    done: Mutex<bool>,
    changed: Condvar,
}

impl Completion {
    fn is_done(&self) -> bool {
        *self.done.lock()
    }

    /// Blocks until the completed handler has run.
    fn wait(&self) {
        let mut done = self.done.lock();
        self.changed.wait_while(&mut done, |done| !*done);
    }

    fn set(&self) {
        *self.done.lock() = true;
        self.changed.notify_all();
    }
}

/// A test's hook into each command buffer before it's committed.
#[cfg(test)]
type BeforeCommit = Box<dyn Fn(&ProtocolObject<dyn MTLCommandBuffer>)>;

/// A [`Gpu`] on a Metal [`Device`].
///
/// Command buffers don't retain the resources they use: a destroyed
/// resource waits in a queue until the command buffers submitted before it
/// was destroyed finish (see [`Completion`]). Writes go into a staging
/// buffer that the next submit copies in before its passes. Each call that
/// encodes commands or compiles a pipeline drains an autorelease pool of its
/// own.
pub struct MetalGpu {
    device: Device,
    /// The texture size limit and whether draws may offset their base
    /// vertex and instance, by the device's GPU family.
    max_texture_size: u32,
    base_vertex_instance: bool,
    buffers: SlotMap<BufferId, Buffer>,
    textures: SlotMap<TextureId, Texture>,
    samplers: SlotMap<SamplerId, MTLSamplerStatePtr>,
    shaders: HashMap<ShaderKey, Shader>,
    depth_stencil_states: HashMap<(DepthState, StencilState, Option<TextureFormat>), MTLDepthStencilStatePtr>,
    /// Pipelines by their key, which `create_pipeline` hands out again.
    pipeline_cache: HashMap<PipelineKey, Rc<Pipeline>>,
    pipelines: SlotMap<PipelineId, Rc<Pipeline>>,
    bind_groups: SlotMap<BindGroupId, Vec<BindGroupEntry>>,
    uploads: Uploads,
    /// The number of command buffers committed so far.
    submitted: u64,
    /// The committed command buffers whose completed handlers may not have
    /// run, by their number, oldest first.
    in_flight: VecDeque<(u64, Arc<Completion>)>,
    /// What the command buffers up to each number may use, freed once they
    /// finish.
    retired: VecDeque<(u64, Box<dyn Any>)>,
    readbacks: SlotMap<ReadbackId, Readback>,
    /// The events the next command buffer waits for, with their values:
    /// those of the textures it uses.
    waits: Vec<(Retained<ProtocolObject<dyn MTLEvent>>, u64)>,
    /// The events the next command buffer signals, with their values.
    signals: Vec<(Retained<ProtocolObject<dyn MTLEvent>>, u64)>,
    /// The drawables the next command buffer presents.
    presents: Vec<Retained<ProtocolObject<dyn MTLDrawable>>>,
    /// Called with each command buffer just before it's committed.
    #[cfg(test)]
    before_commit: Option<BeforeCommit>,
}

impl MetalGpu {
    #[must_use]
    pub fn new(device: &Device) -> MetalGpu {
        let mtl_device = device.device();
        let modern = mtl_device.supportsFamily(MTLGPUFamily::Apple3) || mtl_device.supportsFamily(MTLGPUFamily::Mac2);
        MetalGpu {
            device: device.clone(),
            max_texture_size: if modern { 16384 } else { 8192 },
            base_vertex_instance: modern,
            buffers: SlotMap::with_key(),
            textures: SlotMap::with_key(),
            samplers: SlotMap::with_key(),
            shaders: HashMap::new(),
            depth_stencil_states: HashMap::new(),
            pipeline_cache: HashMap::new(),
            pipelines: SlotMap::with_key(),
            bind_groups: SlotMap::with_key(),
            uploads: Uploads::default(),
            submitted: 0,
            in_flight: VecDeque::new(),
            retired: VecDeque::new(),
            readbacks: SlotMap::with_key(),
            waits: Vec::new(),
            signals: Vec::new(),
            presents: Vec::new(),
            #[cfg(test)]
            before_commit: None,
        }
    }

    /// Takes a texture made elsewhere, such as a drawable's or a host's, as
    /// a texture of this backend, of `format`, whose pixel format it has (a
    /// `Depth32Float_Stencil8` texture is of either depth and stencil
    /// format). Its usage is what its `MTLTextureUsage` allows. Destroying
    /// it drops this backend's reference.
    ///
    /// Fails with [`GpuError::Invalid`] for a texture of another pixel
    /// format.
    pub fn import_texture(
        &mut self,
        texture: &ProtocolObject<dyn MTLTexture>,
        format: TextureFormat,
    ) -> Result<TextureId, GpuError> {
        if texture.pixelFormat() != pixel_format(format) {
            return Err(GpuError::Invalid(format!("a texture of {:?} imported as {format:?}", texture.pixelFormat())));
        }
        Ok(self.textures.insert(Texture { texture: texture.retain(), format, waits: Vec::new() }))
    }

    /// Has the next submit, or lend to a custom layer, whose passes draw
    /// into `texture`, or the next readback of it, wait for `event` to reach
    /// `value` before its commands run. The submits before it don't wait.
    pub fn wait_before_using(
        &mut self,
        texture: TextureId,
        event: &ProtocolObject<dyn MTLEvent>,
        value: u64,
    ) -> Result<(), GpuError> {
        let texture = self.textures.get_mut(texture).ok_or(GpuError::UnknownId("texture"))?;
        texture.waits.push((event.retain(), value));
        Ok(())
    }

    /// Has the next command buffer wait for what `textures` wait for.
    fn take_waits(&mut self, textures: impl IntoIterator<Item = TextureId>) {
        for id in textures {
            if let Some(texture) = self.textures.get_mut(id) {
                self.waits.append(&mut texture.waits);
            }
        }
    }

    /// Has the next command buffer wait for what the attachments of
    /// `passes` wait for.
    fn take_pass_waits<'a>(&mut self, passes: impl IntoIterator<Item = &'a Pass>) {
        let attachments = passes
            .into_iter()
            .flat_map(|pass| [pass.color.map(|color| color.texture), pass.depth_stencil.map(|depth| depth.texture)]);
        self.take_waits(attachments.flatten());
    }

    /// Has the next submit, or the next lend to a custom layer, signal
    /// `event` with `value` once its commands are done. A submit without
    /// passes signals it too.
    pub fn signal_after_next_submit(&mut self, event: &ProtocolObject<dyn MTLEvent>, value: u64) {
        self.signals.push((event.retain(), value));
    }

    /// Has the next submit, or the next lend to a custom layer, present
    /// `drawable` once its commands are done.
    pub fn present_after_next_submit(&mut self, drawable: &ProtocolObject<dyn MTLDrawable>) {
        self.presents.push(drawable.retain());
    }

    /// Whether the next submit has events to wait for or signal, or
    /// drawables to present.
    fn has_sync(&self) -> bool {
        !(self.waits.is_empty() && self.signals.is_empty() && self.presents.is_empty())
    }

    /// Frees what the finished command buffers kept.
    fn retire(&mut self) {
        while self.in_flight.front().is_some_and(|(_, completion)| completion.is_done()) {
            self.in_flight.pop_front();
        }
        let finished = self.finished();
        while self.retired.front().is_some_and(|(number, _)| *number <= finished) {
            self.retired.pop_front();
        }
    }

    /// The number of the last command buffer known to have finished, as
    /// every one before it has (see [`Completion`]).
    fn finished(&self) -> u64 {
        self.in_flight.front().map_or(self.submitted, |(number, _)| number - 1)
    }

    /// Blocks until the command buffers up to number `number` finish.
    fn wait_for(&self, number: u64) {
        for (_, completion) in self.in_flight.iter().take_while(|(at, _)| *at <= number) {
            completion.wait();
        }
    }

    /// Keeps `resource` until the command buffers submitted so far finish.
    fn retire_later(&mut self, resource: impl Any) {
        self.retired.push_back((self.submitted, Box::new(resource)));
        self.retire();
    }

    /// Commits a command buffer of the copies of the writes made so far,
    /// then what `encode` encodes. Where `encode` fails, commits the copies
    /// alone, and returns its error. With `retained`, the command buffer
    /// keeps what its commands use alive, for commands that a custom layer
    /// records with objects of its own. Call it in an autorelease pool.
    fn commit(
        &mut self,
        retained: bool,
        encode: impl FnOnce(&mut MetalGpu, &ProtocolObject<dyn MTLCommandBuffer>) -> Result<(), GpuError>,
    ) -> Result<(), GpuError> {
        self.retire();
        let staging = if self.uploads.copies.is_empty() {
            None
        } else {
            let data = &self.uploads.data;
            let staging =
                new_metal_buffer(self.device.device(), Some(data), data.len(), MTLResourceOptions::StorageModeShared)
                    .ok_or_else(|| GpuError::Invalid("Metal made no staging buffer".to_owned()))?;
            Some(staging)
        };
        let mut command_buffer = self.command_buffer(retained)?;
        if let Some(staging) = &staging {
            self.uploads.encode(&command_buffer, staging);
        }
        let result = encode(self, &command_buffer);
        if result.is_err() {
            // The writes take effect without what `encode` encoded.
            let Some(staging) = &staging else {
                return result;
            };
            command_buffer = self.command_buffer(retained)?;
            self.uploads.encode(&command_buffer, staging);
        }
        for (event, value) in self.signals.drain(..) {
            command_buffer.encodeSignalEvent_value(&event, value);
        }
        for drawable in self.presents.drain(..) {
            command_buffer.presentDrawable(&drawable);
        }
        #[cfg(test)]
        if let Some(before_commit) = &self.before_commit {
            before_commit(&command_buffer);
        }
        let completion = Arc::new(Completion::default());
        let handler = RcBlock::new({
            let completion = Arc::clone(&completion);
            move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| completion.set()
        });
        // SAFETY: the block is valid, and Metal copies it.
        unsafe { command_buffer.addCompletedHandler(RcBlock::as_ptr(&handler)) };
        command_buffer.commit();
        self.waits.clear();
        self.submitted += 1;
        self.in_flight.push_back((self.submitted, completion));
        if let Some(staging) = staging {
            let uploads = std::mem::take(&mut self.uploads);
            self.retired.push_back((self.submitted, Box::new((uploads, staging))));
        }
        result
    }

    /// Commits a command buffer, after the writes made so far, that `encode`
    /// encodes native commands into, such as an overlay's. It keeps what its
    /// commands use alive.
    pub(crate) fn submit_native(
        &mut self,
        encode: impl FnOnce(&ProtocolObject<dyn MTLCommandBuffer>),
    ) -> Result<(), GpuError> {
        autoreleasepool(|_| {
            self.commit(true, |_, command_buffer| {
                encode(command_buffer);
                Ok(())
            })
        })
    }

    /// Drops what `texture` still waits for, which no submit takes now.
    pub(crate) fn forget_waits(&mut self, texture: TextureId) {
        if let Some(texture) = self.textures.get_mut(texture) {
            texture.waits.clear();
        }
    }

    /// Has the next submit wait for what `texture` still waits for
    /// ([`wait_before_using`](Self::wait_before_using)), as when nothing of
    /// a frame used it.
    pub(crate) fn take_waits_into_next_submit(&mut self, texture: TextureId) {
        self.take_waits([texture]);
    }

    fn buffer(&self, id: BufferId) -> Result<&ProtocolObject<dyn MTLBuffer>, GpuError> {
        self.buffers.get(id).map(|buffer| &**buffer).ok_or(GpuError::UnknownId("buffer"))
    }

    fn texture(&self, id: TextureId) -> Result<&Texture, GpuError> {
        self.textures.get(id).ok_or(GpuError::UnknownId("texture"))
    }

    /// A command buffer that waits for the events of
    /// [`wait_before_using`](Self::wait_before_using), and with
    /// `retained` keeps what its commands use alive.
    fn command_buffer(&self, retained: bool) -> Result<MTLCommandBufferPtr, GpuError> {
        let queue = self.device.queue();
        let command_buffer =
            if retained { queue.commandBuffer() } else { queue.commandBufferWithUnretainedReferences() };
        let command_buffer =
            command_buffer.ok_or_else(|| GpuError::Invalid("the command queue gave no command buffer".to_owned()))?;
        command_buffer.setLabel(Some(&NSString::from_str("Gpu::submit")));
        for (event, value) in &self.waits {
            command_buffer.encodeWaitForEvent_value(event, *value);
        }
        Ok(command_buffer)
    }

    /// The program of `key`, compiled on first use.
    fn shader(&mut self, key: &ShaderKey) -> Result<&Shader, GpuError> {
        if !self.shaders.contains_key(key) {
            let source = shader_source(key.program())
                .ok_or_else(|| GpuError::Unsupported(format!("no MSL for {:?}", key.program())))?;
            let (vertex, fragment) = compile(
                self.device.device(),
                source.name,
                &format!("{PRELUDE}{}{}", source.msl.prelude, source.msl.source),
                source.msl.vertex_main_function,
                source.msl.fragment_main_function,
                key.define_values(),
            )?;
            self.shaders.insert(key.clone(), Shader { source, vertex, fragment });
        }
        Ok(&self.shaders[key])
    }

    /// The depth-stencil state of `depth` and `stencil`, for attachments of
    /// `format`, made on first use. Metal rejects a depth or stencil test
    /// without the attachment, so a state for one without them tests
    /// nothing.
    fn depth_stencil_state(
        &mut self,
        depth: DepthState,
        stencil: StencilState,
        format: Option<TextureFormat>,
    ) -> Result<MTLDepthStencilStatePtr, GpuError> {
        if let Some(state) = self.depth_stencil_states.get(&(depth, stencil, format)) {
            return Ok(state.clone());
        }
        let descriptor = MTLDepthStencilDescriptor::new();
        if let Some(format) = format {
            descriptor.setDepthCompareFunction(map_depth_func(depth.compare));
            descriptor.setDepthWriteEnabled(depth.write);
            if format.has_stencil() {
                let face = MTLStencilDescriptor::new();
                face.setStencilCompareFunction(map_stencil_func(stencil.test.func()));
                face.setReadMask(stencil.test.masked().unwrap_or(u32::MAX));
                face.setWriteMask(stencil.write_mask);
                face.setStencilFailureOperation(map_operation(stencil.fail));
                face.setDepthFailureOperation(map_operation(stencil.depth_fail));
                face.setDepthStencilPassOperation(map_operation(stencil.pass));
                descriptor.setFrontFaceStencil(Some(&face));
                descriptor.setBackFaceStencil(Some(&face));
            }
        }
        let state = self
            .device
            .device()
            .newDepthStencilStateWithDescriptor(&descriptor)
            .ok_or_else(|| GpuError::Invalid("Metal made no depth-stencil state".to_owned()))?;
        self.depth_stencil_states.insert((depth, stencil, format), state.clone());
        Ok(state)
    }

    fn make_pipeline(&mut self, key: &PipelineKey) -> Result<Pipeline, GpuError> {
        let state = &key.state;
        let primitive = primitive_type(state.primitive)
            .ok_or_else(|| GpuError::Unsupported(format!("{:?} primitives", state.primitive)))?;
        let depth_stencil = self.depth_stencil_state(state.depth, state.stencil, key.targets.depth_stencil)?;
        let device = self.device.device().clone();
        let shader = self.shader(&key.shader)?;
        let name = shader.source.name;
        let (vertex_descriptor, vertex_buffer_indices) = vertex_descriptor(shader.source, &state.vertex_buffers)?;

        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setLabel(Some(&NSString::from_str(name)));
        descriptor.setVertexFunction(Some(&shader.vertex));
        descriptor.setFragmentFunction(shader.fragment.as_deref());
        descriptor.setVertexDescriptor(Some(&vertex_descriptor));
        // A pipeline without a color target leaves the attachment's format
        // invalid, and Metal discards what the fragment function writes.
        if let Some(format) = key.targets.color {
            set_color_target(&descriptor, pixel_format(format), state.blend, state.color_mask);
        }
        if let Some(format) = key.targets.depth_stencil {
            descriptor.setDepthAttachmentPixelFormat(pixel_format(format));
            if format.has_stencil() {
                descriptor.setStencilAttachmentPixelFormat(pixel_format(format));
            }
        }
        let mut reflection = None;
        let pipeline_state = device
            .newRenderPipelineStateWithDescriptor_options_reflection_error(
                &descriptor,
                MTLPipelineOption::BindingInfo,
                Some(&mut reflection),
            )
            .map_err(|error| GpuError::Invalid(format!("{name}: {}", error.localizedDescription())))?;
        let reflection =
            reflection.ok_or_else(|| GpuError::Invalid(format!("{name}: Metal gave no pipeline reflection")))?;
        let (bindings, reads) = shader_bindings(&reflection, shader.source, &vertex_buffer_indices)?;

        let CullFaceMode { enabled, side, winding } = state.cull;
        Ok(Pipeline {
            name,
            state: pipeline_state,
            depth_stencil,
            primitive,
            cull: if enabled { map_cull_mode(side) } else { MTLCullMode::None },
            winding: map_winding_mode(winding),
            targets: key.targets,
            vertex_buffers: state.vertex_buffers.clone(),
            vertex_buffer_indices,
            bindings,
            reads,
        })
    }

    /// Encodes `pass` into `command_buffer`.
    fn encode(&self, command_buffer: &ProtocolObject<dyn MTLCommandBuffer>, pass: &Pass) -> Result<(), GpuError> {
        let (encoder, size) = self.begin_pass(command_buffer, pass)?;
        let result = self.encode_commands(&encoder, &pass.label, &pass.commands, size);
        encoder.endEncoding();
        result
    }

    /// Begins `pass` in `command_buffer`, without its commands, and returns
    /// its encoder and its attachments' size.
    fn begin_pass(
        &self,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        pass: &Pass,
    ) -> Result<(MTLRenderCommandEncoderPtr, (usize, usize)), GpuError> {
        let descriptor = MTLRenderPassDescriptor::new();
        let mut size = None;
        if let Some(attachment) = pass.color {
            let texture = &self.texture(attachment.texture)?.texture;
            // SAFETY: index 0 is in the attachment array's bounds.
            let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
            color.setTexture(Some(texture));
            match attachment.load {
                LoadOp::Load => color.setLoadAction(MTLLoadAction::Load),
                LoadOp::Clear(value) => {
                    color.setLoadAction(MTLLoadAction::Clear);
                    color.setClearColor(MTLClearColor {
                        red: f64::from(value.r),
                        green: f64::from(value.g),
                        blue: f64::from(value.b),
                        alpha: f64::from(value.a),
                    });
                }
            }
            color.setStoreAction(MTLStoreAction::Store);
            size = Some((texture.width(), texture.height()));
        }
        if let Some(attachment) = pass.depth_stencil {
            let texture = &self.texture(attachment.texture)?.texture;
            let depth = descriptor.depthAttachment();
            depth.setTexture(Some(texture));
            match attachment.depth {
                LoadOp::Load => depth.setLoadAction(MTLLoadAction::Load),
                LoadOp::Clear(value) => {
                    depth.setLoadAction(MTLLoadAction::Clear);
                    depth.setClearDepth(f64::from(value));
                }
            }
            depth.setStoreAction(MTLStoreAction::Store);
            if has_stencil(texture.pixelFormat()) {
                let stencil = descriptor.stencilAttachment();
                stencil.setTexture(Some(texture));
                match attachment.stencil {
                    LoadOp::Load => stencil.setLoadAction(MTLLoadAction::Load),
                    LoadOp::Clear(value) => {
                        stencil.setLoadAction(MTLLoadAction::Clear);
                        stencil.setClearStencil(value);
                    }
                }
                stencil.setStoreAction(MTLStoreAction::Store);
            }
            size.get_or_insert((texture.width(), texture.height()));
        }
        let size = size.ok_or_else(|| GpuError::Invalid(format!("{}: a pass without attachments", pass.label)))?;
        let encoder = command_buffer
            .renderCommandEncoderWithDescriptor(&descriptor)
            .ok_or_else(|| GpuError::Invalid(format!("{}: Metal made no render encoder", pass.label)))?;
        encoder.setLabel(Some(&NSString::from_str(&pass.label)));
        Ok((encoder, size))
    }

    /// Encodes `commands`, which the shared validation passed, into an
    /// encoder whose attachments have `size`.
    fn encode_commands(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        label: &str,
        commands: &[RenderCommand],
        (width, height): (usize, usize),
    ) -> Result<(), GpuError> {
        let mut pipeline: Option<&Pipeline> = None;
        let mut bound: Vec<Option<BindGroupId>> = Vec::new();
        let mut vertex_buffers: Vec<Option<(&ProtocolObject<dyn MTLBuffer>, usize)>> = Vec::new();
        let mut index_buffer: Option<(&ProtocolObject<dyn MTLBuffer>, usize)> = None;
        let mut global_index = 0u32;
        // Whether the next draw binds the groups' resources, and the vertex
        // buffers.
        let mut groups_dirty = true;
        let mut vertex_buffers_dirty = true;
        for command in commands {
            match command {
                RenderCommand::SetPipeline(id) => {
                    let entry = self.pipelines.get(*id).ok_or(GpuError::UnknownId("pipeline"))?;
                    encoder.setRenderPipelineState(&entry.state);
                    encoder.setDepthStencilState(Some(&entry.depth_stencil));
                    encoder.setCullMode(entry.cull);
                    encoder.setFrontFacingWinding(entry.winding);
                    pipeline = Some(entry);
                    groups_dirty = true;
                    vertex_buffers_dirty = true;
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
                    let slot = *slot as usize;
                    if vertex_buffers.len() <= slot {
                        vertex_buffers.resize(slot + 1, None);
                    }
                    vertex_buffers[slot] = Some((self.buffer(*buffer)?, *offset as usize));
                    vertex_buffers_dirty = true;
                }
                RenderCommand::SetIndexBuffer { buffer, offset } => {
                    index_buffer = Some((self.buffer(*buffer)?, *offset as usize));
                }
                RenderCommand::SetViewport(viewport) => encoder.setViewport(MTLViewport {
                    originX: f64::from(viewport.x),
                    originY: f64::from(viewport.y),
                    width: f64::from(viewport.width),
                    height: f64::from(viewport.height),
                    znear: f64::from(viewport.min_depth),
                    zfar: f64::from(viewport.max_depth),
                }),
                RenderCommand::SetScissor(scissor) => {
                    // Metal rejects a scissor past the attachments.
                    let x = (scissor.x as usize).min(width);
                    let y = (scissor.y as usize).min(height);
                    encoder.setScissorRect(MTLScissorRect {
                        x,
                        y,
                        width: (scissor.width as usize).min(width - x),
                        height: (scissor.height as usize).min(height - y),
                    });
                }
                RenderCommand::SetStencilReference(reference) => encoder.setStencilReferenceValue(*reference),
                RenderCommand::SetBlendConstant(color) => {
                    encoder.setBlendColorRed_green_blue_alpha(color.r, color.g, color.b, color.a);
                }
                // Metal draws no wide lines.
                RenderCommand::SetLineWidth(_) => {}
                RenderCommand::Draw { .. } | RenderCommand::DrawIndexed { .. } => {
                    let pipeline =
                        pipeline.ok_or_else(|| GpuError::Invalid(format!("{label}: a draw before a pipeline")))?;
                    if groups_dirty {
                        self.bind_resources(encoder, pipeline, &bound, global_index)?;
                        groups_dirty = false;
                    }
                    if vertex_buffers_dirty {
                        bind_vertex_buffers(encoder, pipeline, &vertex_buffers)?;
                        vertex_buffers_dirty = false;
                    }
                    self.draw(encoder, pipeline, command, index_buffer)?;
                }
            }
        }
        Ok(())
    }

    /// Binds what `pipeline`'s shader reads from what `bound` binds, and
    /// `global_index`.
    fn bind_resources(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        pipeline: &Pipeline,
        bound: &[Option<BindGroupId>],
        global_index: u32,
    ) -> Result<(), GpuError> {
        // The highest slot that binds an id wins.
        let entries = || bound.iter().rev().flatten().filter_map(|id| self.bind_groups.get(*id)).flatten();
        let missing = |what: &str, id: u32| {
            GpuError::Invalid(format!("{} reads {what} {id}, which no bind group binds", pipeline.name))
        };
        for binding in &pipeline.bindings {
            let Binding { resource, index, vertex, fragment } = *binding;
            match resource {
                Resource::GlobalIndex => {
                    let bytes = NonNull::from(&global_index).cast();
                    let length = size_of::<u32>();
                    // SAFETY: `bytes` points at `length` bytes, which Metal
                    // copies.
                    unsafe {
                        if vertex {
                            encoder.setVertexBytes_length_atIndex(bytes, length, index);
                        }
                        if fragment {
                            encoder.setFragmentBytes_length_atIndex(bytes, length, index);
                        }
                    }
                }
                Resource::Buffer(id) => {
                    let (buffer, offset) = entries()
                        .find_map(|entry| match *entry {
                            BindGroupEntry::Buffer { id: entry_id, buffer, offset, .. } if entry_id == id => {
                                Some((buffer, offset as usize))
                            }
                            _ => None,
                        })
                        .ok_or_else(|| missing("buffer", id))?;
                    let buffer = self.buffer(buffer)?;
                    // SAFETY: the buffer is alive until the command buffer
                    // finishes (`retire_later`).
                    unsafe {
                        if vertex {
                            encoder.setVertexBuffer_offset_atIndex(Some(buffer), offset, index);
                        }
                        if fragment {
                            encoder.setFragmentBuffer_offset_atIndex(Some(buffer), offset, index);
                        }
                    }
                }
                Resource::Texture(id) | Resource::Sampler(id) => {
                    let (texture, sampler) = entries()
                        .find_map(|entry| match *entry {
                            BindGroupEntry::Texture { id: entry_id, texture, sampler } if entry_id == id => {
                                Some((texture, sampler))
                            }
                            _ => None,
                        })
                        .ok_or_else(|| missing("texture", id))?;
                    // SAFETY: as for buffers.
                    unsafe {
                        if resource == Resource::Texture(id) {
                            let texture = &*self.texture(texture)?.texture;
                            if vertex {
                                encoder.setVertexTexture_atIndex(Some(texture), index);
                            }
                            if fragment {
                                encoder.setFragmentTexture_atIndex(Some(texture), index);
                            }
                        } else {
                            let sampler = &**self.samplers.get(sampler).ok_or(GpuError::UnknownId("sampler"))?;
                            if vertex {
                                encoder.setVertexSamplerState_atIndex(Some(sampler), index);
                            }
                            if fragment {
                                encoder.setFragmentSamplerState_atIndex(Some(sampler), index);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Encodes the draw `command` of `pipeline`.
    fn draw(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        pipeline: &Pipeline,
        command: &RenderCommand,
        index_buffer: Option<(&ProtocolObject<dyn MTLBuffer>, usize)>,
    ) -> Result<(), GpuError> {
        let (first, count, instances, base_vertex) = match command {
            RenderCommand::Draw { vertices, instances } => (vertices.start, vertices.len(), instances, 0),
            RenderCommand::DrawIndexed { indices, base_vertex, instances } => {
                (indices.start, indices.len(), instances, *base_vertex)
            }
            _ => return Ok(()),
        };
        if count == 0 || instances.is_empty() {
            return Ok(());
        }
        let base_instance = instances.start as usize;
        if (base_vertex != 0 || base_instance != 0) && !self.base_vertex_instance {
            return Err(GpuError::Unsupported(format!("{}: a draw from a base vertex or instance", pipeline.name)));
        }
        let (first, instance_count) = (first as usize, instances.len());
        // SAFETY: Metal checks the draw against the bound buffers' lengths.
        unsafe {
            if let RenderCommand::Draw { .. } = command {
                encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                    pipeline.primitive,
                    first,
                    count,
                    instance_count,
                    base_instance,
                );
            } else {
                let (buffer, offset) = index_buffer.ok_or_else(|| {
                    GpuError::Invalid(format!("{}: an indexed draw without an index buffer", pipeline.name))
                })?;
                encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset_instanceCount_baseVertex_baseInstance(
                    pipeline.primitive,
                    count,
                    MTLIndexType::UInt16,
                    buffer,
                    offset + first * size_of::<u16>(),
                    instance_count,
                    base_vertex as isize,
                    base_instance,
                );
            }
        }
        Ok(())
    }
}

impl Drop for MetalGpu {
    fn drop(&mut self) {
        // The command buffers don't keep what they use alive.
        let _ = self.finish();
    }
}

/// Binds the vertex buffers that `pipeline` reads.
/// Copies `data` into a new buffer of `size` bytes, or allocates an empty
/// one when there is no data.
fn new_metal_buffer(
    device: &ProtocolObject<dyn MTLDevice>,
    data: Option<&[u8]>,
    size: usize,
    usage: MTLResourceOptions,
) -> Option<MTLBufferPtr> {
    match data.filter(|d| !d.is_empty() && size != 0) {
        Some(data) => {
            debug_assert!(data.len() >= size);
            // SAFETY: `data` holds at least `size` bytes, which Metal copies.
            unsafe {
                device.newBufferWithBytes_length_options(
                    NonNull::new_unchecked(data.as_ptr().cast_mut().cast::<std::ffi::c_void>()),
                    size,
                    usage,
                )
            }
        }
        None => device.newBufferWithLength_options(size.max(1), usage),
    }
}

fn bind_vertex_buffers(
    encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    pipeline: &Pipeline,
    vertex_buffers: &[Option<(&ProtocolObject<dyn MTLBuffer>, usize)>],
) -> Result<(), GpuError> {
    for (slot, index) in pipeline.vertex_buffer_indices.iter().enumerate() {
        let Some(index) = *index else { continue };
        let (buffer, offset) = vertex_buffers.get(slot).copied().flatten().ok_or_else(|| {
            GpuError::Invalid(format!("{} reads vertex buffer {slot}, which no command set", pipeline.name))
        })?;
        // SAFETY: the buffer is alive until the command buffer finishes
        // (`retire_later`).
        unsafe { encoder.setVertexBuffer_offset_atIndex(Some(buffer), offset, index) };
    }
    Ok(())
}

/// The vertex descriptor of `layouts` for `source`'s attributes, and the
/// buffer index each layout's slot binds at, `None` for a slot without
/// attributes.
///
/// The slots bind in order from the lowest buffer index of `source`'s
/// attribute tables, which is past the indexes of the shader's own
/// buffers. Which attributes a slot holds is the layouts' choice, not the
/// tables' grouping of them.
fn vertex_descriptor(
    source: &ShaderSource,
    layouts: &[VertexBufferLayout],
) -> Result<(MTLVertexDescriptorPtr, Vec<Option<usize>>), GpuError> {
    let descriptor = MTLVertexDescriptor::vertexDescriptor();
    let first = first_vertex_buffer(source);
    let mut buffer_indices: Vec<Option<usize>> = Vec::with_capacity(layouts.len());
    for (slot, layout) in layouts.iter().enumerate() {
        if layout.attributes.is_empty() {
            buffer_indices.push(None);
            continue;
        }
        let infos = match layout.step {
            VertexStep::Vertex => source.attributes,
            VertexStep::Instance => source.instance_attributes,
        };
        let buffer_index = first.map(|first| first + slot).filter(|&index| index < MAX_BUFFERS).ok_or_else(|| {
            GpuError::Unsupported(format!("{}: vertex buffer slot {slot} past Metal's buffer indexes", source.name))
        })?;
        for attribute in &layout.attributes {
            let info = infos.iter().find(|info| info.id == attribute.id as usize).ok_or_else(|| {
                GpuError::Invalid(format!("{} has no {:?} attribute {}", source.name, layout.step, attribute.id))
            })?;
            let format = vertex_format(attribute.format)
                .ok_or_else(|| GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format)))?;
            // SAFETY: the shaders' attribute indexes are below Metal's 31,
            // and the buffer index below MAX_BUFFERS.
            unsafe {
                let descriptor = descriptor.attributes().objectAtIndexedSubscript(info.index);
                descriptor.setFormat(format);
                descriptor.setOffset(attribute.offset as usize);
                descriptor.setBufferIndex(buffer_index);
            }
        }
        // SAFETY: as above.
        unsafe {
            let descriptor = descriptor.layouts().objectAtIndexedSubscript(buffer_index);
            descriptor.setStride(layout.stride as usize);
            descriptor.setStepFunction(match layout.step {
                VertexStep::Vertex => MTLVertexStepFunction::PerVertex,
                VertexStep::Instance => MTLVertexStepFunction::PerInstance,
            });
            descriptor.setStepRate(1);
        }
        buffer_indices.push(Some(buffer_index));
    }
    Ok((descriptor, buffer_indices))
}

/// The lowest buffer index of `source`'s attribute tables, past the indexes
/// of the shader's own buffers, where the vertex buffers bind from.
fn first_vertex_buffer(source: &ShaderSource) -> Option<usize> {
    source.attributes.iter().chain(source.instance_attributes).map(|info| info.buffer_index).min()
}

/// What the pipeline of `reflection` reads, besides its vertex buffers at
/// `vertex_buffer_indices`: where in its argument tables, and by the
/// shaders' ids, as the shared checks read it.
///
/// Fails for a vertex function that reads a buffer from the indexes the
/// vertex buffers bind at other than through its attributes: which slot's
/// buffer it would read depends on the layouts, and no check bounds it.
fn shader_bindings(
    reflection: &MTLRenderPipelineReflection,
    source: &ShaderSource,
    vertex_buffer_indices: &[Option<usize>],
) -> Result<(Vec<Binding>, Vec<ShaderBinding>), GpuError> {
    let first_vertex_buffer = first_vertex_buffer(source);
    let mut bindings: Vec<Binding> = Vec::new();
    let mut reads: Vec<ShaderBinding> = Vec::new();
    for (vertex, stage) in [(true, reflection.vertexBindings()), (false, reflection.fragmentBindings())] {
        for binding in stage.to_vec() {
            if !binding.isUsed() {
                continue;
            }
            let index = binding.index();
            let texture_id = || {
                source
                    .textures
                    .iter()
                    .find(|texture| texture.index == index)
                    .map(|texture| texture.id as u32)
                    .ok_or_else(|| {
                        GpuError::Invalid(format!("{} reads texture {index}, which has no texture id", source.name))
                    })
            };
            let resource = match binding.r#type() {
                // An argument of the vertex function, not its `[[stage_in]]`.
                MTLBindingType::Buffer
                    if vertex && binding.isArgument() && first_vertex_buffer.is_some_and(|first| index >= first) =>
                {
                    return Err(GpuError::Invalid(format!(
                        "{} reads buffer {index}, where the vertex buffers bind, other than as attributes",
                        source.name
                    )));
                }
                MTLBindingType::Buffer if vertex && vertex_buffer_indices.contains(&Some(index)) => continue,
                MTLBindingType::Buffer if index == ID_GLOBAL_UBO_INDEX => Resource::GlobalIndex,
                MTLBindingType::Buffer => Resource::Buffer(index as u32),
                MTLBindingType::Texture => Resource::Texture(texture_id()?),
                MTLBindingType::Sampler => Resource::Sampler(texture_id()?),
                _ => continue,
            };
            let read = match resource {
                Resource::GlobalIndex => None,
                Resource::Buffer(id) => {
                    // SAFETY: Metal reflects a buffer binding as an
                    // MTLBufferBinding, which ProtocolObject wraps alike.
                    let buffer =
                        unsafe { &*std::ptr::from_ref(&*binding).cast::<ProtocolObject<dyn MTLBufferBinding>>() };
                    Some(ShaderBinding::Buffer { id, min_size: buffer.bufferDataSize() as u64 })
                }
                Resource::Texture(id) | Resource::Sampler(id) => Some(ShaderBinding::Texture { id }),
            };
            if let Some(read) = read {
                merge_read(&mut reads, read);
            }
            match bindings.iter_mut().find(|binding| binding.resource == resource && binding.index == index) {
                Some(binding) if vertex => binding.vertex = true,
                Some(binding) => binding.fragment = true,
                None => bindings.push(Binding { resource, index, vertex, fragment: !vertex }),
            }
        }
    }
    Ok((bindings, reads))
}

/// Adds `read` to `reads`: of a buffer that a shader's two stages read, as
/// many bytes as either reads.
fn merge_read(reads: &mut Vec<ShaderBinding>, read: ShaderBinding) {
    for earlier in reads.iter_mut() {
        match (earlier, read) {
            (ShaderBinding::Buffer { id, min_size }, ShaderBinding::Buffer { id: read_id, min_size: read_size })
                if *id == read_id =>
            {
                *min_size = (*min_size).max(read_size);
                return;
            }
            (earlier, read) if *earlier == read => return,
            _ => {}
        }
    }
    reads.push(read);
}

/// Compiles `source`, with `defines` as preprocessor macros, and finds its
/// `vertex` function and its `fragment` one, if named.
pub(crate) fn compile<'a>(
    device: &ProtocolObject<dyn MTLDevice>,
    name: &str,
    source: &str,
    vertex: &str,
    fragment: &str,
    defines: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(MTLFunctionPtr, Option<MTLFunctionPtr>), GpuError> {
    let (keys, values): (Vec<_>, Vec<_>) = defines
        .into_iter()
        .map(|(key, value)| (NSString::from_str(key), Retained::into_super(NSString::from_str(value))))
        .unzip();
    let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let defines: Retained<NSDictionary<NSString, NSObject>> = NSDictionary::from_retained_objects(&keys, &values);
    let options = MTLCompileOptions::new();
    // SAFETY: the dictionary maps strings to strings, as Metal expects.
    unsafe { options.setPreprocessorMacros(Some(&defines)) };
    // Fast math is Metal's default.
    options.setLanguageVersion(MTLLanguageVersion::Version2_4);
    // Lets the position outputs marked [[invariant]] be computed the same
    // way in every shader.
    options.setPreserveInvariance(true);

    let library = device
        .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
        .map_err(|error| GpuError::Invalid(format!("{name}: {}", error.localizedDescription())))?;
    let function = |function: &str| {
        library
            .newFunctionWithName(&NSString::from_str(function))
            .ok_or_else(|| GpuError::Invalid(format!("{name} has no function {function}")))
    };
    let vertex = function(vertex)?;
    let fragment = if fragment.is_empty() { None } else { Some(function(fragment)?) };
    Ok((vertex, fragment))
}

/// Sets the color attachment of `descriptor` to `format`, blending with
/// `blend` (none for [`BlendFunction::Replace`]) and writing the channels of
/// `mask`.
pub(crate) fn set_color_target(
    descriptor: &MTLRenderPipelineDescriptor,
    format: MTLPixelFormat,
    blend: BlendFunction,
    mask: Mask,
) {
    // SAFETY: index 0 is in the attachment array's bounds.
    let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    let blending = blend != BlendFunction::Replace;
    let (operation, src_factor, dst_factor) = if blending {
        (blend_operation(blend.equation()), blend_factor(blend.src_factor()), blend_factor(blend.dst_factor()))
    } else {
        (MTLBlendOperation::Add, MTLBlendFactor::One, MTLBlendFactor::One)
    };
    color.setPixelFormat(format);
    color.setBlendingEnabled(blending);
    color.setRgbBlendOperation(operation);
    color.setAlphaBlendOperation(operation);
    color.setSourceRGBBlendFactor(src_factor);
    color.setSourceAlphaBlendFactor(src_factor);
    color.setDestinationRGBBlendFactor(dst_factor);
    color.setDestinationAlphaBlendFactor(dst_factor);
    let mut write_mask = MTLColorWriteMask::None;
    for (enabled, bit) in [
        (mask.r, MTLColorWriteMask::Red),
        (mask.g, MTLColorWriteMask::Green),
        (mask.b, MTLColorWriteMask::Blue),
        (mask.a, MTLColorWriteMask::Alpha),
    ] {
        if enabled {
            write_mask |= bit;
        }
    }
    color.setWriteMask(write_mask);
}

fn blend_operation(equation: ColorBlendEquationType) -> MTLBlendOperation {
    match equation {
        ColorBlendEquationType::Add => MTLBlendOperation::Add,
        ColorBlendEquationType::Subtract => MTLBlendOperation::Subtract,
        ColorBlendEquationType::ReverseSubtract => MTLBlendOperation::ReverseSubtract,
    }
}

fn blend_factor(factor: ColorBlendFactorType) -> MTLBlendFactor {
    match factor {
        ColorBlendFactorType::Zero => MTLBlendFactor::Zero,
        ColorBlendFactorType::One => MTLBlendFactor::One,
        ColorBlendFactorType::SrcColor => MTLBlendFactor::SourceColor,
        ColorBlendFactorType::OneMinusSrcColor => MTLBlendFactor::OneMinusSourceColor,
        ColorBlendFactorType::SrcAlpha => MTLBlendFactor::SourceAlpha,
        ColorBlendFactorType::OneMinusSrcAlpha => MTLBlendFactor::OneMinusSourceAlpha,
        ColorBlendFactorType::DstAlpha => MTLBlendFactor::DestinationAlpha,
        ColorBlendFactorType::OneMinusDstAlpha => MTLBlendFactor::OneMinusDestinationAlpha,
        ColorBlendFactorType::DstColor => MTLBlendFactor::DestinationColor,
        ColorBlendFactorType::OneMinusDstColor => MTLBlendFactor::OneMinusDestinationColor,
        ColorBlendFactorType::SrcAlphaSaturate => MTLBlendFactor::SourceAlphaSaturated,
        ColorBlendFactorType::ConstantColor => MTLBlendFactor::BlendColor,
        ColorBlendFactorType::OneMinusConstantColor => MTLBlendFactor::OneMinusBlendColor,
        ColorBlendFactorType::ConstantAlpha => MTLBlendFactor::BlendAlpha,
        ColorBlendFactorType::OneMinusConstantAlpha => MTLBlendFactor::OneMinusBlendAlpha,
    }
}

pub(crate) fn map_depth_func(func: DepthFunctionType) -> MTLCompareFunction {
    match func {
        DepthFunctionType::Never => MTLCompareFunction::Never,
        DepthFunctionType::Less => MTLCompareFunction::Less,
        DepthFunctionType::Equal => MTLCompareFunction::Equal,
        DepthFunctionType::LessEqual => MTLCompareFunction::LessEqual,
        DepthFunctionType::Greater => MTLCompareFunction::Greater,
        DepthFunctionType::NotEqual => MTLCompareFunction::NotEqual,
        DepthFunctionType::GreaterEqual => MTLCompareFunction::GreaterEqual,
        DepthFunctionType::Always => MTLCompareFunction::Always,
    }
}

pub(crate) fn map_stencil_func(func: StencilFunctionType) -> MTLCompareFunction {
    match func {
        StencilFunctionType::Never => MTLCompareFunction::Never,
        StencilFunctionType::Less => MTLCompareFunction::Less,
        StencilFunctionType::Equal => MTLCompareFunction::Equal,
        StencilFunctionType::LessEqual => MTLCompareFunction::LessEqual,
        StencilFunctionType::Greater => MTLCompareFunction::Greater,
        StencilFunctionType::NotEqual => MTLCompareFunction::NotEqual,
        StencilFunctionType::GreaterEqual => MTLCompareFunction::GreaterEqual,
        StencilFunctionType::Always => MTLCompareFunction::Always,
    }
}

pub(crate) fn map_operation(op: StencilOpType) -> MTLStencilOperation {
    match op {
        StencilOpType::Zero => MTLStencilOperation::Zero,
        StencilOpType::Keep => MTLStencilOperation::Keep,
        StencilOpType::Replace => MTLStencilOperation::Replace,
        StencilOpType::Increment => MTLStencilOperation::IncrementClamp,
        StencilOpType::Decrement => MTLStencilOperation::DecrementClamp,
        StencilOpType::Invert => MTLStencilOperation::Invert,
        StencilOpType::IncrementWrap => MTLStencilOperation::IncrementWrap,
        StencilOpType::DecrementWrap => MTLStencilOperation::DecrementWrap,
    }
}

pub(crate) fn map_cull_mode(side: CullFaceSideType) -> MTLCullMode {
    match side {
        CullFaceSideType::Front => MTLCullMode::Front,
        CullFaceSideType::Back => MTLCullMode::Back,
        CullFaceSideType::FrontAndBack => MTLCullMode::None,
    }
}

pub(crate) fn map_winding_mode(winding: CullFaceWindingType) -> MTLWinding {
    match winding {
        CullFaceWindingType::Clockwise => MTLWinding::Clockwise,
        CullFaceWindingType::CounterClockwise => MTLWinding::CounterClockwise,
    }
}

/// The primitive of `mode`; `None` for the fans and loops Metal lacks.
pub(crate) fn primitive_type(mode: DrawModeType) -> Option<MTLPrimitiveType> {
    match mode {
        DrawModeType::Points => Some(MTLPrimitiveType::Point),
        DrawModeType::Lines => Some(MTLPrimitiveType::Line),
        DrawModeType::LineStrip => Some(MTLPrimitiveType::LineStrip),
        DrawModeType::Triangles => Some(MTLPrimitiveType::Triangle),
        DrawModeType::TriangleStrip => Some(MTLPrimitiveType::TriangleStrip),
        DrawModeType::LineLoop | DrawModeType::TriangleFan => None,
    }
}

pub(crate) fn vertex_format(type_: AttributeDataType) -> Option<MTLVertexFormat> {
    use AttributeDataType as A;
    use MTLVertexFormat as F;
    Some(match type_ {
        A::Byte => F::Char,
        A::Byte2 => F::Char2,
        A::Byte3 => F::Char3,
        A::Byte4 => F::Char4,
        A::UByte => F::UChar,
        A::UByte2 => F::UChar2,
        A::UByte3 => F::UChar3,
        A::UByte4 => F::UChar4,
        A::UByte4Normalized => F::UChar4Normalized,
        A::Short => F::Short,
        A::Short2 => F::Short2,
        A::Short3 => F::Short3,
        A::Short4 => F::Short4,
        A::UShort => F::UShort,
        A::UShort2 => F::UShort2,
        A::UShort3 => F::UShort3,
        A::UShort4 => F::UShort4,
        A::Int => F::Int,
        A::Int2 => F::Int2,
        A::Int3 => F::Int3,
        A::Int4 => F::Int4,
        A::UInt => F::UInt,
        A::UInt2 => F::UInt2,
        A::UInt3 => F::UInt3,
        A::UInt4 => F::UInt4,
        A::Float => F::Float,
        A::Float2 => F::Float2,
        A::Float3 => F::Float3,
        A::Float4 => F::Float4,
        _ => return None,
    })
}

/// The format of textures of `pixel_format`, `None` for a format the
/// interface has no name for.
pub(crate) fn texture_format(pixel_format: MTLPixelFormat) -> Option<TextureFormat> {
    TEXTURE_FORMATS.into_iter().find(|&format| self::pixel_format(format) == pixel_format)
}

/// A descriptor of a 2D texture of `size` pixels, neither of them zero. It's
/// made with `new`: the factory methods would leave it in the caller's
/// autorelease pool.
pub(crate) fn texture_descriptor(format: MTLPixelFormat, size: Size) -> Retained<MTLTextureDescriptor> {
    let descriptor = MTLTextureDescriptor::new();
    descriptor.setPixelFormat(format);
    // SAFETY: the size isn't zero, and a texture past the device's limits
    // fails to be made.
    unsafe {
        descriptor.setWidth(size.width as usize);
        descriptor.setHeight(size.height as usize);
    }
    descriptor
}

fn pixel_format(format: TextureFormat) -> MTLPixelFormat {
    match format {
        TextureFormat::R8Unorm => MTLPixelFormat::R8Unorm,
        TextureFormat::A8Unorm => MTLPixelFormat::A8Unorm,
        TextureFormat::R16Float => MTLPixelFormat::R16Float,
        TextureFormat::R32Float => MTLPixelFormat::R32Float,
        TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        TextureFormat::Rgba8UnormSrgb => MTLPixelFormat::RGBA8Unorm_sRGB,
        TextureFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        TextureFormat::Bgra8UnormSrgb => MTLPixelFormat::BGRA8Unorm_sRGB,
        TextureFormat::Rgb10a2Unorm => MTLPixelFormat::RGB10A2Unorm,
        TextureFormat::Rgba16Float => MTLPixelFormat::RGBA16Float,
        TextureFormat::Rgba32Float => MTLPixelFormat::RGBA32Float,
        TextureFormat::Depth32Float => MTLPixelFormat::Depth32Float,
        // Apple GPUs have no 24-bit depth format.
        TextureFormat::Depth24PlusStencil8 | TextureFormat::Depth32FloatStencil8 => {
            MTLPixelFormat::Depth32Float_Stencil8
        }
    }
}

fn has_stencil(format: MTLPixelFormat) -> bool {
    matches!(
        format,
        MTLPixelFormat::Depth32Float_Stencil8
            | MTLPixelFormat::Depth24Unorm_Stencil8
            | MTLPixelFormat::Stencil8
            | MTLPixelFormat::X32_Stencil8
            | MTLPixelFormat::X24_Stencil8
    )
}

impl Resources for MetalGpu {
    fn buffer_info(&self, id: BufferId) -> Option<validation::BufferInfo> {
        self.buffers.get(id).map(|buffer| validation::BufferInfo { usage: buffer.usage, size: buffer.size })
    }

    fn texture_info(&self, id: TextureId) -> Option<validation::TextureInfo> {
        let Texture { texture, format, .. } = self.textures.get(id)?;
        let usage = texture.usage();
        Some(validation::TextureInfo {
            format: Some(*format),
            size: Size { width: texture.width() as u32, height: texture.height() as u32 },
            usage: TextureUsage {
                sampled: usage.contains(MTLTextureUsage::ShaderRead),
                render_target: usage.contains(MTLTextureUsage::RenderTarget),
                // A blit copies out of any texture but a drawable's that
                // is only a framebuffer.
                copy_src: !texture.isFramebufferOnly(),
            },
        })
    }

    fn has_sampler(&self, id: SamplerId) -> bool {
        self.samplers.contains_key(id)
    }

    fn pipeline_info(&self, id: PipelineId) -> Option<validation::PipelineInfo<'_>> {
        let pipeline = self.pipelines.get(id)?;
        Some(validation::PipelineInfo {
            targets: pipeline.targets,
            vertex_buffers: &pipeline.vertex_buffers,
            bindings: &pipeline.reads,
        })
    }

    fn bind_group_entries(&self, id: BindGroupId) -> Option<&[BindGroupEntry]> {
        self.bind_groups.get(id).map(Vec::as_slice)
    }
}

impl Gpu for MetalGpu {
    fn backend_type(&self) -> BackendType {
        BackendType::Metal
    }

    fn limits(&self) -> Limits {
        Limits {
            buffer_offset_alignment: BUFFER_OFFSET_ALIGNMENT,
            max_texture_size: self.max_texture_size,
            half_float_render_targets: true,
        }
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        let size = desc.size.max(4).next_multiple_of(4) as usize;
        let buffer = self
            .device
            .device()
            .newBufferWithLength_options(size, MTLResourceOptions::StorageModePrivate)
            .ok_or_else(|| GpuError::Invalid(format!("{}: Metal made no buffer of {size} bytes", desc.label)))?;
        buffer.setLabel(Some(&NSString::from_str(desc.label)));
        Ok(self.buffers.insert(Buffer { buffer, usage: desc.usage, size: size as u64 }))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        let to = self.buffer(buffer)?;
        let (offset, size) = (offset as usize, data.len());
        if !offset.is_multiple_of(4) || !size.is_multiple_of(4) {
            return Err(GpuError::Invalid(format!("a write of {size} bytes at {offset}, not multiples of 4")));
        }
        if offset.checked_add(size).is_none_or(|end| end > to.length()) {
            return Err(GpuError::Invalid(format!(
                "a write of {size} bytes at {offset} to a buffer of {}",
                to.length()
            )));
        }
        if size > 0 {
            let to = to.retain();
            let from = self.uploads.push(data);
            self.uploads.copies.push(Upload::Buffer { from, to, offset, size });
        }
        Ok(())
    }

    fn destroy_buffer(&mut self, buffer: BufferId) {
        if let Some(buffer) = self.buffers.remove(buffer) {
            self.retire_later(buffer);
        }
    }

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
        let (width, height) = (desc.size.width, desc.size.height);
        let max = self.max_texture_size;
        if !(1..=max).contains(&width) || !(1..=max).contains(&height) {
            return Err(GpuError::Unsupported(format!("{}: a {width}x{height} texture", desc.label)));
        }
        let descriptor = texture_descriptor(pixel_format(desc.format), desc.size);
        let mut usage = MTLTextureUsage::empty();
        if desc.usage.sampled {
            usage |= MTLTextureUsage::ShaderRead;
        }
        if desc.usage.render_target {
            usage |= MTLTextureUsage::RenderTarget;
        }
        descriptor.setUsage(usage);
        descriptor.setStorageMode(MTLStorageMode::Private);
        let texture = self
            .device
            .device()
            .newTextureWithDescriptor(&descriptor)
            .ok_or_else(|| GpuError::Invalid(format!("{}: Metal made no texture", desc.label)))?;
        texture.setLabel(Some(&NSString::from_str(desc.label)));
        Ok(self.textures.insert(Texture { texture, format: desc.format, waits: Vec::new() }))
    }

    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        let Texture { texture, format, .. } = self.texture(texture)?;
        if !texture.usage().contains(MTLTextureUsage::ShaderRead) {
            return Err(GpuError::Invalid("a write to a texture that isn't sampled".to_owned()));
        }
        let format = *format;
        let size = Size { width: texture.width() as u32, height: texture.height() as u32 };
        let bytes_per_row = region.write_bytes_per_row(size, format, data)?;
        if !data.is_empty() {
            let to = texture.clone();
            let from = self.uploads.push(data);
            self.uploads.copies.push(Upload::Texture { from, to, region, bytes_per_row });
        }
        Ok(())
    }

    fn destroy_texture(&mut self, texture: TextureId) {
        if let Some(texture) = self.textures.remove(texture) {
            self.retire_later(texture.texture);
        }
    }

    fn create_sampler(&mut self, state: &SamplerState) -> Result<SamplerId, GpuError> {
        let descriptor = MTLSamplerDescriptor::new();
        let filter = match state.filter {
            TextureFilterType::Linear => MTLSamplerMinMagFilter::Linear,
            TextureFilterType::Nearest => MTLSamplerMinMagFilter::Nearest,
        };
        let address_mode = |wrap| match wrap {
            TextureWrapType::Repeat => MTLSamplerAddressMode::Repeat,
            TextureWrapType::Clamp => MTLSamplerAddressMode::ClampToEdge,
        };
        descriptor.setMinFilter(filter);
        descriptor.setMagFilter(filter);
        descriptor.setMipFilter(if state.mipmapped {
            MTLSamplerMipFilter::Linear
        } else {
            MTLSamplerMipFilter::NotMipmapped
        });
        descriptor.setSAddressMode(address_mode(state.wrap_u));
        descriptor.setTAddressMode(address_mode(state.wrap_v));
        descriptor.setMaxAnisotropy(usize::from(state.max_anisotropy.max(1)));
        let sampler = self
            .device
            .device()
            .newSamplerStateWithDescriptor(&descriptor)
            .ok_or_else(|| GpuError::Invalid("Metal made no sampler".to_owned()))?;
        Ok(self.samplers.insert(sampler))
    }

    fn destroy_sampler(&mut self, sampler: SamplerId) {
        if let Some(sampler) = self.samplers.remove(sampler) {
            self.retire_later(sampler);
        }
    }

    fn create_pipeline(&mut self, key: &PipelineKey) -> Result<PipelineId, GpuError> {
        let pipeline = if let Some(pipeline) = self.pipeline_cache.get(key) {
            Rc::clone(pipeline)
        } else {
            let pipeline = Rc::new(autoreleasepool(|_| self.make_pipeline(key))?);
            self.pipeline_cache.insert(key.clone(), Rc::clone(&pipeline));
            pipeline
        };
        Ok(self.pipelines.insert(pipeline))
    }

    fn destroy_pipeline(&mut self, pipeline: PipelineId) {
        // The cache keeps the pipeline's states.
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
        self.take_pass_waits(passes);
        if passes.is_empty() && self.uploads.copies.is_empty() && !self.has_sync() {
            autoreleasepool(|_| self.retire());
            return Ok(());
        }
        autoreleasepool(|_| {
            self.commit(false, |gpu, command_buffer| {
                passes.iter().try_for_each(|pass| gpu.encode(command_buffer, pass))
            })
        })
    }

    /// Blits into shared memory, in a command buffer of its own after the
    /// writes made so far.
    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError> {
        let readback = validation::check_readback(self, texture, region)?;
        self.take_waits([texture]);
        let texture = self.texture(texture)?.texture.clone();
        autoreleasepool(|_| {
            let buffer = self
                .device
                .device()
                .newBufferWithLength_options(readback.len(), MTLResourceOptions::StorageModeShared)
                .ok_or_else(|| GpuError::Invalid("Metal made no readback buffer".to_owned()))?;
            self.commit(false, |_, command_buffer| {
                if encode_region_copy(command_buffer, &texture, region, readback.bytes_per_row, &buffer) {
                    Ok(())
                } else {
                    Err(GpuError::Invalid("the command buffer gave no blit encoder".to_owned()))
                }
            })?;
            Ok(self.readbacks.insert(Readback { buffer, len: readback.len(), number: self.submitted }))
        })
    }

    fn take_readback(&mut self, readback: ReadbackId, wait: bool) -> Result<Option<Vec<u8>>, GpuError> {
        let number = self.readbacks.get(readback).ok_or(GpuError::UnknownId("readback"))?.number;
        if wait {
            self.wait_for(number);
        }
        self.retire();
        if number > self.finished() {
            return Ok(None);
        }
        let Readback { buffer, len, .. } = self.readbacks.remove(readback).ok_or(GpuError::UnknownId("readback"))?;
        Ok(Some(buffer_bytes(&buffer, len).to_vec()))
    }

    fn destroy_readback(&mut self, readback: ReadbackId) {
        if let Some(readback) = self.readbacks.remove(readback) {
            self.retire_later(readback.buffer);
        }
    }

    fn finish(&mut self) -> Result<(), GpuError> {
        self.wait_for(self.submitted);
        self.retire();
        Ok(())
    }

    /// Lends a command buffer of its own, after the writes made so far,
    /// which is committed after `f` returns, with a render encoder of its
    /// own for `pass`.
    fn lend_to_custom_layer(&mut self, pass: Option<&Pass>, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError> {
        if let Some(pass) = pass {
            validation::check_pass(self, pass)?;
            self.take_pass_waits([pass]);
        }
        autoreleasepool(|_| {
            self.commit(true, |gpu, command_buffer| {
                let Some(pass) = pass else {
                    f(&mut CustomLayerEncoder { command_buffer: command_buffer.retain() });
                    return Ok(());
                };
                let format = |attachment: Option<TextureId>| {
                    attachment.and_then(|id| gpu.textures.get(id)).map(|texture| texture.texture.pixelFormat())
                };
                let depth = format(pass.depth_stencil.map(|attachment| attachment.texture));
                let formats = AttachmentFormats {
                    color: format(pass.color.map(|attachment| attachment.texture)).unwrap_or(MTLPixelFormat::Invalid),
                    depth,
                    stencil: depth.filter(|&format| has_stencil(format)),
                };
                let (encoder, size) = gpu.begin_pass(command_buffer, pass)?;
                let result = gpu.encode_commands(&encoder, &pass.label, &pass.commands, size);
                if result.is_ok() {
                    f(&mut CustomLayerPass {
                        encoder: encoder.clone(),
                        command_buffer: command_buffer.retain(),
                        formats,
                        sample_count: 1,
                    });
                }
                encoder.endEncoding();
                result
            })
        })
    }
}

#[cfg(test)]
mod tests;
