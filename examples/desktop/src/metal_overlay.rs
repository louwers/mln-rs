// The controls' overlay on Metal: egui's meshes drawn over the map's frame,
// in its command buffer, as egui-wgpu draws them on WebGPU (overlay.rs): in
// gamma space, premultiplied, with egui-wgpu's blending, but no dithering.

// Metal through objc2 is the FFI here.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use mln_mtl::surface_target::{Overlay, OverlayTarget};
use mln_util::logging::{Event, Log};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLIndexType,
    MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder,
    MTLRenderPassDescriptor, MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions,
    MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerState, MTLScissorRect, MTLSize,
    MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage, MTLVertexDescriptor, MTLVertexFormat,
    MTLVertexStepFunction,
};

use crate::overlay::Paint;

const SHADER: &str = r"
#include <metal_stdlib>
using namespace metal;

struct VertexIn {
    float2 pos [[attribute(0)]];
    float2 uv [[attribute(1)]];
    float4 color [[attribute(2)]];
};

struct VertexOut {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

// `screen_size` is in points, as the vertices are.
vertex VertexOut vertexMain(VertexIn in [[stage_in]], constant float2& screen_size [[buffer(1)]]) {
    VertexOut out;
    out.position = float4(2.0 * in.pos.x / screen_size.x - 1.0, 1.0 - 2.0 * in.pos.y / screen_size.y, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

fragment float4 fragmentMain(VertexOut in [[stage_in]],
                             texture2d<float> tex [[texture(0)]],
                             sampler tex_sampler [[sampler(0)]]) {
    return in.color * tex.sample(tex_sampler, in.uv);
}
";

/// The vertex buffer's slot; the screen size takes the next.
const VERTEX_BUFFER: usize = 0;
const SCREEN_SIZE_BUFFER: usize = 1;
/// The size of an `epaint::Vertex`: position and texture coordinates as f32
/// pairs, then the color as four bytes.
const VERTEX_STRIDE: usize = 20;

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Sampler = Retained<ProtocolObject<dyn MTLSamplerState>>;

struct Painter {
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    textures: HashMap<egui::TextureId, (Texture, Sampler)>,
}

/// The overlay that paints the controls on Metal, made with the first frame.
pub(crate) fn metal_overlay(paint: Rc<RefCell<Paint>>) -> Overlay {
    let mut painter: Option<Painter> = None;
    let mut failed = false;
    Box::new(move |target: &OverlayTarget<'_>| {
        if failed {
            return;
        }
        if painter.is_none() {
            match Painter::new(target.device, target.texture.pixelFormat()) {
                Ok(new) => painter = Some(new),
                Err(e) => {
                    Log::error(Event::Render, format!("The controls can't be drawn: {e}"));
                    failed = true;
                    return;
                }
            }
        }
        let Some(painter) = painter.as_mut() else { return };
        let mut paint = paint.borrow_mut();
        let mut textures = std::mem::take(&mut paint.textures);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                painter.update_texture(target.device, *id, delta);
            }
        }
        painter.paint(target, &paint.primitives, paint.pixels_per_point);
        for id in &textures.free {
            painter.textures.remove(id);
        }
        textures.clear();
    })
}

impl Painter {
    fn new(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat) -> Result<Painter, String> {
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(SHADER), None)
            .map_err(|e| e.localizedDescription().to_string())?;
        let function = |name: &str| {
            library.newFunctionWithName(&NSString::from_str(name)).ok_or_else(|| format!("no function {name}"))
        };
        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setLabel(Some(&NSString::from_str("controls")));
        let (vertex_function, fragment_function) = (function("vertexMain")?, function("fragmentMain")?);
        descriptor.setVertexFunction(Some(&vertex_function));
        descriptor.setFragmentFunction(Some(&fragment_function));

