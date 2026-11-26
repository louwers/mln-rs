// The controls' overlay on D3D12: egui's meshes drawn over the map's frame,
// in its command list, as egui-wgpu draws them on WebGPU (overlay.rs): in
// gamma space, premultiplied, with egui-wgpu's blending, but no dithering.
//
// The textures and the frames' vertex and index buffers are the backend's
// `Gpu`'s, which destroys them once the frames that use them are done. The
// painter's own root signature, pipeline state and descriptor heap go through
// `D3d12Gpu::destroy_later` for the same reason, and so does a freed texture's
// descriptor on its way to the next texture.
//
// Each frame prepares what its draws read through the `Gpu` (`prepare`), and
// then records the draws into the pass over the frame's buffer (`draw`).

#![expect(unsafe_code, reason = "D3D12 through the windows crate is the FFI here")]

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use mln_d3d12::windows::Win32::Foundation::RECT;
use mln_d3d12::windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use mln_d3d12::windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_INV_DEST_ALPHA, D3D12_BLEND_INV_SRC_ALPHA,
    D3D12_BLEND_ONE, D3D12_BLEND_OP_ADD, D3D12_COLOR_WRITE_ENABLE_ALL, D3D12_COMPARISON_FUNC_NEVER,
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_CULL_MODE_NONE, D3D12_DEPTH_STENCIL_DESC, D3D12_DESCRIPTOR_HEAP_DESC,
    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_RANGE,
    D3D12_DESCRIPTOR_RANGE_TYPE_SRV, D3D12_FILL_MODE_SOLID, D3D12_FILTER, D3D12_FILTER_MIN_MAG_MIP_LINEAR,
    D3D12_FILTER_MIN_MAG_MIP_POINT, D3D12_GPU_DESCRIPTOR_HANDLE, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_INDEX_BUFFER_VIEW, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC,
    D3D12_INPUT_LAYOUT_DESC, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC,
    D3D12_RENDER_TARGET_BLEND_DESC, D3D12_ROOT_CONSTANTS, D3D12_ROOT_DESCRIPTOR_TABLE, D3D12_ROOT_PARAMETER,
    D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS, D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
    D3D12_ROOT_SIGNATURE_DESC, D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, D3D12_SHADER_BYTECODE,
    D3D12_SHADER_VISIBILITY_ALL, D3D12_SHADER_VISIBILITY_PIXEL, D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
    D3D12_STATIC_SAMPLER_DESC, D3D12_TEXTURE_ADDRESS_MODE, D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
    D3D12_TEXTURE_ADDRESS_MODE_WRAP, D3D12_VERTEX_BUFFER_VIEW, D3D12_VIEWPORT, D3D12SerializeRootSignature,
    ID3D12DescriptorHeap, ID3D12Device, ID3D12PipelineState, ID3D12RootSignature,
};
use mln_d3d12::windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use mln_d3d12::windows::core::{PCSTR, s};
use mln_d3d12::{CustomLayerPass, D3d12Gpu, Overlay, OverlayTarget, ShaderCompiler, ShaderStage};
use mln_gfx::Size;
use mln_gfx::gpu::{
    BufferDesc, BufferId, BufferUsage, Gpu as _, GpuError, ResourceId, TextureDesc, TextureFormat, TextureId,
    TextureRegion, TextureUsage, destroy_unused,
};
use mln_style::hash::HashMap;

use crate::overlay::Paint;

// `screen_size` is in points, as the vertices are. D3D's clip space has y up,
// and egui's points y down. Each texture's sampler is one of the four static
// ones, by its filter and wrap mode.
const SHADER: &str = r"
struct Constants {
    float2 screen_size;
    uint sampler_index;
};
ConstantBuffer<Constants> constants : register(b0);
Texture2D tex : register(t0);
SamplerState linear_clamp : register(s0);
SamplerState nearest_clamp : register(s1);
SamplerState linear_wrap : register(s2);
SamplerState nearest_wrap : register(s3);

