use mln_gfx::gpu::conformance::{
    clip_mask_and_background, custom_layers, depth_only, foreign_ids, imported_textures, instanced_walls,
    invalid_calls, readbacks, topologies, transparent_replace, vertex_layouts, wall_quads,
};
use mln_gfx::gpu::{Gpu as _, GpuError, ShaderKey};
use mln_gfx::shader_group::GLOBE_DEFINE;
use mln_glsl::PROGRAMS;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN};

use crate::{D3d12Gpu, Device};

fn gpu() -> D3d12Gpu {
    let device = Device::new(None).expect("a D3D12 device");
    D3d12Gpu::new(&device).expect("D3d12Gpu")
}

#[test]
fn clip_mask_and_background_through_gpu() {
    clip_mask_and_background(&mut gpu()).expect("the frame");
}

#[test]
fn invalid_calls_through_gpu() {
    invalid_calls(&mut gpu()).expect("the valid calls");
}

#[test]
fn foreign_ids_through_gpu() {
    let device = Device::new(None).expect("a D3D12 device");
    let mut first = D3d12Gpu::new(&device).expect("D3d12Gpu");
    let mut second = D3d12Gpu::new(&device).expect("D3d12Gpu");
    foreign_ids(&mut first, &mut second).expect("the valid calls");
}

#[test]
fn transparent_replace_through_gpu() {
    transparent_replace(&mut gpu()).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    topologies(&mut gpu()).expect("the frames");
}

#[test]
fn vertex_layouts_through_gpu() {
    vertex_layouts(&mut gpu()).expect("the frames");
}

#[test]
fn readbacks_through_gpu() {
    readbacks(&mut gpu()).expect("the readbacks");
}

#[test]
fn instanced_walls_through_gpu() {
    instanced_walls(&mut gpu()).expect("the frame");
}

#[test]
fn wall_quads_through_gpu() {
    wall_quads(&mut gpu()).expect("the frame");
}

#[test]
fn depth_only_through_gpu() {
    depth_only(&mut gpu()).expect("the frame");
}

#[test]
fn custom_layers_through_gpu() {
    custom_layers(
        &mut gpu(),
        |lent| assert!(lent.is::<crate::CustomLayerEncoder>()),
        |lent| {
            let lent = lent.downcast_mut::<crate::CustomLayerPass>().expect("a D3D12 pass");
            assert_eq!(lent.color_format, DXGI_FORMAT_R8G8B8A8_UNORM);
            assert_eq!(lent.depth_stencil_format, DXGI_FORMAT_UNKNOWN);
        },
    )
    .expect("the frames");
}

#[test]
fn imported_textures_through_gpu() {
    imported_textures(&mut gpu(), |gpu, texture| {
        let (resource, state) = gpu.native_texture(texture).ok_or(GpuError::UnknownId("texture"))?;
        let resource = resource.clone();
        gpu.import_texture(&resource, state)
    })
    .expect("the frames");
}

// Every program compiles for `Gpu`, which maps each binding to a root
// parameter, with every property per vertex and every property as a
// uniform, in both projections.
#[test]
fn every_program_compiles_for_gpu() {
    let mut gpu = gpu();
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
