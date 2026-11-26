use mln_engine::conformance::{
    clip_mask_and_background, custom_layers, depth_only, drawn_vertex_texels, foreign_ids, imported_textures,
    instanced_walls, invalid_calls, mipmapped_targets, pulled_walls, readbacks, resources_in_use, terrain_heights,
    topologies, transparent_replace, vertex_layouts, wall_quads, written_vertex_texels,
};
use mln_engine::shaders::{PROGRAMS, projection_variants};
use mln_gfx::gpu::{Gpu as _, GpuError};
use mln_gfx::shaders::program_parameters::ProgramParameters;
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
fn resources_in_use_through_gpu() {
    resources_in_use(&mut gpu()).expect("the frames");
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
fn pulled_walls_through_gpu() {
    pulled_walls(&mut gpu()).expect("the bindings");
}

#[test]
fn depth_only_through_gpu() {
    depth_only(&mut gpu()).expect("the frame");
}

#[test]
fn written_vertex_texels_through_gpu() {
    written_vertex_texels(&mut gpu()).expect("the frame");
}

#[test]
fn drawn_vertex_texels_through_gpu() {
    drawn_vertex_texels(&mut gpu()).expect("the frames");
}

#[test]
fn terrain_heights_through_gpu() {
    terrain_heights(&mut gpu()).expect("the frames");
}

#[test]
fn mipmapped_targets_through_gpu() {
    mipmapped_targets(&mut gpu()).expect("the frames");
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

// The memory of destroyed buffers goes back once the work before it has
// finished, and new buffers take it again.
#[test]
fn destroyed_buffers_memory_is_reused() {
    use mln_gfx::gpu::{BufferDesc, BufferUsage, ResourceId};

    let mut gpu = gpu();
    let round = |gpu: &mut D3d12Gpu| {
        let buffers: Vec<_> = [BufferUsage::Vertex, BufferUsage::Uniform]
            .into_iter()
            .flat_map(|usage| (0..500).map(move |_| BufferDesc { label: "small", usage, size: 300 }))
            .map(|desc| gpu.create_buffer(&desc).expect("a buffer"))
            .collect();
        for buffer in buffers {
            gpu.destroy(ResourceId::Buffer(buffer)).expect("the destroy");
        }
        gpu.flush(true).expect("the flush");
        gpu.slab_pages()
    };
    let first = round(&mut gpu);
    assert!(first > 0);
    assert_eq!(round(&mut gpu), first, "the second round takes the first's slabs");
}

// A uniform buffer in reused memory holds zeros in every window, so a write
// of zeros, which matches its contents, leaves zeros there.
#[test]
fn a_uniform_buffer_in_reused_memory_starts_zeroed() {
    use mln_gfx::gpu::{
        BufferDesc, BufferUsage, ColorAttachment, LoadOp, PassDesc, ResourceId, TextureDesc, TextureFormat,
        TextureUsage,
    };

    let mut gpu = gpu();
    let desc = BufferDesc { label: "uniform", usage: BufferUsage::Uniform, size: 256 };
    let first = gpu.create_buffer(&desc).expect("a buffer");
    gpu.write_buffer(first, 0, &[0xAB; 256]).expect("the write");
    gpu.flush(true).expect("the flush");
    // A pass in the next batch brings its other window up to date.
    let size = mln_gfx::Size::new(4, 4);
    let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
    let target = gpu
        .create_texture(&TextureDesc {
            label: "target",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage,
            mipmapped: false,
        })
        .expect("a texture");
    let color = Some(ColorAttachment { texture: target, load: LoadOp::Load });
    gpu.begin_pass(&PassDesc { label: "pass", color, depth_stencil: None }).expect("the pass");
    gpu.end_pass().expect("the pass's end");
    gpu.flush(true).expect("the flush");
    assert!(gpu.windows(first).iter().all(|window| window.iter().all(|&byte| byte == 0xAB)));
    gpu.destroy(ResourceId::Buffer(first)).expect("the destroy");
    gpu.flush(true).expect("the flush");
    let pages = gpu.slab_pages();
    let second = gpu.create_buffer(&desc).expect("a buffer");
    assert_eq!(gpu.slab_pages(), pages, "the second buffer takes the first's memory");
    gpu.write_buffer(second, 0, &[0; 256]).expect("the write");
    for window in gpu.windows(second) {
        assert!(window.iter().all(|&byte| byte == 0));
    }
}

// Readbacks waiting at once each copy into memory that a texture copy can
// write to, from an offset of 512 bytes.
#[test]
fn small_readbacks_waiting_at_once_read_their_rows() {
    use mln_gfx::Size;
    use mln_gfx::gpu::{TextureDesc, TextureFormat, TextureRegion, TextureUsage};

    let mut gpu = gpu();
    let size = Size::new(4, 4);
    let usage = TextureUsage { sampled: true, copy_src: true, ..TextureUsage::default() };
    let texture = gpu
        .create_texture(&TextureDesc {
            label: "rows",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage,
            mipmapped: false,
        })
        .expect("a texture");
    let texels: Vec<u8> = (0..64).collect();
    gpu.write_texture(texture, TextureRegion::whole(size), &texels).expect("the write");
    let row = |y| TextureRegion { x: 0, y, size: Size::new(4, 1) };
    let readbacks: Vec<_> = (0..4).map(|y| gpu.read_texture(texture, row(y)).expect("a readback")).collect();
    for (y, readback) in readbacks.into_iter().enumerate() {
        let read = gpu.take_readback(readback, true).expect("the texels").expect("waited for");
        assert_eq!(read, texels[y * 16..(y + 1) * 16], "row {y}");
    }
}

// A texture's wait holds back only the submit of a pass that draws into it:
// a pass into another texture runs at once, and the wait, once forgotten,
// holds back nothing.
#[test]
fn a_textures_wait_holds_back_only_its_passes() {
    use mln_gfx::Size;
    use mln_gfx::gpu::{ColorAttachment, LoadOp, PassDesc, TextureDesc, TextureFormat, TextureUsage};
    use windows::Win32::Graphics::Direct3D12::{D3D12_FENCE_FLAG_NONE, ID3D12Fence};

    let mut gpu = gpu();
    let texture = |gpu: &mut D3d12Gpu| {
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let desc = TextureDesc {
            label: "target",
            size: Size::new(4, 4),
            format: TextureFormat::Rgba8Unorm,
            usage,
            mipmapped: false,
        };
        gpu.create_texture(&desc).expect("a texture")
    };
    let (held, free) = (texture(&mut gpu), texture(&mut gpu));
    // SAFETY: a plain call of the live device.
    let host: ID3D12Fence = unsafe { gpu.device().device().CreateFence(0, D3D12_FENCE_FLAG_NONE) }.unwrap();
    gpu.wait_before_using(held, host.clone(), 1);
    let clear = |gpu: &mut D3d12Gpu, texture| {
        let color = Some(ColorAttachment { texture, load: LoadOp::Clear(mln_gfx::Rgba::new(1.0, 0.0, 0.0, 1.0)) });
        gpu.begin_pass(&PassDesc { label: "clear", color, depth_stencil: None }).unwrap();
        gpu.end_pass().unwrap();
        gpu.flush(false).unwrap();
    };
    // Whether the work submitted so far finishes within a second.
    let finishes = |gpu: &D3d12Gpu| {
        let (fence, value) = gpu.last_submit();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        // SAFETY: a plain query.
        while unsafe { fence.GetCompletedValue() } < value {
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::yield_now();
        }
        true
    };
    clear(&mut gpu, free);
    assert!(finishes(&gpu), "a pass into another texture waited for the host");
    gpu.forget_waits(held);
    clear(&mut gpu, held);
    assert!(finishes(&gpu), "a forgotten wait held the pass back");
    gpu.wait_before_using(held, host.clone(), 1);
    clear(&mut gpu, held);
    assert!(!finishes(&gpu), "the pass didn't wait for the host");
    // SAFETY: a plain call of the live fence.
    unsafe { host.Signal(1) }.unwrap();
    assert!(finishes(&gpu));
}
