use std::rc::Rc;

use glow::HasContext as _;
use mln_gfx::frame::{FrameTarget as _, Handoff};
use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu as _, LoadOp, PassDesc, TextureId};
use mln_gfx::{ConsumerSync, GpuSync, SyncKind};
use mln_util::Color;
use mln_util::size::Size;

use super::gpu_test::with_device;
use crate::borrowed_target::{BorrowedTarget, BorrowedTextures};
use crate::renderer_backend::RendererBackend;

/// A ring of `count` host textures of `size`, made on `backend`'s context.
fn textures(backend: &RendererBackend, count: usize, size: Size) -> BorrowedTextures {
    let textures = backend.gpu().with_gl(|gl| {
        (0..count)
            .map(|_| {
                // SAFETY: the context is current, and the texture is new.
                unsafe {
                    let texture = gl.create_texture().expect("a texture");
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, size.width as i32, size.height as i32);
                    texture.0.get()
                }
            })
            .collect()
    });
    BorrowedTextures { textures, size, depth_textures: Vec::new() }
}

/// Draws a frame into `slot` and gives it back. Returns the textures the
/// frame drew into.
fn draw(backend: &RendererBackend, target: &BorrowedTarget, slot: usize) -> (TextureId, Option<TextureId>) {
    target.select(slot);
    let image = target.begin_frame().expect("a frame");
    let mut gpu = backend.gpu();
    gpu.begin_pass(&PassDesc {
        label: "ring",
        color: Some(ColorAttachment { texture: image.color, load: LoadOp::Clear(Color::red()) }),
        depth_stencil: image.depth_stencil.map(|texture| DepthStencilAttachment {
            texture,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
    })
    .expect("the frame");
    gpu.end_pass().expect("the frame");
    gpu.flush(false).expect("the frame");
    drop(gpu);
    assert!(matches!(target.end_frame(), Handoff::GivenBack { slot: given, .. } if given == slot));
    target.release(slot, ConsumerSync::of(GpuSync::of(SyncKind::CpuComplete))).expect("the release");
    (image.color, image.depth_stencil)
}

// Each slot keeps its imports, and the `GlGpu` the framebuffer of them, from
// one frame to the next, until the ring's textures are replaced or the
// target goes.
#[test]
fn a_ring_keeps_each_slots_framebuffer() {
    let Some(backend) = with_device(|device| RendererBackend::new(device).map(Rc::new)) else {
        return;
    };
    let size = Size::new(4, 4);
    let target =
        BorrowedTarget::new(Rc::clone(&backend), textures(&backend, 2, size), SyncKind::CpuComplete).expect("a ring");
    let first: Vec<_> = (0..2).map(|slot| draw(&backend, &target, slot)).collect();
    assert_eq!(backend.gpu().framebuffer_count(), 2);
    let second: Vec<_> = (0..2).map(|slot| draw(&backend, &target, slot)).collect();
    assert_eq!(first, second, "the slots draw into the textures of their first frames");
    assert_eq!(backend.gpu().framebuffer_count(), 2, "the slots draw through the framebuffers of their first frames");

    target.replace(textures(&backend, 2, size)).expect("new textures");
    assert_eq!(backend.gpu().framebuffer_count(), 0, "the old textures' framebuffers go with them");
    draw(&backend, &target, 0);
    assert_eq!(backend.gpu().framebuffer_count(), 1);
    drop(target);
    assert_eq!(backend.gpu().framebuffer_count(), 0, "the framebuffers go with the target");
}