        let vertex_descriptor = MTLVertexDescriptor::vertexDescriptor();
        // SAFETY: the attribute and buffer indexes are below Metal's 31.
        unsafe {
            for (index, (format, offset)) in
                [(MTLVertexFormat::Float2, 0), (MTLVertexFormat::Float2, 8), (MTLVertexFormat::UChar4Normalized, 16)]
                    .into_iter()
                    .enumerate()
            {
                let attribute = vertex_descriptor.attributes().objectAtIndexedSubscript(index);
                attribute.setFormat(format);
                attribute.setOffset(offset);
                attribute.setBufferIndex(VERTEX_BUFFER);
            }
            let layout = vertex_descriptor.layouts().objectAtIndexedSubscript(VERTEX_BUFFER);
            layout.setStride(VERTEX_STRIDE);
            layout.setStepFunction(MTLVertexStepFunction::PerVertex);
            layout.setStepRate(1);
        }
        descriptor.setVertexDescriptor(Some(&vertex_descriptor));

        // SAFETY: index 0 is in the attachment array's bounds.
        let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        color.setPixelFormat(format);
        color.setBlendingEnabled(true);
        color.setRgbBlendOperation(MTLBlendOperation::Add);
        color.setSourceRGBBlendFactor(MTLBlendFactor::One);
        color.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        color.setAlphaBlendOperation(MTLBlendOperation::Add);
        color.setSourceAlphaBlendFactor(MTLBlendFactor::OneMinusDestinationAlpha);
        color.setDestinationAlphaBlendFactor(MTLBlendFactor::One);

        let pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&descriptor)
            .map_err(|e| e.localizedDescription().to_string())?;
        Ok(Painter { pipeline, textures: HashMap::new() })
    }

    /// Applies one of egui's texture updates: a whole new texture, or a
    /// region of an existing one.
    fn update_texture(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        id: egui::TextureId,
        delta: &egui::epaint::ImageDelta,
    ) {
        let egui::ImageData::Color(image) = &delta.image;
        let [width, height] = image.size;
        if width == 0 || height == 0 {
            return;
        }
        let pixels: Vec<u8> = image.pixels.iter().flat_map(egui::Color32::to_array).collect();
        let origin = if let Some([x, y]) = delta.pos {
            MTLOrigin { x, y, z: 0 }
        } else {
            // SAFETY: the size is non-zero and the format a valid 2D one.
            let descriptor = unsafe {
                MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                    MTLPixelFormat::RGBA8Unorm,
                    width,
                    height,
                    false,
                )
            };
            descriptor.setUsage(MTLTextureUsage::ShaderRead);
            // The CPU writes it (see mln_mtl's texture2d.rs).
            descriptor.setAllowGPUOptimizedContents(false);
            let (Some(texture), Some(sampler)) =
                (device.newTextureWithDescriptor(&descriptor), new_sampler(device, delta.options))
            else {
                return;
            };
            self.textures.insert(id, (texture, sampler));
            MTLOrigin { x: 0, y: 0, z: 0 }
        };
        let Some((texture, _)) = self.textures.get(&id) else {
            return;
        };
        let region = MTLRegion { origin, size: MTLSize { width, height, depth: 1 } };
        // SAFETY: `pixels` holds the region's rows, which Metal copies, and
        // the region is inside the texture egui made it for.
        unsafe {
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                region,
                0,
                NonNull::from(pixels.as_slice()).cast::<c_void>(),
                width * 4,
            );
        }
    }

    /// Encodes the controls' meshes over the target, clipped to their
    /// rectangles.
    fn paint(&self, target: &OverlayTarget<'_>, primitives: &[egui::ClippedPrimitive], pixels_per_point: f32) {
        let meshes: Vec<(&egui::Rect, &egui::Mesh)> = primitives
            .iter()
            .filter_map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(mesh) if !mesh.indices.is_empty() => Some((&p.clip_rect, mesh)),
                _ => None,
            })
            .collect();
        if meshes.is_empty() {
            return;
        }

        // All the meshes' vertices and indexes, each mesh's after the last.
        let mut vertices = Vec::new();
        let mut indexes: Vec<u8> = Vec::new();
        let mut draws = Vec::with_capacity(meshes.len());
        for (clip, mesh) in &meshes {
            draws.push((*clip, mesh.texture_id, vertices.len(), indexes.len(), mesh.indices.len()));
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
        let (Some(vertex_buffer), Some(index_buffer)) =
            (new_buffer(target.device, &vertices), new_buffer(target.device, &indexes))
        else {
            return;
        };

        let descriptor = MTLRenderPassDescriptor::new();
        // SAFETY: index 0 is in the attachment array's bounds.
        let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(target.texture));
        color.setLoadAction(MTLLoadAction::Load);
        color.setStoreAction(MTLStoreAction::Store);
        let Some(encoder) = target.command_buffer.renderCommandEncoderWithDescriptor(&descriptor) else {
            return;
        };
        encoder.pushDebugGroup(&NSString::from_str("controls"));
        encoder.setRenderPipelineState(&self.pipeline);

        let (width, height) = (target.size.width as f32, target.size.height as f32);
        let screen_size = [width / pixels_per_point, height / pixels_per_point];
        // SAFETY: Metal copies the size; the slots are below Metal's 31.
        unsafe {
            encoder.setVertexBytes_length_atIndex(
                NonNull::from(&screen_size).cast::<c_void>(),
                std::mem::size_of_val(&screen_size),
                SCREEN_SIZE_BUFFER,
            );
            encoder.setVertexBuffer_offset_atIndex(Some(&vertex_buffer), 0, VERTEX_BUFFER);
        }

        for (clip, texture_id, vertex_offset, index_offset, index_count) in draws {
            // The clip rectangle in pixels, inside the target.
            let x0 = (clip.min.x * pixels_per_point).round().clamp(0.0, width) as usize;
            let y0 = (clip.min.y * pixels_per_point).round().clamp(0.0, height) as usize;
            let x1 = (clip.max.x * pixels_per_point).round().clamp(0.0, width) as usize;
            let y1 = (clip.max.y * pixels_per_point).round().clamp(0.0, height) as usize;
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let Some((texture, sampler)) = self.textures.get(&texture_id) else {
                continue;
            };
            encoder.setScissorRect(MTLScissorRect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 });
            // SAFETY: the texture, sampler and buffers live until the command
            // buffer, which retains them, completes; the offsets and the
            // index count are inside the buffers.
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(texture), 0);
                encoder.setFragmentSamplerState_atIndex(Some(sampler), 0);
                encoder.setVertexBufferOffset_atIndex(vertex_offset, VERTEX_BUFFER);
                encoder.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset(
                    MTLPrimitiveType::Triangle,
                    index_count,
                    MTLIndexType::UInt32,
                    &index_buffer,
                    index_offset,
                );
            }
        }
        encoder.popDebugGroup();
        encoder.endEncoding();
    }
}

