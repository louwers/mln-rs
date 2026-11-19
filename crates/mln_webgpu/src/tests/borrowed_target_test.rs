use mln_gfx::frame::FrameTarget as _;
use mln_gfx::{RendererBackend as _, SyncKind};
use mln_util::size::Size;

use crate::{BorrowedTarget, BorrowedTextures, Device, RendererBackend};

/// A ring of two host textures of `size`.
fn textures(device: &Device, size: Size) -> BorrowedTextures {
    let texture = || {
        device.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("host texture"),
            size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    };
    BorrowedTextures { textures: vec![texture(), texture()], size, depth_textures: Vec::new() }
}

// A target torn down with a frame begun forgets the frame's imports.
#[test]
fn a_target_torn_down_in_a_frame_forgets_its_imports() {
    let device = Device::new(None).expect("device");
    let mut backend = RendererBackend::new(&device);
    let size = Size::new(4, 4);
    let before = backend.gpu().texture_count();
    let mut target = BorrowedTarget::new(&backend, textures(&device, size), SyncKind::CpuComplete).expect("a ring");
    let gpu = backend.gpu();
    target.begin_frame(gpu).expect("a frame");
    assert!(gpu.texture_count() > before, "the frame imports its textures");
    target.tear_down(gpu);
    assert_eq!(gpu.texture_count(), before);
}
