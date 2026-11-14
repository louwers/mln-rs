//! The [`Gpu`] interface on Metal.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::ptr::NonNull;
use std::rc::Rc;

use mln_gfx::color_mode::{BlendFunction, Mask};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferDesc, BufferId, DepthState, Gpu, GpuError, Limits, LoadOp, Pass,
    PipelineId, PipelineKey, RenderCommand, SamplerId, ShaderKey, StencilState, TextureDesc, TextureFormat, TextureId,
    TextureRegion, VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType, TextureFilterType, TextureWrapType,
};
use objc2::Message;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, ProtocolObject};
use objc2_foundation::{NSDictionary, NSString};
use objc2_metal::{
    MTLBinding, MTLBindingType, MTLBlendFactor, MTLBlendOperation, MTLBlitCommandEncoder, MTLBuffer, MTLClearColor,
    MTLColorWriteMask, MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue,
    MTLCompareFunction, MTLCompileOptions, MTLCullMode, MTLDepthStencilDescriptor, MTLDevice, MTLGPUFamily,
    MTLIndexType, MTLLanguageVersion, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPipelineOption, MTLPixelFormat,
    MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineReflection, MTLResource, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerDescriptor,
    MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLScissorRect, MTLSize, MTLStencilDescriptor, MTLStencilOperation,
    MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage, MTLVertexDescriptor,
    MTLVertexFormat, MTLVertexStepFunction, MTLViewport, MTLWinding,
};
use slotmap::SlotMap;

use crate::Device;
use crate::buffer_resource::new_metal_buffer;
use crate::mtl_fwd::{
    MTLBufferPtr, MTLCommandBufferPtr, MTLDepthStencilStatePtr, MTLFunctionPtr, MTLRenderPipelineStatePtr,
    MTLSamplerStatePtr, MTLTexturePtr, MTLVertexDescriptorPtr,
};
use crate::shaders::msl_sources::PRELUDE;
use crate::shaders::shader_source::{ShaderSource, shader_source};

/// The alignment Metal asks of a constant buffer's offset on a Mac with an
/// Intel or AMD GPU, which Apple GPUs' 4 bytes divide.
const BUFFER_OFFSET_ALIGNMENT: u32 = 256;

