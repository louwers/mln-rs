use mln_gfx::gpu::conformance::{
    clip_mask_and_background, depth_only, foreign_ids, invalid_calls, topologies, transparent_replace, vertex_layouts,
};
use mln_gfx::gpu::{BufferUsage, ColorAttachment};
use mln_util::Color;
use objc2_metal::MTLCommandBuffer;

use super::*;

/// The RGBA texels of `texture`, once the work submitted before is done.
fn read_back(gpu: &mut MetalGpu, texture: TextureId) -> Vec<u8> {
    gpu.wait_until_completed();
    let texture = &gpu.textures.get(texture).expect("the texture").texture;
    let (width, height) = (texture.width(), texture.height());
    let (row, length) = (width * 4, width * 4 * height);
    let buffer = gpu
        .device
        .device()
        .newBufferWithLength_options(length, MTLResourceOptions::StorageModeShared)
        .expect("a buffer");
    let command_buffer = gpu.device.queue().commandBuffer().expect("a command buffer");
    let blit = command_buffer.blitCommandEncoder().expect("a blit encoder");
    // SAFETY: the copy is within the texture and the buffer.
    unsafe {
        blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
            texture,
            0,
            0,
            MTLOrigin { x: 0, y: 0, z: 0 },
            MTLSize { width, height, depth: 1 },
            &buffer,
            0,
            row,
            length,
        );
    }
    blit.endEncoding();
    command_buffer.commit();
    command_buffer.waitUntilCompleted();
    // SAFETY: the shared buffer holds `length` bytes, which the GPU is done
    // writing.
    unsafe { std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), length) }.to_vec()
}

#[test]
fn clip_mask_and_background_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    let mut gpu = MetalGpu::new(&device);
    clip_mask_and_background(&mut gpu, read_back).expect("the frame");
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
    transparent_replace(&mut MetalGpu::new(&device), read_back).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    topologies(&mut MetalGpu::new(&device), read_back).expect("the frames");
}

#[test]
fn depth_only_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    depth_only(&mut MetalGpu::new(&device), read_back).expect("the frame");
}

#[test]
fn vertex_layouts_through_gpu() {
    let device = Device::new(None).expect("Metal device");
    vertex_layouts(&mut MetalGpu::new(&device), read_back).expect("the frames");
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
    gpu.wait_until_completed();
    assert!(gpu.retired.is_empty());
    assert!(gpu.in_flight.is_empty());
}
