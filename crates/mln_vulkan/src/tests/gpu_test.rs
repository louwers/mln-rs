use ash::vk;
use mln_gfx::gpu::conformance::{
    clip_mask_and_background, custom_layers, depth_only, foreign_ids, imported_textures, instanced_walls,
    invalid_calls, readbacks, topologies, transparent_replace, vertex_layouts,
};
use mln_gfx::gpu::{
    ColorAttachment, Gpu as _, GpuError, LoadOp, Pass, ShaderKey, TextureDesc, TextureFormat, TextureRegion,
    TextureUsage,
};
use mln_gfx::shader_group::GLOBE_DEFINE;
use mln_util::Color;
use mln_util::size::Size;

use super::glsl_compile_test::{ALL_BUILT_INS, uniform_properties};
use crate::shaders::shader_source::shader_source;
use crate::{Device, ImportedImage, VulkanGpu};

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

#[test]
fn imported_textures_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    imported_textures(&mut VulkanGpu::new(&device).expect("VulkanGpu"), |gpu, texture| {
        let imported = gpu.as_imported(texture).expect("a texture of the backend's");
        gpu.import_image(&imported)
    })
    .expect("the frames");
}

// An image of another queue family is acquired by a pass that loads it, and
// by a readback, and released after each submit that draws into it; a
// semaphore that a submit signals reaches its value once the submit is done.
#[test]
fn imported_images_go_to_their_queue_family_and_signal() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    let size = Size { width: 4, height: 4 };
    let owned = gpu
        .create_texture(&TextureDesc {
            label: "native",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        })
        .unwrap();
    let imported = gpu.as_imported(owned).unwrap();
    let external = ImportedImage { release_to: Some(vk::QUEUE_FAMILY_EXTERNAL), ..imported };
    let sampled = ImportedImage { usage: TextureUsage { sampled: true, ..imported.usage }, ..external };
    assert!(matches!(gpu.import_image(&sampled), Err(GpuError::Invalid(_))), "no draw acquires it");
    let external = gpu.import_image(&external).unwrap();
    let pass = |load| Pass {
        label: "external".to_owned(),
        color: Some(ColorAttachment { texture: external, load }),
        depth_stencil: None,
        commands: Vec::new(),
    };
    let red = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
    gpu.submit(&[pass(LoadOp::Clear(red))]).unwrap();
    gpu.submit(&[pass(LoadOp::Load), pass(LoadOp::Load)]).unwrap();
    assert_eq!(gpu.read_texels(external, TextureRegion::whole(size)).unwrap()[..4], [255, 0, 0, 255]);

    let vk_device = device.ash_device();
    let mut type_info = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
    // SAFETY: the create info is valid for the device.
    let timeline =
        unsafe { vk_device.create_semaphore(&vk::SemaphoreCreateInfo::default().push_next(&mut type_info), None) }
            .expect("a timeline semaphore");
    gpu.signal_after_next_submit(timeline, Some(7));
    gpu.submit(&[]).expect("the submit");
    let (semaphores, values) = ([timeline], [7]);
    let wait = vk::SemaphoreWaitInfo::default().semaphores(&semaphores).values(&values);
    // SAFETY: the semaphore is alive and a submit signals the value.
    unsafe { vk_device.wait_semaphores(&wait, u64::MAX) }.expect("the signal");
    gpu.wait_before_next_submit(timeline, Some(7), vk::PipelineStageFlags::ALL_COMMANDS);
    gpu.submit(&[]).expect("a submit that waits for the signal");
    gpu.finish().expect("the submits");
    // SAFETY: no submit uses the semaphore any more.
    unsafe { vk_device.destroy_semaphore(timeline, None) };
}
