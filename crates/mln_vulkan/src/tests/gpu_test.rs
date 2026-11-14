use ash::vk;
use mln_gfx::gpu::ShaderKey;
use mln_gfx::gpu::conformance::{
    clip_mask_and_background, custom_layers, depth_only, foreign_ids, instanced_walls, invalid_calls, readbacks,
    topologies, transparent_replace, vertex_layouts,
};
use mln_gfx::shader_group::GLOBE_DEFINE;

use super::glsl_compile_test::{ALL_BUILT_INS, uniform_properties};
use crate::shaders::shader_source::shader_source;
use crate::{Device, VulkanGpu};

#[test]
fn clip_mask_and_background_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    clip_mask_and_background(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

#[test]
fn invalid_calls_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    invalid_calls(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the valid calls");
}

#[test]
fn foreign_ids_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut first = VulkanGpu::new(&device).expect("VulkanGpu");
    let mut second = VulkanGpu::new(&device).expect("VulkanGpu");
    foreign_ids(&mut first, &mut second).expect("the valid calls");
}

#[test]
fn transparent_replace_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    transparent_replace(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    topologies(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
}

#[test]
fn vertex_layouts_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    vertex_layouts(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
}

#[test]
fn readbacks_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    readbacks(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the readbacks");
}

#[test]
fn instanced_walls_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    instanced_walls(&mut gpu).expect("the frame");
}

#[test]
fn depth_only_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    depth_only(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

// Every program compiles for `Gpu`, which maps each binding to an id, with
// every property per vertex and every property as a uniform, in both
// projections.
#[test]
fn every_program_compiles_for_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    for &program in ALL_BUILT_INS {
        let Some(source) = shader_source(program) else {
            continue;
        };
        let uniforms: Vec<String> =
            uniform_properties(source).into_iter().map(|name| format!("HAS_UNIFORM_u_{name}")).collect();
        for globe in [false, true] {
            let projection = globe.then(|| GLOBE_DEFINE.to_owned());
            for defines in [Vec::new(), uniforms.clone()] {
                let pixel_ratio = "DEVICE_PIXEL_RATIO=1.0".to_owned();
                let key = ShaderKey::new(program, defines.into_iter().chain(projection.clone()).chain([pixel_ratio]));
                if let Err(error) = gpu.shader(&key) {
                    panic!("{key:?}: {error}");
                }
            }
        }
    }
}

#[test]
fn custom_layers_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    custom_layers(
        &mut VulkanGpu::new(&device).expect("VulkanGpu"),
        |lent| assert!(lent.is::<crate::CustomLayerEncoder>()),
        |lent| {
            let lent = lent.downcast_mut::<crate::CustomLayerPass>().expect("a Vulkan pass");
            assert_eq!(lent.color_format, vk::Format::R8G8B8A8_UNORM);
            assert_eq!(lent.depth_stencil_format, vk::Format::UNDEFINED);
        },
    )
    .expect("the frames");
}
