#![cfg(target_vendor = "apple")]

// Ports test/renderer/metal_renderable.test.cpp

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::headless_backend::HeadlessBackend as _;
use mln_gfx::renderable::SwapBehaviour;
use mln_gfx::renderer_backend::ContextMode;
use mln_mtl::HeadlessBackend;
use mln_mtl::renderable_resource::as_mtl;
use mln_util::size::Size;

// Each frame an upload pass and then a render pass bind the headless renderable, and only the
// swap after the render pass commits. If the second bind replaces the pending command buffer, one
// buffer per frame is never committed, and after about 64 frames (the queue's default pool) the
// next commandBuffer() call blocks forever.
// gtest: MetalRenderable.BindKeepsThePendingCommandBuffer
#[test]
fn bind_keeps_the_pending_command_buffer() {
    let backend =
        HeadlessBackend::new(Size::new(256, 256), SwapBehaviour::NoFlush, ContextMode::Unique).expect("Metal device");
    let _scope = BackendScope::new(backend.get_renderer_backend(), ScopeType::Explicit);
    let renderable = backend.get_renderer_backend().get_default_renderable();
    let resource = renderable.get_resource().expect("the default renderable's resource");
    let resource = as_mtl(&*resource).expect("a Metal renderable resource");

    resource.bind();
    let pending = resource.get_command_buffer().expect("a pending command buffer");

    resource.bind();
    let rebound = resource.get_command_buffer().expect("a pending command buffer");
    assert!(std::ptr::eq(&*rebound, &*pending));

    resource.swap();
    resource.bind();
    assert!(resource.get_command_buffer().is_some());
    resource.swap();
}
