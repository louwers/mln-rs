use std::cell::Cell;
use std::rc::Rc;

use ash::vk;
use mln_engine::conformance::{
    clip_mask_and_background, custom_layers, depth_only, drawn_vertex_texels, foreign_ids, imported_textures,
    instanced_walls, invalid_calls, mipmapped_targets, pulled_walls, readbacks, resources_in_use, terrain_heights,
    topologies, transparent_replace, vertex_layouts, wall_quads, written_vertex_texels,
};
use mln_engine::shaders::{PROGRAMS, projection_variants};
use mln_gfx::gpu::{
    ColorAttachment, Gpu as _, GpuError, LoadOp, PassDesc, TextureDesc, TextureFormat, TextureRegion, TextureUsage,
    read_texels,
};
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::{Rgba, Size};

use crate::{ImportedImage, VulkanGpu};

#[test]
fn clip_mask_and_background_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    clip_mask_and_background(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

#[test]
fn resources_in_use_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    resources_in_use(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
}

#[test]
fn invalid_calls_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    invalid_calls(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the valid calls");
}

#[test]
fn foreign_ids_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    let mut first = VulkanGpu::new(&device).expect("VulkanGpu");
    let mut second = VulkanGpu::new(&device).expect("VulkanGpu");
    foreign_ids(&mut first, &mut second).expect("the valid calls");
}

#[test]
fn transparent_replace_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    transparent_replace(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    topologies(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
}

#[test]
fn vertex_layouts_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    vertex_layouts(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
}

#[test]
fn readbacks_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    readbacks(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the readbacks");
}

#[test]
fn instanced_walls_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    instanced_walls(&mut gpu).expect("the frame");
}

#[test]
fn wall_quads_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    wall_quads(&mut gpu).expect("the frame");
}

#[test]
fn pulled_walls_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    pulled_walls(&mut gpu).expect("the bindings");
}

#[test]
fn depth_only_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    depth_only(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

#[test]
fn written_vertex_texels_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    written_vertex_texels(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

#[test]
fn drawn_vertex_texels_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    drawn_vertex_texels(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
}

#[test]
fn terrain_heights_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    terrain_heights(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
}

#[test]
fn mipmapped_targets_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    mipmapped_targets(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
}

// What `destroy_later` destroys waits for the next flush, or goes with the
// `Gpu`.
#[test]
fn destroy_later_waits_for_the_next_submit() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    let destroyed = Rc::new(Cell::new(0));
    let count = |destroyed: &Rc<Cell<u32>>| {
        let destroyed = Rc::clone(destroyed);
        move |_: &ash::Device| destroyed.set(destroyed.get() + 1)
    };
    gpu.destroy_later(count(&destroyed));
    assert_eq!(destroyed.get(), 0, "before the flush");
    gpu.flush(true).expect("the flush");
    assert_eq!(destroyed.get(), 1, "after the flush");
    gpu.destroy_later(count(&destroyed));
    drop(gpu);
    assert_eq!(destroyed.get(), 2, "with the Gpu");
}

// Every program compiles for `Gpu`, which maps each binding to an id, with
// every property per vertex and every property as a uniform, in both
// projections.
#[test]
fn every_program_compiles_for_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    let parameters = ProgramParameters::new(1.0);
    for program in PROGRAMS.iter().copied().filter(|program| program.sources.glsl.is_some()) {
        for &variant in projection_variants(program) {
            for uniforms in [&[][..], program.properties] {
                let key = program.variant_key(&parameters, uniforms, variant);
                if let Err(error) = gpu.create_shader(&key) {
                    panic!("{key:?}: {error}");
                }
            }
        }
    }
}

#[test]
fn custom_layers_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
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
    let Some(device) = super::device() else {
        return;
    };
    imported_textures(&mut VulkanGpu::new(&device).expect("VulkanGpu"), |gpu, texture| {
        let imported = gpu.as_imported(texture).expect("a texture of the backend's");
        gpu.import_image(&imported)
    })
    .expect("the frames");
}

// An image of another queue family is acquired by a pass that loads it, and
// by a readback, and released after each flush that draws into it; a
// semaphore that a flush signals reaches its value once the flush is done.
#[test]
fn imported_images_go_to_their_queue_family_and_signal() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    let size = Size { width: 4, height: 4 };
    let owned = gpu
        .create_texture(&TextureDesc {
            label: "native",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
            mipmapped: false,
        })
        .unwrap();
    let imported = gpu.as_imported(owned).unwrap();
    let external = ImportedImage { release_to: Some(vk::QUEUE_FAMILY_EXTERNAL), ..imported };
    let sampled = ImportedImage { usage: TextureUsage { sampled: true, ..imported.usage }, ..external };
    assert!(matches!(gpu.import_image(&sampled), Err(GpuError::Invalid(_))), "no draw acquires it");
    let external = gpu.import_image(&external).unwrap();
    let passes = |gpu: &mut VulkanGpu, loads: &[LoadOp<Rgba>]| {
        for &load in loads {
            let color = Some(ColorAttachment { texture: external, load });
            gpu.begin_pass(&PassDesc { label: "external", color, depth_stencil: None })?;
            gpu.end_pass()?;
        }
        gpu.flush(false)
    };
    let red = Rgba::new(1.0, 0.0, 0.0, 1.0);
    passes(&mut gpu, &[LoadOp::Clear(red)]).unwrap();
    passes(&mut gpu, &[LoadOp::Load, LoadOp::Load]).unwrap();
    assert_eq!(read_texels(&mut gpu, external, TextureRegion::whole(size)).unwrap()[..4], [255, 0, 0, 255]);

    let vk_device = device.ash_device();
    let mut type_info = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
    // SAFETY: the create info is valid for the device.
    let timeline =
        unsafe { vk_device.create_semaphore(&vk::SemaphoreCreateInfo::default().push_next(&mut type_info), None) }
            .expect("a timeline semaphore");
    gpu.signal_after_next_submit(timeline, Some(7));
    gpu.flush(false).expect("the flush");
    let (semaphores, values) = ([timeline], [7]);
    let wait = vk::SemaphoreWaitInfo::default().semaphores(&semaphores).values(&values);
    // SAFETY: the semaphore is alive and a submit signals the value.
    unsafe { vk_device.wait_semaphores(&wait, u64::MAX) }.expect("the signal");
    gpu.wait_before_using(external, timeline, Some(7), vk::PipelineStageFlags::ALL_COMMANDS).unwrap();
    passes(&mut gpu, &[LoadOp::Load]).expect("a flush that waits for the signal");
    gpu.flush(true).expect("the flushes");
    // SAFETY: no submit uses the semaphore any more.
    unsafe { vk_device.destroy_semaphore(timeline, None) };
}