struct Varyings {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
    float4 color : COLOR0;
};

Varyings vs(float2 pos : POSITION, float2 uv : TEXCOORD, float4 color : COLOR) {
    Varyings out_;
    float2 clip = 2.0 * pos / constants.screen_size - 1.0;
    out_.position = float4(clip.x, -clip.y, 0.0, 1.0);
    out_.uv = uv;
    out_.color = color;
    return out_;
}

float4 ps(Varyings in_) : SV_Target {
    float4 texel;
    switch (constants.sampler_index) {
        case 0: texel = tex.Sample(linear_clamp, in_.uv); break;
        case 1: texel = tex.Sample(nearest_clamp, in_.uv); break;
        case 2: texel = tex.Sample(linear_wrap, in_.uv); break;
        default: texel = tex.Sample(nearest_wrap, in_.uv); break;
    }
    return in_.color * texel;
}
";

/// The size of an `epaint::Vertex`: position and texture coordinates as f32
/// pairs, then the color as four bytes.
const VERTEX_STRIDE: u32 = 20;
/// The descriptors of the painter's heap: one for each of egui's textures.
const HEAP_SIZE: u32 = 64;
/// The root parameters: the constants, then the texture's table.
const CONSTANTS: u32 = 0;
const TEXTURE: u32 = 1;

/// One of egui's textures, with the descriptor that binds it and the static
/// sampler that samples it.
#[derive(Clone, Copy)]
struct Texture {
    texture: TextureId,
    slot: u32,
    sampler: u32,
}

/// A draw of a mesh: its clip rectangle in pixels, its texture, and its
/// vertices and indexes in the frame's buffers.
struct MeshDraw {
    scissor: RECT,
    slot: u32,
    sampler: u32,
    first_vertex: i32,
    first_index: u32,
    index_count: u32,
}

/// What a frame's draws read, which `prepare` made.
struct Frame {
    vertex_buffer: D3D12_VERTEX_BUFFER_VIEW,
    index_buffer: D3D12_INDEX_BUFFER_VIEW,
    screen_size: [f32; 2],
    draws: Vec<MeshDraw>,
}

struct Painter {
    root_signature: ID3D12RootSignature,
    /// The pipeline state, for the format it was made for.
    pipeline: ID3D12PipelineState,
    format: DXGI_FORMAT,
    heap: ID3D12DescriptorHeap,
    descriptor_size: u32,
    /// The heap's slots that no texture or frame uses.
    free_slots: Rc<RefCell<Vec<u32>>>,
    textures: HashMap<egui::TextureId, Texture>,
    /// The buffers of the last frame, which the next `prepare` destroys.
    buffers: Vec<BufferId>,
    frame: Option<Frame>,
}

/// The overlay that paints the controls on D3D12, with a painter made with
/// the first frame.
struct D3d12Overlay {
    paint: Rc<RefCell<Paint>>,
    painter: Option<Painter>,
    failed: bool,
}

pub(crate) fn d3d12_overlay(paint: Rc<RefCell<Paint>>) -> Box<dyn Overlay> {
    Box::new(D3d12Overlay { paint, painter: None, failed: false })
}

