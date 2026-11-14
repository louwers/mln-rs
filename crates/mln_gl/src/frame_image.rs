//! The textures that the GL targets' frames draw into, imported into the
//! backend's `GlGpu` for a frame.

use std::cell::Cell;

use mln_gfx::frame::FrameImage;
use mln_gfx::gpu::{Gpu as _, TargetFormats, TextureFormat};
use mln_util::size::Size;

use crate::renderer_backend::RendererBackend;

/// The format the frames of a framebuffer made elsewhere are named by.
pub(crate) const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
pub(crate) const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The textures of the frame begun, imported into the backend's `GlGpu`
/// until it ends, and whether its depth and stencil texture is imported
/// too, rather than the target's own.
#[derive(Default)]
pub(crate) struct FrameImports(Cell<Option<(FrameImage, bool)>>);

impl FrameImports {
    /// The frame's textures: those of `import`, at the frame's first call.
    pub(crate) fn get_or_import(&self, import: impl FnOnce() -> Option<(FrameImage, bool)>) -> Option<FrameImage> {
        if let Some((image, _)) = self.0.get() {
            return Some(image);
        }
        let imported = import()?;
        self.0.set(Some(imported));
        Some(imported.0)
    }

    /// The textures of the frame begun, if any.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub(crate) fn get(&self) -> Option<FrameImage> {
        self.0.get().map(|(image, _)| image)
    }

    /// Forgets the frame's imported textures, at its end. Returns whether
    /// there were any. Call it with the context current.
    pub(crate) fn release(&self, backend: &RendererBackend) -> bool {
        let Some((image, depth_imported)) = self.0.take() else {
            return false;
        };
        if let Some(context) = backend.gl_context() {
            let mut gpu = context.gl_gpu_mut();
            let depth_stencil = image.depth_stencil.filter(|_| depth_imported);
            for texture in [Some(image.color), depth_stencil].into_iter().flatten() {
                gpu.destroy_texture(texture);
            }
        }
        true
    }
}

/// The default framebuffer of the surface the context is current on, of
/// `size`, with its depth and stencil, imported into the backend's `GlGpu`.
pub(crate) fn default_framebuffer(backend: &RendererBackend, size: Size) -> Option<(FrameImage, bool)> {
    if size.width == 0 || size.height == 0 {
        return None;
    }
    let context = backend.gl_context()?;
    let imported = context.gl_gpu_mut().import_framebuffer(None, size, Some(COLOR), Some(DEPTH_STENCIL));
    let image = FrameImage {
        size,
        color: imported.color?,
        depth_stencil: imported.depth_stencil,
        targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
    };
    Some((image, true))
}
