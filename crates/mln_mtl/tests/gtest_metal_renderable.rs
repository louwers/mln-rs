#![cfg(target_vendor = "apple")]

use std::rc::Rc;

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::frame::FrameTarget as _;
use mln_mtl::draw_target::{TargetResource as _, downcast_resource};
use mln_mtl::offscreen_texture::OffscreenTextureResource;
use mln_mtl::{Device, OffscreenTarget, RendererBackend};
use mln_util::size::Size;

// Each frame an upload pass and then a render pass bind the offscreen target's image, and only
// the commit after the render pass commits. If the second bind replaces the pending command buffer, one
// buffer per frame is never committed, and after about 64 frames (the queue's default pool) the
// next commandBuffer() call blocks forever.
// gtest: MetalRenderable.BindKeepsThePendingCommandBuffer
#[test]
fn bind_keeps_the_pending_command_buffer() {
    let backend = Rc::new(RendererBackend::new(&Device::new(None).expect("Metal device")));
    let target = OffscreenTarget::new(backend.clone(), Size::new(256, 256));
    let _scope = BackendScope::new(&*backend, ScopeType::Explicit);
    let image = target.begin_frame().expect("a frame");
    let resource = downcast_resource::<OffscreenTextureResource>(&*image).expect("a Metal offscreen texture");

    resource.bind();
    let pending = resource.get_command_buffer().expect("a pending command buffer");

    resource.bind();
    let rebound = resource.get_command_buffer().expect("a pending command buffer");
    assert!(std::ptr::eq(&raw const *rebound, &raw const *pending));

    resource.commit();
    resource.bind();
    assert!(resource.get_command_buffer().is_some());
    resource.commit();
}