/// The formats that [`TextureFormat`]s map to, for an imported texture's.
const TEXTURE_FORMATS: [TextureFormat; 13] = [
    TextureFormat::R8Unorm,
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

struct Texture {
    texture: MTLTexturePtr,
    /// `None` for an imported texture of a format with no [`TextureFormat`].
    format: Option<TextureFormat>,
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
    /// The pixel formats of the color and depth-stencil attachments the
    /// pipeline draws into.
    formats: (MTLPixelFormat, Option<MTLPixelFormat>),
    /// The buffer index of each vertex buffer slot, `None` for a slot with
    /// no attributes.
    vertex_buffer_indices: Vec<Option<usize>>,
    bindings: Vec<Binding>,
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

/// A [`Gpu`] on a Metal [`Device`].
///
/// Command buffers don't retain the resources they use: a destroyed
/// resource waits in a queue until the GPU finishes the command buffers
/// submitted before it was destroyed. Writes go into a staging buffer that
/// the next submit copies in before its passes. Each submit and pipeline
/// creation drains an autorelease pool of its own.
pub struct MetalGpu {
    device: Device,
    /// The texture size limit and whether draws may offset their base
    /// vertex and instance, by the device's GPU family.
    max_texture_size: u32,
    base_vertex_instance: bool,
    buffers: SlotMap<BufferId, MTLBufferPtr>,
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
    /// The committed command buffers the GPU may not have finished, by
    /// their number, oldest first.
    in_flight: VecDeque<(u64, MTLCommandBufferPtr)>,
    /// What the command buffers up to each number may use, freed once they
    /// finish.
    retired: VecDeque<(u64, Box<dyn Any>)>,
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
        }
    }

    /// Takes a texture made elsewhere, such as a drawable's, as a texture
    /// of this backend. Destroying it drops this backend's reference.
    pub fn import_texture(&mut self, texture: &ProtocolObject<dyn MTLTexture>) -> TextureId {
        let pixel_format = texture.pixelFormat();
        let format = TEXTURE_FORMATS.into_iter().find(|&format| self::pixel_format(format) == pixel_format);
        self.textures.insert(Texture { texture: texture.retain(), format })
    }

    /// Waits for the GPU to finish the work submitted so far, and frees what
    /// it kept for it.
    pub fn wait_until_completed(&mut self) {
        if let Some((_, command_buffer)) = self.in_flight.back() {
            command_buffer.waitUntilCompleted();
        }
        self.retire();
    }

    /// Frees what the finished command buffers kept.
    fn retire(&mut self) {
        while self.in_flight.front().is_some_and(|(_, command_buffer)| {
            matches!(command_buffer.status(), MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error)
        }) {
            self.in_flight.pop_front();
        }
        let finished = self.in_flight.front().map_or(self.submitted, |(number, _)| number - 1);
        while self.retired.front().is_some_and(|(number, _)| *number <= finished) {
            self.retired.pop_front();
        }
    }

    /// Keeps `resource` until the command buffers submitted so far finish.
    fn retire_later(&mut self, resource: impl Any) {
        self.retired.push_back((self.submitted, Box::new(resource)));
        self.retire();
    }

    fn buffer(&self, id: BufferId) -> Result<&ProtocolObject<dyn MTLBuffer>, GpuError> {
        self.buffers.get(id).map(|buffer| &**buffer).ok_or(GpuError::UnknownId("buffer"))
    }

    fn texture(&self, id: TextureId) -> Result<&Texture, GpuError> {
        self.textures.get(id).ok_or(GpuError::UnknownId("texture"))
    }

    fn command_buffer(&self) -> Result<MTLCommandBufferPtr, GpuError> {
        let command_buffer = self
            .device
            .queue()
            .commandBufferWithUnretainedReferences()
            .ok_or_else(|| GpuError::Invalid("the command queue gave no command buffer".to_owned()))?;
        command_buffer.setLabel(Some(&NSString::from_str("Gpu::submit")));
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
        let color_format = pixel_format(key.targets.color);
        set_color_target(&descriptor, color_format, state.blend, state.color_mask);
        let depth_stencil_format = key.targets.depth_stencil.map(pixel_format);
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
        let bindings = shader_bindings(&reflection, shader.source, &vertex_buffer_indices)?;

        let CullFaceMode { enabled, side, winding } = state.cull;
        Ok(Pipeline {
            name,
            state: pipeline_state,
            depth_stencil,
            primitive,
            cull: if enabled { map_cull_mode(side) } else { MTLCullMode::None },
            winding: map_winding_mode(winding),
            formats: (color_format, depth_stencil_format),
            vertex_buffer_indices,
            bindings,
        })
    }

    /// Encodes `pass` into `command_buffer`.
    fn encode(&self, command_buffer: &ProtocolObject<dyn MTLCommandBuffer>, pass: &Pass) -> Result<(), GpuError> {
        let descriptor = MTLRenderPassDescriptor::new();
        let mut size = None;
        let mut formats = (MTLPixelFormat::Invalid, None);
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
            formats.0 = texture.pixelFormat();
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
            formats.1 = Some(texture.pixelFormat());
        }
        let size = size.ok_or_else(|| GpuError::Invalid(format!("{}: a pass without attachments", pass.label)))?;
        let encoder = command_buffer
            .renderCommandEncoderWithDescriptor(&descriptor)
            .ok_or_else(|| GpuError::Invalid(format!("{}: Metal made no render encoder", pass.label)))?;
        encoder.setLabel(Some(&NSString::from_str(&pass.label)));
        let result = self.encode_commands(&encoder, pass, formats, size);
        encoder.endEncoding();
        result
    }

    /// Encodes the commands of `pass`, whose attachments have `formats` and
    /// `size`.
    fn encode_commands(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        pass: &Pass,
        formats: (MTLPixelFormat, Option<MTLPixelFormat>),
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
        for command in &pass.commands {
            match command {
                RenderCommand::SetPipeline(id) => {
                    let entry = self.pipelines.get(*id).ok_or(GpuError::UnknownId("pipeline"))?;
                    if entry.formats != formats {
                        return Err(GpuError::Invalid(format!(
                            "{}: a pipeline of {} for attachments {:?}, in a pass with {formats:?}",
                            pass.label, entry.name, entry.formats
                        )));
                    }
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
                    let pipeline = pipeline
                        .ok_or_else(|| GpuError::Invalid(format!("{}: a draw before a pipeline", pass.label)))?;
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
        self.wait_until_completed();
    }
}

/// Binds the vertex buffers that `pipeline` reads.
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
/// buffer index each layout's slot binds at.
fn vertex_descriptor(
    source: &ShaderSource,
    layouts: &[VertexBufferLayout],
) -> Result<(MTLVertexDescriptorPtr, Vec<Option<usize>>), GpuError> {
    let descriptor = MTLVertexDescriptor::vertexDescriptor();
    let mut buffer_indices: Vec<Option<usize>> = Vec::with_capacity(layouts.len());
    for layout in layouts {
        let infos = match layout.step {
            VertexStep::Vertex => source.attributes,
            VertexStep::Instance => source.instance_attributes,
        };
        let mut buffer_index = None;
        for attribute in &layout.attributes {
            let info = infos.iter().find(|info| info.id == attribute.id as usize).ok_or_else(|| {
                GpuError::Invalid(format!("{} has no {:?} attribute {}", source.name, layout.step, attribute.id))
            })?;
            if buffer_index.is_some_and(|index| index != info.buffer_index) {
                return Err(GpuError::Invalid(format!(
                    "{} reads the attributes of one vertex buffer from two",
                    source.name
                )));
            }
            buffer_index = Some(info.buffer_index);
            let format = vertex_format(attribute.format)
                .ok_or_else(|| GpuError::Unsupported(format!("vertex attribute format {:?}", attribute.format)))?;
            // SAFETY: the shaders' attribute and buffer indexes are below
            // Metal's 31.
            unsafe {
                let descriptor = descriptor.attributes().objectAtIndexedSubscript(info.index);
                descriptor.setFormat(format);
                descriptor.setOffset(attribute.offset as usize);
                descriptor.setBufferIndex(info.buffer_index);
            }
        }
        if let Some(index) = buffer_index {
            if buffer_indices.contains(&Some(index)) {
                return Err(GpuError::Invalid(format!("{} reads two vertex buffers as one", source.name)));
            }
            // SAFETY: as above.
            unsafe {
                let descriptor = descriptor.layouts().objectAtIndexedSubscript(index);
                descriptor.setStride(layout.stride as usize);
                descriptor.setStepFunction(match layout.step {
                    VertexStep::Vertex => MTLVertexStepFunction::PerVertex,
                    VertexStep::Instance => MTLVertexStepFunction::PerInstance,
                });
                descriptor.setStepRate(1);
            }
        }
        buffer_indices.push(buffer_index);
    }
    Ok((descriptor, buffer_indices))
}

/// What the pipeline of `reflection` reads, besides its vertex buffers at
/// `vertex_buffer_indices`.
fn shader_bindings(
    reflection: &MTLRenderPipelineReflection,
    source: &ShaderSource,
    vertex_buffer_indices: &[Option<usize>],
) -> Result<Vec<Binding>, GpuError> {
    let mut bindings: Vec<Binding> = Vec::new();
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
                MTLBindingType::Buffer if vertex && vertex_buffer_indices.contains(&Some(index)) => continue,
                MTLBindingType::Buffer if index == ID_GLOBAL_UBO_INDEX => Resource::GlobalIndex,
                MTLBindingType::Buffer => Resource::Buffer(index as u32),
                MTLBindingType::Texture => Resource::Texture(texture_id()?),
                MTLBindingType::Sampler => Resource::Sampler(texture_id()?),
                _ => continue,
            };
            match bindings.iter_mut().find(|binding| binding.resource == resource && binding.index == index) {
                Some(binding) if vertex => binding.vertex = true,
                Some(binding) => binding.fragment = true,
                None => bindings.push(Binding { resource, index, vertex, fragment: !vertex }),
            }
        }
    }
    Ok(bindings)
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