fn new_buffer(device: &ProtocolObject<dyn MTLDevice>, data: &[u8]) -> Option<Retained<ProtocolObject<dyn MTLBuffer>>> {
    // SAFETY: `data` is non-empty, and Metal copies it.
    unsafe {
        device.newBufferWithBytes_length_options(
            NonNull::from(data).cast::<c_void>(),
            data.len(),
            MTLResourceOptions::StorageModeShared,
        )
    }
}

fn new_sampler(device: &ProtocolObject<dyn MTLDevice>, options: egui::TextureOptions) -> Option<Sampler> {
    let filter = |filter: egui::TextureFilter| match filter {
        egui::TextureFilter::Nearest => MTLSamplerMinMagFilter::Nearest,
        egui::TextureFilter::Linear => MTLSamplerMinMagFilter::Linear,
    };
    let wrap = match options.wrap_mode {
        egui::TextureWrapMode::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
        egui::TextureWrapMode::Repeat => MTLSamplerAddressMode::Repeat,
        egui::TextureWrapMode::MirroredRepeat => MTLSamplerAddressMode::MirrorRepeat,
    };
    let descriptor = MTLSamplerDescriptor::new();
    descriptor.setMagFilter(filter(options.magnification));
    descriptor.setMinFilter(filter(options.minification));
    descriptor.setSAddressMode(wrap);
    descriptor.setTAddressMode(wrap);
    device.newSamplerStateWithDescriptor(&descriptor)
}
