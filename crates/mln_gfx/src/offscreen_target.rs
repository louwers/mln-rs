//! Frames drawn into textures of the library's and read back, which headless
//! rendering and render tests draw into.

use std::cell::Cell;
use std::rc::Rc;

use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::frame::{FrameImage, FrameTarget, Handoff};
use crate::gpu::{GpuError, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage};
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
    image: Cell<Option<Image>>,
}

/// A frame's color and depth-stencil textures, of `size`.
#[derive(Clone, Copy)]
struct Image {
    size: Size,
    color: TextureId,
    depth_stencil: TextureId,
}

impl<B: RendererBackend> OffscreenTarget<B> {
    /// A target of `size` physical pixels on `backend`.
    pub fn new(backend: Rc<B>, size: Size) -> Self {
        OffscreenTarget { backend, size: Cell::new(size), image: Cell::new(None) }
    }

    /// The backend the target renders on.
    pub fn backend(&self) -> &Rc<B> {
        &self.backend
    }

    /// Draws the next frame at `size`, and drops the textures of the last
    /// one.
    pub fn resize(&self, size: Size) {
        if self.size.replace(size) != size {
            self.release();
        }
    }

    /// The last frame's pixels: premultiplied RGBA, in rows from the top.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) before
    /// the first frame of the size, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) where the backend reads
    /// nothing back.
    pub fn read_image(&self) -> Result<PremultipliedImage, Error> {
        let image = self
            .image
            .get()
            .ok_or_else(|| Error::invalid_state("the target has no frame since it was made or resized"))?;
        let texels = self
            .backend
            .gpu()
            .read_texels(image.color, TextureRegion::whole(image.size))
            .map_err(|error| Error::native(format!("reading the frame back failed: {error}")))?;
        Ok(PremultipliedImage::from_vec(image.size, texels))
    }

    fn make_image(&self, size: Size) -> Result<Image, GpuError> {
        let mut gpu = self.backend.gpu();
        let texture = |gpu: &mut dyn crate::gpu::Gpu, label, format, copy_src| {
            let usage = TextureUsage { render_target: true, copy_src, ..TextureUsage::default() };
            gpu.create_texture(&TextureDesc { label, size, format, usage })
        };
        let color = texture(&mut *gpu, "offscreen color", COLOR, true)?;
        match texture(&mut *gpu, "offscreen depth and stencil", DEPTH_STENCIL, false) {
            Ok(depth_stencil) => Ok(Image { size, color, depth_stencil }),
            Err(error) => {
                gpu.destroy_texture(color);
                Err(error)
            }
        }
    }

    /// Destroys the textures of the last frame, which the `Gpu` keeps until
    /// the submits that use them finish.
    fn release(&self) {
        if let Some(image) = self.image.take() {
            let mut gpu = self.backend.gpu();
            gpu.destroy_texture(image.color);
            gpu.destroy_texture(image.depth_stencil);
        }
    }
}

impl<B: RendererBackend> FrameTarget for OffscreenTarget<B> {
    fn begin_frame(&self) -> Option<FrameImage> {
        let size = self.size.get();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let image = match self.image.get() {
            Some(image) => image,
            None => match self.make_image(size) {
                Ok(image) => {
                    self.image.set(Some(image));
                    image
                }
                Err(error) => {
                    Log::error(Event::Render, format!("Making an offscreen frame's textures failed: {error}"));
                    return None;
                }
            },
        };
        Some(FrameImage {
            size,
            color: image.color,
            depth_stencil: Some(image.depth_stencil),
            targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
        })
    }

    /// The frame is read back when it is read.
    fn end_frame(&self) -> Handoff {
        if self.image.get().is_some() { Handoff::ReadBack } else { Handoff::Dropped }
    }
}

impl<B: RendererBackend> Drop for OffscreenTarget<B> {
    fn drop(&mut self) {
        self.release();
    }
}
