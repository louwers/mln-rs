// Ports platform/default/include/mln/gfx/headless_backend.hpp and
// platform/default/src/mln/gfx/headless_backend.cpp
//
// `HeadlessBackend::Create(size, swapBehaviour, contextMode)` dispatches to
// `Backend::Create<Backend::DefaultType>` in C++. mln_gfx doesn't depend on
// the backends: call the `new` of the backend's own `HeadlessBackend`, such
// as `mln_webgpu::HeadlessBackend::new` or `mln_mtl::HeadlessBackend::new`.

use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::renderable::Renderable;
use crate::renderer_backend::RendererBackend;

/// `gfx::HeadlessBackend`: common headless backend interface, extends
/// `gfx::Renderable` with `readStillImage`.
pub trait HeadlessBackend: Renderable {
    fn read_still_image(&self) -> PremultipliedImage;
    fn get_renderer_backend(&self) -> &dyn RendererBackend;

    /// `setSize`: resizes and drops the resource (recreated lazily by
    /// `getDefaultRenderable`).
    fn set_size(&self, size: Size) {
        self.set_renderable_size(size);
        self.set_resource(None);
    }
}