impl Overlay for D3d12Overlay {
    fn prepare(&mut self, target: &mut OverlayTarget<'_>) {
        if self.failed {
            return;
        }
        if self.painter.is_none() {
            match Painter::new(target.gpu.device().device(), target.format) {
                Ok(new) => self.painter = Some(new),
                Err(e) => {
                    log::error!("The controls can't be drawn: {e}");
                    self.failed = true;
                    return;
                }
            }
        }
        let Some(painter) = self.painter.as_mut() else { return };
        for buffer in painter.buffers.drain(..) {
            destroy_unused(target.gpu, [ResourceId::Buffer(buffer)]);
        }
        painter.frame = None;
        if let Err(e) = painter.set_format(target) {
            log::error!("The controls can't be drawn: {e}");
            self.failed = true;
            return;
        }
        let mut paint = self.paint.borrow_mut();
        let mut textures = std::mem::take(&mut paint.textures);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                if let Err(e) = painter.update_texture(target.gpu, *id, delta) {
                    log::error!("A texture of the controls can't be updated: {e}");
                }
            }
        }
        if let Err(e) = painter.prepare(target, &paint.primitives, paint.pixels_per_point) {
            log::error!("The controls can't be drawn: {e}");
            self.failed = true;
        }
        for id in &textures.free {
            if let Some(texture) = painter.textures.remove(id) {
                painter.free_texture(target.gpu, texture);
            }
        }
        textures.clear();
    }

    fn draw(&mut self, pass: &CustomLayerPass, size: Size) {
        if let Some(painter) = self.painter.as_mut() {
            painter.draw(pass, size);
        }
    }

    fn destroy(&mut self, gpu: &mut D3d12Gpu) {
        if let Some(painter) = self.painter.take() {
            painter.destroy(gpu);
        }
    }
}

/// The name of the GPU that `device` renders on, for the controls' status
/// line.
pub(crate) fn device_name(device: &mln_d3d12::Device) -> String {
    device.adapter_name().to_owned()
}

/// A static sampler of `register`, with `filter` and `address` on every
/// axis.
fn sampler(register: u32, filter: D3D12_FILTER, address: D3D12_TEXTURE_ADDRESS_MODE) -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: filter,
        AddressU: address,
        AddressV: address,
        AddressW: address,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
        ShaderRegister: register,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    }
}

/// The root signature: the constants (`b0`, three values), the texture's
/// table (`t0`), and the four static samplers (`s0` to `s3`).
fn root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, String> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 3 },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &raw const range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
    ];
    let samplers = [
        sampler(0, D3D12_FILTER_MIN_MAG_MIP_LINEAR, D3D12_TEXTURE_ADDRESS_MODE_CLAMP),
        sampler(1, D3D12_FILTER_MIN_MAG_MIP_POINT, D3D12_TEXTURE_ADDRESS_MODE_CLAMP),
        sampler(2, D3D12_FILTER_MIN_MAG_MIP_LINEAR, D3D12_TEXTURE_ADDRESS_MODE_WRAP),
        sampler(3, D3D12_FILTER_MIN_MAG_MIP_POINT, D3D12_TEXTURE_ADDRESS_MODE_WRAP),
    ];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: samplers.len() as u32,
        pStaticSamplers: samplers.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };
    let mut blob: Option<ID3DBlob> = None;
    // SAFETY: the description and what it points at outlive the calls, and
    // the blob's buffer is valid while it is held.
    unsafe {
        D3D12SerializeRootSignature(&raw const desc, D3D_ROOT_SIGNATURE_VERSION_1, &raw mut blob, None)
            .map_err(|e| format!("root signature: {e}"))?;
        let blob = blob.ok_or("root signature: no blob")?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        device.CreateRootSignature(0, bytes).map_err(|e| format!("root signature: {e}"))
    }
}

