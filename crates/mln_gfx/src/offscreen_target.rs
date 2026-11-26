//! Frames drawn into textures of the library's and read back, which headless
//! rendering and render tests draw into.

use std::marker::PhantomData;

use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::frame::{FrameImage, FrameTarget, Handoff};
use crate::gpu::{
    Gpu, GpuError, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage, destroy_unused,
    read_texels,
};
use crate::log_target;

/// The format of an offscreen frame's color, which
/// [`read_image`](OffscreenTarget::read_image) reads as it is.
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;

/// Frames drawn into a color texture and a depth and stencil one of a
/// backend's `Gpu`, which [`read_image`](Self::read_image) reads back.
pub struct OffscreenTarget<G: ?Sized> {
    size: Size,
    /// The textures, made with the first frame of a size.
    image: Option<Image>,
    gpu: PhantomData<fn(&mut G)>,
}

/// A frame's color and depth-stencil textures, of `size`.
#[derive(Clone, Copy)]
struct Image {
    size: Size,
    color: TextureId,
    depth_stencil: TextureId,
}

impl<G: Gpu + ?Sized> OffscreenTarget<G> {
    /// A target of `size` physical pixels, whose frames are textures of the
    /// `Gpu` its calls take.
    #[must_use]
    pub fn new(size: Size) -> Self {
        OffscreenTarget { size, image: None, gpu: PhantomData }
    }

    /// The last frame's pixels: premultiplied RGBA, in rows from the top.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) before
    /// the first frame of the size, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) where the backend reads
    /// nothing back.
    pub fn read_image(&self, gpu: &mut G) -> Result<PremultipliedImage, Error> {
        let image =
            self.image.ok_or_else(|| Error::invalid_state("the target has no frame since it was made or resized"))?;
        let texels = read_texels(gpu, image.color, TextureRegion::whole(image.size))
            .map_err(|error| Error::native(format!("reading the frame back failed: {error}")))?;
        Ok(PremultipliedImage::from_vec(image.size, texels))
    }

    fn make_image(gpu: &mut G, size: Size) -> Result<Image, GpuError> {
        let mut texture = |label, format, copy_src| {
            let usage = TextureUsage { render_target: true, copy_src, ..TextureUsage::default() };
            gpu.create_texture(&TextureDesc { label, size, format, usage, mipmapped: false })
        };
        let color = texture("offscreen color", COLOR, true)?;
        match texture("offscreen depth and stencil", DEPTH_STENCIL, false) {
            Ok(depth_stencil) => Ok(Image { size, color, depth_stencil }),
            Err(error) => {
                destroy_unused(gpu, [color.into()]);
                Err(error)
            }
        }
    }
}

impl<G: Gpu + ?Sized> FrameTarget for OffscreenTarget<G> {
    type Gpu = G;

    /// Destroys the textures of the last frame where `size` is new.
    fn resize(&mut self, gpu: &mut G, size: Size) {
        if std::mem::replace(&mut self.size, size) != size {
            self.tear_down(gpu);
        }
    }

    fn begin_frame(&mut self, gpu: &mut G) -> Option<FrameImage> {
        let size = self.size;
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let image = match self.image {
            Some(image) => image,
            None => match Self::make_image(gpu, size) {
                Ok(image) => *self.image.insert(image),
                Err(error) => {
                    log::error!(target: log_target::RENDER, "Making an offscreen frame's textures failed: {error}");
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
    fn end_frame(&mut self, _gpu: &mut G) -> Handoff {
        if self.image.is_some() { Handoff::ReadBack } else { Handoff::Dropped }
    }

    /// Destroys the textures of the last frame, which the `Gpu` keeps until
    /// the work that uses them finishes.
    fn tear_down(&mut self, gpu: &mut G) {
        if let Some(image) = self.image.take() {
            destroy_unused(gpu, [image.color.into(), image.depth_stencil.into()]);
        }
    }
}
