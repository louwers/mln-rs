use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mln_engine::conformance::{
    clip_mask_and_background, custom_layers, depth_only, drawn_vertex_texels, foreign_ids, imported_textures,
    instanced_walls, invalid_calls, mipmapped_targets, pulled_walls, readbacks, resources_in_use, terrain_heights,
    topologies, transparent_replace, vertex_layouts, written_vertex_texels,
};
use mln_engine::layers::fill_extrusion::programs::FILL_EXTRUSION_INSTANCED;
use mln_gfx::Rgba;
use mln_gfx::gpu::{BufferUsage, ColorAttachment};
use objc2::runtime::NSObjectProtocol as _;
use objc2::{msg_send, sel};
use objc2_metal::MTLSharedEvent as _;

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
fn resources_in_use_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    resources_in_use(&mut MetalGpu::new(&device)).expect("the frames");
}

#[test]
fn depth_only_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    depth_only(&mut MetalGpu::new(&device)).expect("the frame");
}

#[test]
fn written_vertex_texels_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    written_vertex_texels(&mut MetalGpu::new(&device)).expect("the frame");
}

#[test]
fn drawn_vertex_texels_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    drawn_vertex_texels(&mut MetalGpu::new(&device)).expect("the frames");
}

#[test]
fn terrain_heights_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    terrain_heights(&mut MetalGpu::new(&device)).expect("the frames");
}

#[test]
fn mipmapped_targets_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    mipmapped_targets(&mut MetalGpu::new(&device)).expect("the frames");
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
fn pulled_walls_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    pulled_walls(&mut MetalGpu::new(&device)).expect("the frame");
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
    let program = &FILL_EXTRUSION_INSTANCED;
    let (vertex, fragment) =
        compile(device.device(), "direct read", MSL, "vertexMain", "fragmentMain", [], true).expect("the MSL compiles");
    let active_attributes = active_attributes(&vertex);
    let shader = gpu.shaders.insert(Shader { program, vertex, active_attributes, fragment, table: None });
    let unblended = mln_gfx::color_mode::ColorMode::unblended();
    let layouts = [VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![GpuVertexAttribute {
            location: program.location("pos") as u32,
            format: AttributeDataType::Short2,
            offset: 0,
        }],
    }];
    let desc = PipelineDesc {
        shader,
        vertex_buffers: &layouts,
        state: FixedState {
            primitive: DrawModeType::Triangles,
            cull: CullFaceMode::disabled(),
            blend: unblended.blend_function,
            color_mask: unblended.mask,
            depth: DepthState::from(&mln_gfx::depth_mode::DepthMode::disabled()),
            stencil: StencilState::from(&StencilMode::disabled()),
        },
        targets: TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None },
    };
    assert!(matches!(gpu.create_pipeline(&desc), Err(GpuError::Invalid(_))));
}

// What a flush uses stays, though destroyed, until its command buffers
// finish.
#[test]
fn a_destroyed_buffer_stays_for_the_flushes_before() {
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
            mipmapped: false,
        })
        .unwrap();
    let pass = PassDesc {
        label: "clear",
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 1.0)) }),
        depth_stencil: None,
    };
    let texel = TextureRegion { x: 0, y: 0, size: Size { width: 1, height: 1 } };
    assert!(matches!(gpu.write_texture(target, texel, &[0; 4]), Err(GpuError::Invalid(_))), "the target isn't sampled");
    gpu.begin_pass(&pass).unwrap();
    gpu.end_pass().unwrap();
    gpu.flush(false).unwrap();
    gpu.destroy(ResourceId::Buffer(buffer)).unwrap();
    gpu.destroy(ResourceId::Texture(target)).unwrap();
    gpu.flush(true).unwrap();
    assert!(gpu.in_flight.is_empty());
}

// A command buffer finishes once its completed handlers run, not once its
// status says so: Metal reports it completed before running them, and its
// API validation counts its references to what it uses until then.
#[test]
fn a_flush_finishes_once_its_completed_handlers_run() {
    let device = Device::new(None).expect("Metal device");
    let mut gpu = MetalGpu::new(&device);
    // Holds up the completed handlers of the next flush's command buffer,
    // after it reports that it completed, with a handler of Metal's own
    // command buffer under API validation's wrappers, which runs before
    // theirs.
    let held = Arc::new(AtomicBool::new(true));
    gpu.before_commit = Some(Box::new({
        let held = Arc::clone(&held);
        move |command_buffer| {
            let mut inner = command_buffer.retain();
            while inner.respondsToSelector(sel!(baseObject)) {
                // SAFETY: API validation's wrappers wrap a command buffer.
                inner = unsafe { msg_send![&*inner, baseObject] };
            }
            let held = Arc::clone(&held);
            let hold = RcBlock::new(move |_: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                std::thread::sleep(Duration::from_millis(100));
                held.store(false, Ordering::SeqCst);
            });
            // SAFETY: the block is valid, and Metal copies it.
            unsafe { inner.addCompletedHandler(RcBlock::as_ptr(&hold)) };
        }
    }));
    let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage: BufferUsage::Uniform, size: 1008 }).unwrap();
    gpu.write_buffer(buffer, 0, &[1; 1008]).unwrap();
    gpu.flush(false).unwrap();
    gpu.before_commit = None;
    while !gpu.in_flight.is_empty() {
        gpu.retire();
        assert!(!gpu.in_flight.is_empty() || !held.load(Ordering::SeqCst), "finished before the completed handlers");
    }
    assert!(!held.load(Ordering::SeqCst));
    gpu.flush(true).unwrap();
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

#[test]
fn imported_textures_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    imported_textures(&mut MetalGpu::new(&device), |gpu, texture| {
        let Texture { texture, format, .. } = gpu.texture(texture)?;
        let (texture, format) = (texture.clone(), *format);
        gpu.import_texture(&texture, format)
    })
    .expect("the frames");
}

// A flush signals an event once its commands are done, and a pass that
// draws into a texture waits for the event's value.
#[test]
fn flushes_wait_for_and_signal_events() {
    let device = Device::new(None).expect("Metal device");
    let mut gpu = MetalGpu::new(&device);
    let shared = device.device().newSharedEvent().expect("a shared event");
    let event = ProtocolObject::<dyn MTLEvent>::from_ref(&*shared);
    gpu.signal_after_next_submit(event, 1);
    gpu.flush(true).unwrap();
    assert_eq!(shared.signaledValue(), 1);
    let target = gpu
        .create_texture(&TextureDesc {
            label: "target",
            size: Size { width: 4, height: 4 },
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { render_target: true, ..Default::default() },
            mipmapped: false,
        })
        .unwrap();
    let pass = PassDesc {
        label: "clear",
        color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 1.0)) }),
        depth_stencil: None,
    };
    gpu.wait_before_using(target, event, 1).unwrap();
    gpu.signal_after_next_submit(event, 2);
    gpu.begin_pass(&pass).unwrap();
    gpu.end_pass().unwrap();
    gpu.flush(true).unwrap();
    assert_eq!(shared.signaledValue(), 2);
}
