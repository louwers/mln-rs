use mln_gfx::gpu::{Gpu, TextureDesc, TextureFormat, TextureRegion, TextureUsage};
use mln_layers::conformance::{
    clip_mask_and_background, custom_layers, depth_only, foreign_ids, imported_textures, instanced_walls,
    invalid_calls, pulled_walls, readbacks, resources_in_use, topologies, transparent_replace, vertex_layouts,
    wall_quads,
};
use mln_util::size::Size;

use crate::{CustomLayerEncoder, CustomLayerPass, Device, WebGpu};

#[test]
fn clip_mask_and_background_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    clip_mask_and_background(&mut WebGpu::new(&device)).expect("the frame");
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
    transparent_replace(&mut WebGpu::new(&device)).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    topologies(&mut WebGpu::new(&device)).expect("the frames");
}

#[test]
fn depth_only_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    depth_only(&mut WebGpu::new(&device)).expect("the frame");
}

#[test]
fn vertex_layouts_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    vertex_layouts(&mut WebGpu::new(&device)).expect("the frames");
}

#[test]
fn readbacks_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    readbacks(&mut WebGpu::new(&device)).expect("the readbacks");
}

#[test]
fn resources_in_use_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    resources_in_use(&mut WebGpu::new(&device)).expect("the frames");
}

// A frame read back each time copies into the buffer of the readback
// before, which holds what the last copy wrote.
#[test]
fn a_readback_copies_into_the_last_ones_buffer() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    let size = Size { width: 4, height: 2 };
    let texture = gpu
        .create_texture(&TextureDesc {
            label: "read back",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { sampled: true, copy_src: true, ..TextureUsage::default() },
        })
        .expect("a texture");
    let whole = TextureRegion::whole(size);
    gpu.write_texture(texture, whole, &[1; 4 * 4 * 2]).expect("the first texels");
    let first = gpu.read_texture(texture, whole).expect("a readback");
    let buffer = gpu.readback_buffer(first).cloned();
    assert_eq!(gpu.take_readback(first, true).expect("the texels"), Some(vec![1; 4 * 4 * 2]));
    gpu.write_texture(texture, whole, &[2; 4 * 4 * 2]).expect("the second texels");
    let second = gpu.read_texture(texture, whole).expect("a readback");
    assert!(buffer.is_some() && gpu.readback_buffer(second) == buffer.as_ref(), "the first readback's buffer");
    assert_eq!(gpu.take_readback(second, true).expect("the texels"), Some(vec![2; 4 * 4 * 2]));
}

#[test]
fn instanced_walls_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    instanced_walls(&mut gpu).expect("the frame");
}

#[test]
fn wall_quads_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    wall_quads(&mut gpu).expect("the frame");
}

#[test]
fn pulled_walls_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    pulled_walls(&mut gpu).expect("the frame");
}

#[test]
fn custom_layers_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    custom_layers(
        &mut WebGpu::new(&device),
        |lent| assert!(lent.is::<CustomLayerEncoder>()),
        |lent| {
            let lent = lent.downcast_mut::<CustomLayerPass>().expect("a WebGPU pass");
            assert_eq!(lent.color_format, wgpu::TextureFormat::Rgba8Unorm);
        },
    )
    .expect("the frames");
}

#[test]
fn imported_textures_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    imported_textures(&mut WebGpu::new(&device), |gpu, texture| {
        let native = gpu.native_texture(texture).expect("a texture of the backend's").clone();
        Ok(gpu.import_texture(&native))
    })
    .expect("the frames");
}
