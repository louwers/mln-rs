use std::cell::Cell;
use std::rc::Rc;

use ash::vk;
use mln_gfx::gpu::conformance::{
    clip_mask_and_background, custom_layers, depth_only, foreign_ids, imported_textures, instanced_walls,
    invalid_calls, readbacks, resources_in_use, topologies, transparent_replace, vertex_layouts, wall_quads,
};
use mln_gfx::gpu::{
    ColorAttachment, Gpu as _, GpuError, LoadOp, PassDesc, ShaderKey, TextureDesc, TextureFormat, TextureRegion,
    TextureUsage, read_texels,
};
use mln_gfx::shaders::program_info::GLOBE_DEFINE;
use mln_glsl::PROGRAMS;
use mln_util::Color;
use mln_util::size::Size;

use crate::{Device, ImportedImage, VulkanGpu};

#[test]
fn clip_mask_and_background_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    clip_mask_and_background(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

#[test]
fn resources_in_use_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    resources_in_use(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frames");
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
fn wall_quads_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    wall_quads(&mut gpu).expect("the frame");
}

#[test]
fn depth_only_through_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    depth_only(&mut VulkanGpu::new(&device).expect("VulkanGpu")).expect("the frame");
}

// Every program compiles for `Gpu`, which maps each binding to an id, with
// every property per vertex and every property as a uniform, in both
// projections.
// What `destroy_later` destroys waits for the next flush, or goes with the
// `Gpu`.
#[test]
fn destroy_later_waits_for_the_next_submit() {
    let device = Device::new(None).expect("Vulkan device");
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

// A destroyed buffer whose windows aren't all current stays while bindings
// hold it, and goes with the last of them though no frame follows. Debug
// builds refuse to destroy a buffer that bindings hold.
#[cfg(not(debug_assertions))]
#[test]
fn destroyed_stale_buffers_go_with_their_last_bindings() {
    use mln_gfx::gpu::{BindGroupEntry, BindingsDesc, BufferDesc, BufferUsage};
    use mln_gfx::shaders::background_layer_ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
    use mln_gfx::shaders::layer_ubo::ProjectionUBO;
    use mln_gfx::shaders::shader_defines::{ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_PROPS_UBO, ID_PROJECTION_UBO};
    use mln_gfx::shaders::shader_source::BuiltIn;

    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    let shader = gpu.create_shader(&ShaderKey::new(BuiltIn::BackgroundShader, [])).expect("the shader");
    let mut buffers = Vec::new();
    let mut entry = |id: usize, usage, size: usize| {
        let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage, size: size as u64 }).expect("a buffer");
        gpu.write_buffer(buffer, 0, &vec![1; size]).expect("the write");
        buffers.push(buffer);
        BindGroupEntry::Buffer { id: id as u32, buffer, offset: 0, size: size as u64 }
    };
    let entries = [
        entry(ID_PROJECTION_UBO, BufferUsage::Storage, size_of::<ProjectionUBO>()),
        entry(ID_BACKGROUND_DRAWABLE_UBO, BufferUsage::Storage, size_of::<BackgroundDrawableUnionUBO>()),
        entry(ID_BACKGROUND_PROPS_UBO, BufferUsage::Uniform, size_of::<BackgroundPropsUBO>()),
    ];
    let bindings =
        gpu.create_bindings(&BindingsDesc { label: "background", shader, entries: &entries }).expect("the bindings");
    for buffer in buffers {
        gpu.destroy(buffer.into()).expect("the destroy");
    }
    assert_eq!(gpu.destroyed_stale_count(), 3, "while the bindings hold them");
    gpu.destroy(bindings.into()).expect("the destroy");
    gpu.flush(true).expect("the flush");
    assert_eq!(gpu.destroyed_stale_count(), 0, "after the bindings go");
}

#[test]
fn every_program_compiles_for_gpu() {
    let device = Device::new(None).expect("Vulkan device");
    let mut gpu = VulkanGpu::new(&device).expect("VulkanGpu");
    for &(program, source) in PROGRAMS {
        let uniforms: Vec<String> =
            source.uniform_properties().into_iter().map(|name| format!("HAS_UNIFORM_u_{name}")).collect();
        for globe in [false, true] {
            let projection = globe.then(|| GLOBE_DEFINE.to_owned());
            for defines in [Vec::new(), uniforms.clone()] {
                let pixel_ratio = "DEVICE_PIXEL_RATIO=1.0".to_owned();
                let key = ShaderKey::new(program, defines.into_iter().chain(projection.clone()).chain([pixel_ratio]));
                if let Err(error) = gpu.create_shader(&key) {
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
// by a readback, and released after each flush that draws into it; a
// semaphore that a flush signals reaches its value once the flush is done.
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
    let passes = |gpu: &mut VulkanGpu, loads: &[LoadOp<Color>]| {
        for &load in loads {
            let color = Some(ColorAttachment { texture: external, load });
            gpu.begin_pass(&PassDesc { label: "external", color, depth_stencil: None })?;
            gpu.end_pass()?;
        }
        gpu.flush(false)
    };
    let red = Color::red();
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
