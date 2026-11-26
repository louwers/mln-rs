use mln_engine::conformance::{
    clip_mask_and_background, custom_layers, depth_only, drawn_vertex_texels, foreign_ids, imported_textures,
    instanced_walls, invalid_calls, mipmapped_targets, mipmapped_targets_of, pulled_walls, readbacks, resources_in_use,
    terrain_heights, topologies, transparent_replace, vertex_layouts, wall_quads, written_vertex_texels,
};
use mln_gfx::Size;
use mln_gfx::gpu::{Gpu, TextureDesc, TextureFormat, TextureRegion, TextureUsage};

use crate::{CustomLayerEncoder, CustomLayerPass, Device, WebGpu};

#[test]
fn clip_mask_and_background_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    clip_mask_and_background(&mut WebGpu::new(&device)).expect("the frame");
}

#[test]
fn invalid_calls_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    invalid_calls(&mut WebGpu::new(&device)).expect("the valid calls");
}

#[test]
fn foreign_ids_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    foreign_ids(&mut WebGpu::new(&device), &mut WebGpu::new(&device)).expect("the valid calls");
}

#[test]
fn transparent_replace_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    transparent_replace(&mut WebGpu::new(&device)).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    topologies(&mut WebGpu::new(&device)).expect("the frames");
}

#[test]
fn depth_only_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    depth_only(&mut WebGpu::new(&device)).expect("the frame");
}

#[test]
fn written_vertex_texels_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    written_vertex_texels(&mut WebGpu::new(&device)).expect("the frame");
}

#[test]
fn drawn_vertex_texels_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    drawn_vertex_texels(&mut WebGpu::new(&device)).expect("the frames");
}

#[test]
fn terrain_heights_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    terrain_heights(&mut WebGpu::new(&device)).expect("the frames");
}

#[test]
fn mipmapped_targets_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    mipmapped_targets(&mut WebGpu::new(&device)).expect("the frames");
}

/// The mips of a format that shaders can't write, which a compute pass
/// can't make: a pass draws each level.
#[test]
fn mipmapped_targets_of_a_format_shaders_cannot_write_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    mipmapped_targets_of(&mut WebGpu::new(&device), TextureFormat::Bgra8Unorm).expect("the frames");
}

/// The mips of a device without storage textures, where a compute pass
/// can't write them: a pass draws each level. The host's device keeps
/// wgpu's handler, which panics on a validation error.
#[test]
fn mipmapped_targets_without_storage_textures_through_gpu() {
    let Some(host) = super::device() else {
        return;
    };
    let adapter = host.adapter().expect("the library's adapter");
    let limits = wgpu::Limits { max_storage_textures_per_shader_stage: 0, ..host.device().limits() };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("host device"),
        required_limits: limits,
        ..Default::default()
    }))
    .expect("device");
    let device = Device::from_wgpu(host.instance().cloned(), device, queue, None);
    mipmapped_targets(&mut WebGpu::new(&device)).expect("the frames");
}

#[test]
fn vertex_layouts_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    vertex_layouts(&mut WebGpu::new(&device)).expect("the frames");
}

#[test]
fn readbacks_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    readbacks(&mut WebGpu::new(&device)).expect("the readbacks");
}

#[test]
fn resources_in_use_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    resources_in_use(&mut WebGpu::new(&device)).expect("the frames");
}

// A frame read back each time copies into the buffer of the readback
// before, which holds what the last copy wrote.
#[test]
fn a_readback_copies_into_the_last_ones_buffer() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = WebGpu::new(&device);
    let size = Size { width: 4, height: 2 };
    let texture = gpu
        .create_texture(&TextureDesc {
            label: "read back",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { sampled: true, copy_src: true, ..TextureUsage::default() },
            mipmapped: false,
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
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = WebGpu::new(&device);
    instanced_walls(&mut gpu).expect("the frame");
}

#[test]
fn wall_quads_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = WebGpu::new(&device);
    wall_quads(&mut gpu).expect("the frame");
}

#[test]
fn pulled_walls_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
    let mut gpu = WebGpu::new(&device);
    pulled_walls(&mut gpu).expect("the frame");
}

#[test]
fn custom_layers_through_gpu() {
    let Some(device) = super::device() else {
        return;
    };
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
    let Some(device) = super::device() else {
        return;
    };
    imported_textures(&mut WebGpu::new(&device), |gpu, texture| {
        let native = gpu.native_texture(texture).expect("a texture of the backend's").clone();
        Ok(gpu.import_texture(&native))
    })
    .expect("the frames");
}