fn pixel_format(format: TextureFormat) -> MTLPixelFormat {
    match format {
        TextureFormat::R8Unorm => MTLPixelFormat::R8Unorm,
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

impl Gpu for MetalGpu {
    fn limits(&self) -> Limits {
        Limits { buffer_offset_alignment: BUFFER_OFFSET_ALIGNMENT, max_texture_size: self.max_texture_size }
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        let size = desc.size.max(4).next_multiple_of(4) as usize;
        let buffer = self
            .device
            .device()
            .newBufferWithLength_options(size, MTLResourceOptions::StorageModePrivate)
            .ok_or_else(|| GpuError::Invalid(format!("{}: Metal made no buffer of {size} bytes", desc.label)))?;
        buffer.setLabel(Some(&NSString::from_str(desc.label)));
        Ok(self.buffers.insert(buffer))
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
        // SAFETY: the size is in the device's limits.
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                pixel_format(desc.format),
                width as usize,
                height as usize,
                false,
            )
        };
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
        Ok(self.textures.insert(Texture { texture, format: Some(desc.format) }))
    }

    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        let Texture { texture, format } = self.texture(texture)?;
        let bytes_per_texel = format
            .and_then(TextureFormat::bytes_per_texel)
            .ok_or_else(|| GpuError::Invalid(format!("{:?} textures take no writes", texture.pixelFormat())))?;
        let (width, height) = (region.size.width as usize, region.size.height as usize);
        let bytes_per_row = width * bytes_per_texel as usize;
        if data.len() != bytes_per_row * height {
            return Err(GpuError::Invalid(format!(
                "a write of {} bytes to a region of {}",
                data.len(),
                bytes_per_row * height
            )));
        }
        if region.x as usize + width > texture.width() || region.y as usize + height > texture.height() {
            return Err(GpuError::Invalid(format!(
                "a write to {region:?} of a {}x{} texture",
                texture.width(),
                texture.height()
            )));
        }
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
        autoreleasepool(|_| {
            self.retire();
            if passes.is_empty() && self.uploads.copies.is_empty() {
                return Ok(());
            }
            let staging = if self.uploads.copies.is_empty() {
                None
            } else {
                let data = &self.uploads.data;
                let staging = new_metal_buffer(
                    self.device.device(),
                    Some(data),
                    data.len(),
                    MTLResourceOptions::StorageModeShared,
                )
                .ok_or_else(|| GpuError::Invalid("Metal made no staging buffer".to_owned()))?;
                Some(staging)
            };
            let mut command_buffer = self.command_buffer()?;
            if let Some(staging) = &staging {
                self.uploads.encode(&command_buffer, staging);
            }
            let result = passes.iter().try_for_each(|pass| self.encode(&command_buffer, pass));
            if result.is_err() {
                // The writes take effect without the passes.
                let Some(staging) = &staging else {
                    return result;
                };
                command_buffer = self.command_buffer()?;
                self.uploads.encode(&command_buffer, staging);
            }
            command_buffer.commit();
            self.submitted += 1;
            self.in_flight.push_back((self.submitted, command_buffer));
            if let Some(staging) = staging {
                let uploads = std::mem::take(&mut self.uploads);
                self.retired.push_back((self.submitted, Box::new((uploads, staging))));
            }
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use mln_gfx::gpu::{BufferUsage, ColorAttachment};
    use mln_util::Color;
    use mln_util::size::Size;

    use super::*;

    #[test]
    fn a_destroyed_buffer_waits_for_the_submits_before() {
        let device = Device::new(None).expect("Metal device");
        let mut gpu = MetalGpu::new(&device);
        let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage: BufferUsage::Uniform, size: 4 }).unwrap();
        gpu.write_buffer(buffer, 0, &[1, 2, 3, 4]).unwrap();
        let target = gpu
            .create_texture(&TextureDesc {
                label: "target",
                size: Size { width: 4, height: 4 },
                format: TextureFormat::Rgba8Unorm,
                usage: mln_gfx::gpu::TextureUsage { render_target: true, ..Default::default() },
            })
            .unwrap();
        let pass = Pass {
            label: "clear".to_owned(),
            color: Some(ColorAttachment {
                texture: target,
                load: LoadOp::Clear(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
            }),
            depth_stencil: None,
            commands: Vec::new(),
        };
        gpu.submit(&[pass]).unwrap();
        gpu.destroy_buffer(buffer);
        gpu.destroy_texture(target);
        if !gpu.in_flight.is_empty() {
            // The buffer, the texture and the write's staging buffer.
            assert_eq!(gpu.retired.len(), 3, "the GPU may still be using them");
        }
        gpu.wait_until_completed();
        assert!(gpu.retired.is_empty());
        assert!(gpu.in_flight.is_empty());
    }
}
