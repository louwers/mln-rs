use std::rc::Rc;

use mln_gfx::SyncKind;
use mln_gfx::frame::FrameTarget as _;
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

// A target dropped with a frame begun forgets the frame's imports.
#[test]
fn a_target_dropped_in_a_frame_forgets_its_imports() {
    let device = Device::new(None).expect("device");
    let backend = Rc::new(RendererBackend::new(&device));
    let size = Size::new(4, 4);
    let before = backend.gpu().texture_count();
    let target = BorrowedTarget::new(Rc::clone(&backend), textures(&device, size), SyncKind::CpuComplete)
        .expect("a ring");
    target.begin_frame().expect("a frame");
    assert!(backend.gpu().texture_count() > before, "the frame imports its textures");
    drop(target);
    assert_eq!(backend.gpu().texture_count(), before);
}