/// The pipeline state of the painter's shaders for `format`.
fn pipeline(
    device: &ID3D12Device,
    root_signature: &ID3D12RootSignature,
    format: DXGI_FORMAT,
) -> Result<ID3D12PipelineState, String> {
    let compiler = ShaderCompiler::new().map_err(|e| e.to_string())?;
    let vertex = compiler.compile_hlsl(ShaderStage::Vertex, SHADER, "vs").map_err(|e| e.to_string())?;
    let fragment = compiler.compile_hlsl(ShaderStage::Fragment, SHADER, "ps").map_err(|e| e.to_string())?;
    let element = |name: PCSTR, format, offset| D3D12_INPUT_ELEMENT_DESC {
        SemanticName: name,
        SemanticIndex: 0,
        Format: format,
        InputSlot: 0,
        AlignedByteOffset: offset,
        InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
        InstanceDataStepRate: 0,
    };
    let elements = [
        element(s!("POSITION"), DXGI_FORMAT_R32G32_FLOAT, 0),
        element(s!("TEXCOORD"), DXGI_FORMAT_R32G32_FLOAT, 8),
        element(s!("COLOR"), DXGI_FORMAT_R8G8B8A8_UNORM, 16),
    ];
    // egui-wgpu's blending, of premultiplied colors.
    let mut blend = D3D12_BLEND_DESC::default();
    blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_INV_DEST_ALPHA,
        DestBlendAlpha: D3D12_BLEND_ONE,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
        ..Default::default()
    };
    let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
    formats[0] = format;
    let bytecode =
        |code: &[u8]| D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() };
    let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root_signature.clone())),
        VS: bytecode(&vertex),
        PS: bytecode(&fragment),
        BlendState: blend,
        SampleMask: u32::MAX,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            DepthClipEnable: true.into(),
            ..Default::default()
        },
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC::default(),
        InputLayout: D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: elements.as_ptr(),
            NumElements: elements.len() as u32,
        },
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        RTVFormats: formats,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    // SAFETY: the description, its bytecode and its elements outlive the
    // call.
    let pipeline = unsafe { device.CreateGraphicsPipelineState(&raw const desc) };
    drop(ManuallyDrop::into_inner(desc.pRootSignature));
    pipeline.map_err(|e| format!("pipeline state: {e}"))
}

