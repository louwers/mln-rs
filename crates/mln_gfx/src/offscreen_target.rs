//! Frames drawn into textures of the library's and read back, which headless
//! rendering and render tests draw into.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::attachment::Attachment;
use crate::backend_scope::{BackendScope, ScopeType};
use crate::context::Context;
use crate::frame::{FrameImage, FrameTarget, Handoff};
use crate::gpu::{GpuError, TargetFormats, TextureFormat, TextureRegion};
use crate::renderer_backend::RendererBackend;

/// The format of an offscreen frame's color, which
/// [`read_image`](OffscreenTarget::read_image) reads as it is.
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// Frames drawn into a color texture and a depth and stencil one of the
/// backend's `Gpu`, which [`read_image`](Self::read_image) reads back.
pub struct OffscreenTarget<B: RendererBackend> {
    backend: Rc<B>,
    size: Cell<Size>,
    /// The textures, made with the first frame of a size.
    image: RefCell<Option<Image>>,
}

/// A frame's color and depth-stencil textures, which the rendering stats
/// count as a framebuffer while they live.
struct Image {
    color: Attachment,
    depth_stencil: Attachment,
    context: Weak<dyn Context>,
}

impl Drop for Image {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            context.rendering_stats().num_frame_buffers -= 1;
        }
    }
}

impl<B: RendererBackend> OffscreenTarget<B> {
    /// A target of `size` physical pixels on `backend`.
    pub fn new(backend: Rc<B>, size: Size) -> Self {
        OffscreenTarget { backend, size: Cell::new(size), image: RefCell::new(None) }
    }

    /// The backend the target renders on.
    pub fn backend(&self) -> &Rc<B> {
        &self.backend
    }

    /// Draws the next frame at `size`, and drops the textures of the last
    /// one.
    pub fn resize(&self, size: Size) {
        if self.size.replace(size) != size {
            self.image.borrow_mut().take();
        }
    }

    /// The last frame's pixels: premultiplied RGBA, in rows from the top.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) before
    /// the first frame of the size, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) where the backend reads
    /// nothing back.
    pub fn read_image(&self) -> Result<PremultipliedImage, Error> {
        let _scope = BackendScope::new(&*self.backend, ScopeType::Explicit);
        let image = self.image.borrow();
        let color = &image
            .as_ref()
            .ok_or_else(|| Error::invalid_state("the target has no frame since it was made or resized"))?
            .color;
        let texels = self
            .backend
            .get_context()
            .gpu()
            .read_texels(color.id(), TextureRegion::whole(color.size()))
            .map_err(|error| Error::native(format!("reading the frame back failed: {error}")))?;
        Ok(PremultipliedImage::from_vec(color.size(), texels))
    }

    fn make_image(&self, size: Size) -> Result<Image, GpuError> {
        let context = self.backend.get_context_ptr();
        let color = Attachment::new(&**context, "offscreen color", size, COLOR, true)?;
        let depth_stencil = Attachment::new(&**context, "offscreen depth and stencil", size, DEPTH_STENCIL, false)?;
        context.rendering_stats().num_frame_buffers += 1;
        Ok(Image { color, depth_stencil, context: Rc::downgrade(context) })
    }
}

impl<B: RendererBackend> FrameTarget for OffscreenTarget<B> {
    fn begin_frame(&self) -> Option<FrameImage> {
        let size = self.size.get();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let mut image = self.image.borrow_mut();
        if image.is_none() {
            match self.make_image(size) {
                Ok(made) => *image = Some(made),
                Err(error) => {
                    Log::error(Event::Render, format!("Making an offscreen frame's textures failed: {error}"));
                    return None;
                }
            }
        }
        let image = image.as_ref()?;
        Some(FrameImage {
            size,
            color: image.color.id(),
            depth_stencil: Some(image.depth_stencil.id()),
            targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
        })
    }

    /// The frame is read back when it is read.
    fn end_frame(&self) -> Handoff {
        if self.image.borrow().is_some() { Handoff::ReadBack } else { Handoff::Dropped }
    }
}
