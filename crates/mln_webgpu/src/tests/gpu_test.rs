use mln_gfx::gpu::conformance::{
    clip_mask_and_background, depth_only, foreign_ids, instanced_walls, invalid_calls, readbacks, topologies,
    transparent_replace, vertex_layouts,
};

use crate::{Device, WebGpu};

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
fn instanced_walls_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);
    instanced_walls(&mut gpu).expect("the frame");
}
