use mln_gfx::gpu::conformance::{
    clip_mask_and_background, depth_only, foreign_ids, invalid_calls, topologies, transparent_replace, vertex_layouts,
};
use mln_gfx::gpu::{Gpu, GpuError, ShaderKey, TextureDesc, TextureFormat, TextureRegion, TextureUsage};
use mln_gfx::shader_group::GLOBE_DEFINE;
use mln_util::size::Size;

use super::glsl_compile_test::{ALL_BUILT_INS, uniform_properties};
use crate::shaders::shader_source::shader_source;
use crate::{Device, VulkanGpu};

#[test]
fn clip_mask_and_background_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    clip_mask_and_background(&mut gpu, |gpu, texture| gpu.read_texture(texture).expect("readback")).expect("the frame");
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
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    transparent_replace(&mut gpu, |gpu, texture| gpu.read_texture(texture).expect("readback")).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    topologies(&mut gpu, |gpu, texture| gpu.read_texture(texture).expect("readback")).expect("the frames");
}

#[test]
fn vertex_layouts_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    vertex_layouts(&mut gpu, |gpu, texture| gpu.read_texture(texture).expect("readback")).expect("the frames");
}

#[test]
fn depth_only_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    depth_only(&mut gpu, |gpu, texture| gpu.read_texture(texture).expect("readback")).expect("the frame");
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

// A texture written whole, then in part, reads back both writes in order,
// and is gone once destroyed.
#[test]
fn texture_writes_take_effect_in_order() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    let size = Size { width: 4, height: 2 };
    let texture = gpu
        .create_texture(&TextureDesc {
            label: "written",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { sampled: true, copy_src: true, ..TextureUsage::default() },
        })
        .unwrap();
    let whole = TextureRegion { x: 0, y: 0, size };
    gpu.write_texture(texture, whole, &[1; 4 * 4 * 2]).unwrap();
    gpu.write_texture(texture, TextureRegion { x: 1, y: 1, size: Size { width: 2, height: 1 } }, &[2; 4 * 2]).unwrap();
    let texels = gpu.read_texture(texture).unwrap();
    let mut expected = vec![1; 4 * 4 * 2];
    expected[4 * 5..4 * 7].fill(2);
    assert_eq!(texels, expected);

    assert!(matches!(gpu.write_texture(texture, whole, &[0; 4]), Err(GpuError::Invalid(_))));
    gpu.destroy_texture(texture);
    gpu.submit(&[]).unwrap();
    assert!(matches!(gpu.write_texture(texture, whole, &[1; 4 * 4 * 2]), Err(GpuError::UnknownId("texture"))));
}
