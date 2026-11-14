use mln_gfx::gpu::conformance::{
    clip_mask_and_background, custom_layers, depth_only, foreign_ids, instanced_walls, invalid_calls, readbacks,
    topologies, transparent_replace, vertex_layouts,
};
use mln_gfx::gpu::{BufferUsage, ColorAttachment};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_util::Color;

use super::*;

#[test]
fn clip_mask_and_background_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    clip_mask_and_background(&mut MetalGpu::new(&device)).expect("the frame");
}

#[test]
fn invalid_calls_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    invalid_calls(&mut MetalGpu::new(&device)).expect("the valid calls");
}

#[test]
fn foreign_ids_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    foreign_ids(&mut MetalGpu::new(&device), &mut MetalGpu::new(&device)).expect("the valid calls");
}

#[test]
fn transparent_replace_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    transparent_replace(&mut MetalGpu::new(&device)).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    topologies(&mut MetalGpu::new(&device)).expect("the frames");
}

#[test]
fn readbacks_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    readbacks(&mut MetalGpu::new(&device)).expect("the readbacks");
}

#[test]
fn depth_only_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    depth_only(&mut MetalGpu::new(&device)).expect("the frame");
}

#[test]
fn vertex_layouts_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    vertex_layouts(&mut MetalGpu::new(&device)).expect("the frames");
}

#[test]
fn instanced_walls_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    instanced_walls(&mut MetalGpu::new(&device)).expect("the frame");
}

#[test]
fn a_vertex_function_reading_where_vertex_buffers_bind_is_rejected() {
    // Reads buffer 8, where the instanced fill extrusion shader's second
    // vertex buffer slot binds, directly.
    const MSL: &str = "
#include <metal_stdlib>
using namespace metal;
struct VertexStage { short2 pos [[attribute(0)]]; };
vertex float4 vertexMain(VertexStage in [[stage_in]], device const short2* outline [[buffer(8)]],
                         uint instance [[instance_id]]) {
    return float4(float2(in.pos) + float2(outline[instance]), 0.0, 1.0);
}
fragment half4 fragmentMain() { return half4(1.0); }
";
    let device = Device::new(None).expect("Metal device");
    let mut gpu = MetalGpu::new(&device);
    let program = BuiltIn::FillExtrusionInstancedShader;
    let (vertex, fragment) =
        compile(device.device(), "direct read", MSL, "vertexMain", "fragmentMain", []).expect("the MSL compiles");
    let source = shader_source(program).expect("the table");
    let shader = ShaderKey::new(program, []);
    gpu.shaders.insert(shader.clone(), Shader { source, vertex, fragment });
    let unblended = mln_gfx::color_mode::ColorMode::unblended();
    let key = PipelineKey {
        shader,
        state: mln_gfx::gpu::PipelineState {
            vertex_buffers: vec![VertexBufferLayout {
                stride: 4,
                step: VertexStep::Vertex,
                attributes: vec![mln_gfx::gpu::VertexAttribute {
                    id: mln_gfx::shaders::shader_defines::ID_FILL_EXTRUSION_POS_VERTEX_ATTRIBUTE as u32,
                    format: AttributeDataType::Short2,
                    offset: 0,
                }],
            }],
            primitive: DrawModeType::Triangles,
            cull: CullFaceMode::disabled(),
            blend: unblended.blend_function,
            color_mask: unblended.mask,
            depth: DepthState::from(&mln_gfx::depth_mode::DepthMode::disabled()),
            stencil: StencilState::from(&mln_gfx::stencil_mode::StencilMode::disabled()),
        },
        targets: TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None },
    };
    assert!(matches!(gpu.create_pipeline(&key), Err(GpuError::Invalid(_))));
}

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
            usage: TextureUsage { render_target: true, ..Default::default() },
        })
        .unwrap();
    let pass = Pass {
        label: "clear".to_owned(),
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }) }),
        depth_stencil: None,
        commands: Vec::new(),
    };
    let texel = TextureRegion { x: 0, y: 0, size: Size { width: 1, height: 1 } };
    assert!(matches!(gpu.write_texture(target, texel, &[0; 4]), Err(GpuError::Invalid(_))), "the target isn't sampled");
    gpu.submit(&[pass]).unwrap();
    gpu.destroy_buffer(buffer);
    gpu.destroy_texture(target);
    if !gpu.in_flight.is_empty() {
        // The buffer, the texture and the write's staging buffer.
        assert_eq!(gpu.retired.len(), 3, "the GPU may still be using them");
    }
    gpu.finish().unwrap();
    assert!(gpu.retired.is_empty());
    assert!(gpu.in_flight.is_empty());
}

#[test]
fn custom_layers_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    custom_layers(
        &mut MetalGpu::new(&device),
        |lent| assert!(lent.is::<CustomLayerEncoder>()),
        |lent| {
            let lent = lent.downcast_mut::<CustomLayerPass>().expect("a Metal pass");
            assert_eq!(lent.formats.color, MTLPixelFormat::RGBA8Unorm);
            assert_eq!(lent.formats.depth, None);
        },
    )
    .expect("the frames");
}
