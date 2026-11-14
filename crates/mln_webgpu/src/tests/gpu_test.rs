use mln_gfx::gpu::TextureId;
use mln_gfx::gpu::conformance::{
    SIZE, clip_mask_and_background, depth_only, foreign_ids, invalid_calls, topologies, transparent_replace,
    vertex_layouts,
};

use crate::{Device, WebGpu};

/// The RGBA texels of `texture`.
fn read_back(device: &Device, gpu: &WebGpu, texture: TextureId) -> Vec<u8> {
    let texture = gpu.texture(texture).expect("the color texture");
    let (device, queue) = (device.device(), device.queue());
    let row = SIZE.width * 4;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * SIZE.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(SIZE.height),
            },
        },
        wgpu::Extent3d { width: SIZE.width, height: SIZE.height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    staging.slice(..).map_async(wgpu::MapMode::Read, |result| result.expect("mapped"));
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("polled");
    let mapped = staging.slice(..).get_mapped_range().expect("mapped range");
    mapped.chunks(padded as usize).flat_map(|line| &line[..row as usize]).copied().collect()
}

#[test]
fn clip_mask_and_background_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    clip_mask_and_background(&mut gpu, |gpu, texture| read_back(&device, gpu, texture)).expect("the frame");
}

#[test]
fn invalid_calls_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    invalid_calls(&mut WebGpu::new(&device)).expect("the valid calls");
}

#[test]
fn foreign_ids_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    foreign_ids(&mut WebGpu::new(&device), &mut WebGpu::new(&device)).expect("the valid calls");
}

#[test]
fn transparent_replace_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    transparent_replace(&mut gpu, |gpu, texture| read_back(&device, gpu, texture)).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    topologies(&mut gpu, |gpu, texture| read_back(&device, gpu, texture)).expect("the frames");
}

#[test]
fn depth_only_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    depth_only(&mut gpu, |gpu, texture| read_back(&device, gpu, texture)).expect("the frame");
}

#[test]
fn vertex_layouts_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    vertex_layouts(&mut gpu, |gpu, texture| read_back(&device, gpu, texture)).expect("the frames");
}
