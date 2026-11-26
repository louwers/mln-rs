//! The textures that the GL targets' frames draw into, imported into the
//! backend's `GlGpu` for a frame.

use mln_gfx::Size;
use mln_gfx::frame::FrameImage;
use mln_gfx::gpu::{ResourceId, TargetFormats, TextureFormat, destroy_unused};

use crate::gpu::GlGpu;

/// The format the frames of a framebuffer made elsewhere are named by.
pub(crate) const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
pub(crate) const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// The textures of the frame begun, imported into the backend's `GlGpu`
/// until it ends, and whether its depth and stencil texture is imported
/// too, rather than the target's own.
#[derive(Default)]
pub(crate) struct FrameImports(Option<(FrameImage, bool)>);

impl FrameImports {
    /// The frame's textures: those of `import`, at the frame's first call.
    pub(crate) fn get_or_import(&mut self, import: impl FnOnce() -> Option<(FrameImage, bool)>) -> Option<FrameImage> {
        if let Some((image, _)) = self.0 {
            return Some(image);
        }
        Some(self.0.insert(import()?).0)
    }

    /// The textures of the frame begun, if any.
    #[cfg(any(windows, all(unix, not(target_vendor = "apple"))))]
    pub(crate) fn get(&self) -> Option<FrameImage> {
        self.0.map(|(image, _)| image)
    }

    /// Forgets the frame's imported textures in `gpu`, at its end. Returns
    /// whether there were any.
    pub(crate) fn release(&mut self, gpu: &mut GlGpu) -> bool {
        let Some((image, depth_imported)) = self.0.take() else {
            return false;
        };
        let depth_stencil = image.depth_stencil.filter(|_| depth_imported);
        let textures = [Some(image.color), depth_stencil].into_iter().flatten();
        destroy_unused(gpu, textures.map(ResourceId::Texture));
        true
    }
}

/// The default framebuffer of the surface the context is current on, of
/// `size`, with its depth and stencil, imported into `gpu`.
pub(crate) fn default_framebuffer(gpu: &mut GlGpu, size: Size) -> Option<(FrameImage, bool)> {
    if size.width == 0 || size.height == 0 {
        return None;
    }
    let imported = gpu.import_framebuffer(None, size, Some(COLOR), Some(DEPTH_STENCIL));
    let image = FrameImage {
        size,
        color: imported.color?,
        depth_stencil: imported.depth_stencil,
        targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
    };
    Some((image, true))
}