impl Painter {
    fn new(device: &ID3D12Device, format: DXGI_FORMAT) -> Result<Painter, String> {
        let root_signature = root_signature(device)?;
        let pipeline = pipeline(device, &root_signature, format)?;
        let desc = D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            NumDescriptors: HEAP_SIZE,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        };
        // SAFETY: plain calls of the live device.
        let (heap, descriptor_size) = unsafe {
            let heap: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&raw const desc).map_err(|e| format!("descriptor heap: {e}"))?;
            (heap, device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV))
        };
        Ok(Painter {
            root_signature,
            pipeline,
            format,
            heap,
            descriptor_size,
            free_slots: Rc::new(RefCell::new((0..HEAP_SIZE).rev().collect())),
            textures: HashMap::default(),
            buffers: Vec::new(),
            frame: None,
        })
    }

    /// Makes the pipeline state for the target's format, unless it is the
    /// one it has.
    fn set_format(&mut self, target: &mut OverlayTarget<'_>) -> Result<(), String> {
        if self.format == target.format {
            return Ok(());
        }
        let device = target.gpu.device().device().clone();
        let new = pipeline(&device, &self.root_signature, target.format)?;
        let old = std::mem::replace(&mut self.pipeline, new);
        target.gpu.destroy_later(Box::new(old));
        self.format = target.format;
        Ok(())
    }

    fn cpu_slot(&self, slot: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        // SAFETY: a plain query.
        let start = unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + (slot * self.descriptor_size) as usize }
    }

    fn gpu_slot(&self, slot: u32) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        // SAFETY: a plain query.
        let start = unsafe { self.heap.GetGPUDescriptorHandleForHeapStart() };
        D3D12_GPU_DESCRIPTOR_HANDLE { ptr: start.ptr + u64::from(slot * self.descriptor_size) }
    }

    /// Frees a texture: its descriptor is another texture's to take once the
    /// frames that draw with it are done, when the `Gpu` destroys the
    /// texture too.
    fn free_texture(&self, gpu: &mut D3d12Gpu, texture: Texture) {
        destroy_unused(gpu, [ResourceId::Texture(texture.texture)]);
        gpu.destroy_later(Box::new(FreeSlot { slot: texture.slot, free: Rc::clone(&self.free_slots) }));
    }

    /// Applies one of egui's texture updates: a whole new texture, or a
    /// region of an existing one.
    fn update_texture(
        &mut self,
        gpu: &mut D3d12Gpu,
        id: egui::TextureId,
        delta: &egui::epaint::ImageDelta,
    ) -> Result<(), GpuError> {
        let egui::ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        if width == 0 || height == 0 {
            return Ok(());
        }
        let pixels: Vec<u8> = image.pixels.iter().flat_map(egui::Color32::to_array).collect();
        let size = Size::new(width as u32, height as u32);
        if let Some([x, y]) = delta.pos {
            if let Some(texture) = self.textures.get(&id) {
                gpu.write_texture(texture.texture, TextureRegion { x: x as u32, y: y as u32, size }, &pixels)?;
            }
            return Ok(());
        }
        if let Some(old) = self.textures.remove(&id) {
            self.free_texture(gpu, old);
        }
        let Some(slot) = self.free_slots.borrow_mut().pop() else {
            return Err(GpuError::Invalid("egui's textures fill the painter's descriptor heap".to_owned()));
        };
        let texture = gpu.create_texture(&TextureDesc {
            label: "egui",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { sampled: true, ..TextureUsage::default() },
            mipmapped: false,
        });
        let texture = match texture.and_then(|texture| {
            gpu.write_texture(texture, TextureRegion::whole(size), &pixels).map(|()| texture).inspect_err(|_| {
                destroy_unused(gpu, [ResourceId::Texture(texture)]);
            })
        }) {
            Ok(texture) => texture,
            Err(e) => {
                self.free_slots.borrow_mut().push(slot);
                return Err(e);
            }
        };
        let Some((resource, _)) = gpu.native_texture(texture) else {
            destroy_unused(gpu, [ResourceId::Texture(texture)]);
            self.free_slots.borrow_mut().push(slot);
            return Err(GpuError::UnknownId("texture"));
        };
        // SAFETY: the view is of the live texture, into a descriptor no frame
        // reads: a free slot's.
        unsafe { gpu.device().device().CreateShaderResourceView(resource, None, self.cpu_slot(slot)) };
        let sampler = u32::from(delta.options.magnification == egui::TextureFilter::Nearest)
            + 2 * u32::from(delta.options.wrap_mode != egui::TextureWrapMode::ClampToEdge);
        self.textures.insert(id, Texture { texture, slot, sampler });
        Ok(())
    }

    /// Writes the meshes' vertices and indexes to the frame's buffers, and
    /// lists their draws.
    fn prepare(
        &mut self,
        target: &mut OverlayTarget<'_>,
        primitives: &[egui::ClippedPrimitive],
        pixels_per_point: f32,
    ) -> Result<(), String> {
        let (width, height) = (target.size.width as f32, target.size.height as f32);
        let mut vertices: Vec<u8> = Vec::new();
        let mut indexes: Vec<u8> = Vec::new();
        let mut draws = Vec::new();
        for primitive in primitives {
            let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive else { continue };
            let Some(texture) = self.textures.get(&mesh.texture_id) else { continue };
            if mesh.indices.is_empty() {
                continue;
            }
            // The clip rectangle in pixels, inside the target.
            let clip = primitive.clip_rect;
            let x0 = (clip.min.x * pixels_per_point).round().clamp(0.0, width) as i32;
            let y0 = (clip.min.y * pixels_per_point).round().clamp(0.0, height) as i32;
            let x1 = (clip.max.x * pixels_per_point).round().clamp(0.0, width) as i32;
            let y1 = (clip.max.y * pixels_per_point).round().clamp(0.0, height) as i32;
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            draws.push(MeshDraw {
                scissor: RECT { left: x0, top: y0, right: x1, bottom: y1 },
                slot: texture.slot,
                sampler: texture.sampler,
                first_vertex: (vertices.len() / VERTEX_STRIDE as usize) as i32,
                first_index: (indexes.len() / size_of::<u32>()) as u32,
                index_count: mesh.indices.len() as u32,
            });
            for vertex in &mesh.vertices {
                for value in [vertex.pos.x, vertex.pos.y, vertex.uv.x, vertex.uv.y] {
                    vertices.extend_from_slice(&value.to_ne_bytes());
                }
                vertices.extend_from_slice(&vertex.color.to_array());
            }
            for index in &mesh.indices {
                indexes.extend_from_slice(&index.to_ne_bytes());
            }
        }
        if draws.is_empty() {
            return Ok(());
        }
        let gpu = &mut *target.gpu;
        let vertex_buffer = buffer(gpu, BufferUsage::Vertex, &vertices)?;
        self.buffers.push(vertex_buffer);
        let index_buffer = buffer(gpu, BufferUsage::Index, &indexes)?;
        self.buffers.push(index_buffer);
        let (Some(vertex_address), Some(index_address)) =
            (gpu.buffer_address(vertex_buffer), gpu.buffer_address(index_buffer))
        else {
            return Err("the meshes' buffers".to_owned());
        };
        self.frame = Some(Frame {
            vertex_buffer: D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: vertex_address,
                SizeInBytes: vertices.len() as u32,
                StrideInBytes: VERTEX_STRIDE,
            },
            index_buffer: D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: index_address,
                SizeInBytes: indexes.len() as u32,
                Format: DXGI_FORMAT_R32_UINT,
            },
            screen_size: [width / pixels_per_point, height / pixels_per_point],
            draws,
        });
        Ok(())
    }

    /// Records the frame's draws into the pass, clipped to their rectangles.
    fn draw(&mut self, pass: &CustomLayerPass, size: Size) {
        let Some(frame) = self.frame.take() else { return };
        let list = &pass.command_list;
        let [screen_width, screen_height] = frame.screen_size;
        // SAFETY: the list records in the pass, of the pipeline's format, the
        // buffers and descriptors hold what the draws read, which stays until
        // the frame is done, and each scissor rectangle is inside the target.
        unsafe {
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetGraphicsRootSignature(&self.root_signature);
            list.SetPipelineState(&self.pipeline);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.IASetVertexBuffers(0, Some(&[frame.vertex_buffer]));
            list.IASetIndexBuffer(Some(&raw const frame.index_buffer));
            list.RSSetViewports(&[D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: size.width as f32,
                Height: size.height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]);
            list.SetGraphicsRoot32BitConstants(
                CONSTANTS,
                2,
                [screen_width.to_bits(), screen_height.to_bits()].as_ptr().cast(),
                0,
            );
            for draw in &frame.draws {
                list.RSSetScissorRects(&[draw.scissor]);
                list.SetGraphicsRoot32BitConstant(CONSTANTS, draw.sampler, 2);
                list.SetGraphicsRootDescriptorTable(TEXTURE, self.gpu_slot(draw.slot));
                list.DrawIndexedInstanced(draw.index_count, 1, draw.first_index, draw.first_vertex, 0);
            }
        }
    }

    /// Destroys what the painter made, once the frames that use it are
    /// done.
    fn destroy(self, gpu: &mut D3d12Gpu) {
        for (_, texture) in self.textures {
            destroy_unused(gpu, [ResourceId::Texture(texture.texture)]);
        }
        for buffer in self.buffers {
            destroy_unused(gpu, [ResourceId::Buffer(buffer)]);
        }
        gpu.destroy_later(Box::new((self.root_signature, self.pipeline, self.heap)));
    }
}

/// A descriptor slot on its way back to the free slots, which it returns to
/// when the `Gpu` drops it: once the frames that read it are done.
struct FreeSlot {
    slot: u32,
    free: Rc<RefCell<Vec<u32>>>,
}

impl Drop for FreeSlot {
    fn drop(&mut self) {
        self.free.borrow_mut().push(self.slot);
    }
}

/// A buffer of `usage` holding `data`.
fn buffer(gpu: &mut D3d12Gpu, usage: BufferUsage, data: &[u8]) -> Result<BufferId, String> {
    let buffer = gpu.create_buffer(&BufferDesc { label: "egui", usage, size: data.len() as u64 });
    let buffer = buffer.map_err(|e| e.to_string())?;
    if let Err(e) = gpu.write_buffer(buffer, 0, data) {
        destroy_unused(gpu, [ResourceId::Buffer(buffer)]);
        return Err(e.to_string());
    }
    Ok(buffer)
}
