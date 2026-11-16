//! The texture that a video source's frames go into, which raster layers
//! sample as they sample an image source's image.

use std::cell::{Cell, OnceCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::GpuError;
use mln_gfx::video::{CpuPlane, FrameKeepAlive, FramePixels, VideoFrame};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_video::FrameSlot;

use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2DPtr;
use crate::sources::video_converter::VideoConverter;

/// A video source's frames as one RGBA texture: each new frame overwrites
/// it in place, at the frame's size. The render source and its render data
/// share it, and the render data's upload writes the frames.
pub(crate) struct VideoTexture {
    frames: Arc<FrameSlot>,
    /// The generation of the frame the texture holds, 0 for none.
    seen: Cell<u64>,
    /// Made with the first frame.
    texture: OnceCell<Texture2DPtr>,
    /// Turns YUV and GPU frames into the texture's RGBA, once one comes.
    converter: Cell<Option<Box<VideoConverter>>>,
}

impl VideoTexture {
    pub(crate) fn new(frames: Arc<FrameSlot>) -> VideoTexture {
        VideoTexture { frames, seen: Cell::new(0), texture: OnceCell::new(), converter: Cell::new(None) }
    }

    pub(crate) fn frames(&self) -> &Arc<FrameSlot> {
        &self.frames
    }

    /// Whether the texture holds a frame, or the player failed and none
    /// comes.
    pub(crate) fn drawn(&self) -> bool {
        self.seen.get() != 0 || self.frames.failure().is_some()
    }

    /// The texture, once it holds a frame.
    pub(crate) fn texture(&self) -> Option<Texture2DPtr> {
        self.texture.get().filter(|_| self.seen.get() != 0).map(Rc::clone)
    }

    /// Writes the slot's newest frame to the texture, if the texture doesn't
    /// hold it yet.
    pub(crate) fn upload(&self, context: &Context) {
        let Some(newest) = self.frames.newer_than(self.seen.get()) else {
            return;
        };
        match self.write(context, &newest.frame) {
            Ok(()) => self.seen.set(newest.generation),
            Err(GpuError::Unsupported(what)) if matches!(newest.frame.pixels, FramePixels::Gpu(_)) => {
                // The player sends frames in memory from now on; the texture
                // keeps the frame it has until one comes, and each render
                // till then tries the GPU frame again.
                if !self.frames.gpu_frames_refused() {
                    Log::info(Event::Render, format!("Video frames go through memory: {what} isn't supported"));
                }
                self.frames.refuse_gpu_frames();
            }
            Err(error) => {
                Log::error(Event::Render, format!("Drawing a video frame failed: {error}"));
                self.seen.set(newest.generation);
            }
        }
    }

    fn write(&self, context: &Context, frame: &Arc<VideoFrame>) -> Result<(), GpuError> {
        let size = frame.size;
        if size.is_empty() {
            return Err(GpuError::Invalid("a video frame without pixels".to_owned()));
        }
        let mut texture = self.texture.get_or_init(|| context.create_texture_2d()).borrow_mut();
        texture.set_render_target();
        match &frame.pixels {
            FramePixels::Rgba(plane) => {
                let data = tight_rows(plane, size.width as usize * 4, size.height as usize)?;
                texture.upload_data(context, &data, size)
            }
            pixels => {
                let keep_alive: FrameKeepAlive = Arc::<VideoFrame>::clone(frame);
                let mut converter = self.converter.take().unwrap_or_else(|| Box::new(VideoConverter::new()));
                let converted = converter.convert(context, size, pixels, keep_alive, &mut texture);
                self.converter.set(Some(converter));
                converted
            }
        }
    }
}

/// The rows of `plane` with no padding between them, `row_bytes` each.
pub(crate) fn tight_rows(
    plane: &CpuPlane,
    row_bytes: usize,
    rows: usize,
) -> Result<std::borrow::Cow<'_, [u8]>, GpuError> {
    let short =
        || GpuError::Invalid(format!("a video plane of {} bytes for {rows} rows of {row_bytes}", plane.data.len()));
    if plane.stride < row_bytes {
        return Err(short());
    }
    let needed = plane.stride * rows.saturating_sub(1) + row_bytes;
    if rows == 0 || plane.data.len() < needed {
        return Err(short());
    }
    if plane.stride == row_bytes {
        return Ok(std::borrow::Cow::Borrowed(&plane.data[..row_bytes * rows]));
    }
    let mut data = Vec::with_capacity(row_bytes * rows);
    for row in plane.data.chunks(plane.stride).take(rows) {
        data.extend_from_slice(&row[..row_bytes]);
    }
    Ok(std::borrow::Cow::Owned(data))
}

/// The size of a 4:2:0 frame's chroma planes.
pub(crate) fn chroma_size(size: Size) -> Size {
    Size::new(size.width.div_ceil(2), size.height.div_ceil(2))
}
