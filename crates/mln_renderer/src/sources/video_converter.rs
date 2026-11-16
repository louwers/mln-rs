//! Turns a video frame's YUV planes, in memory or on the GPU, into the RGBA
//! texture that raster layers sample.

use mln_gfx::gpu::GpuError;
use mln_gfx::video::{FrameKeepAlive, FramePixels, YuvColor};
use mln_util::size::Size;

use crate::gfx::context::Context;
use crate::gfx::texture2d::Texture2D;
use crate::sources::video_texture::{chroma_size, tight_rows};

pub(crate) struct VideoConverter {}

impl VideoConverter {
    pub(crate) fn new() -> VideoConverter {
        VideoConverter {}
    }

    /// Writes the frame of `size` with `pixels` to `target`, an RGBA
    /// texture that passes draw into, at the frame's size.
    pub(crate) fn convert(
        &mut self,
        context: &Context,
        size: Size,
        pixels: &FramePixels,
        _keep_alive: FrameKeepAlive,
        target: &mut Texture2D,
    ) -> Result<(), GpuError> {
        let (width, height) = (size.width as usize, size.height as usize);
        let chroma = chroma_size(size);
        let (chroma_width, chroma_height) = (chroma.width as usize, chroma.height as usize);
        let rgba = match pixels {
            FramePixels::Nv12 { y, uv, color } => {
                let y = tight_rows(y, width, height)?;
                let uv = tight_rows(uv, chroma_width * 2, chroma_height)?;
                yuv_to_rgba(size, *color, |x, row| {
                    let c = (row / 2) * chroma_width * 2 + (x / 2) * 2;
                    (y[row * width + x], uv[c], uv[c + 1])
                })
            }
            FramePixels::I420 { y, u, v, color } => {
                let y = tight_rows(y, width, height)?;
                let u = tight_rows(u, chroma_width, chroma_height)?;
                let v = tight_rows(v, chroma_width, chroma_height)?;
                yuv_to_rgba(size, *color, |x, row| {
                    let c = (row / 2) * chroma_width + x / 2;
                    (y[row * width + x], u[c], v[c])
                })
            }
            FramePixels::Gpu(frame) => return Err(GpuError::Unsupported(format!("drawing {} frames", frame.kind()))),
            FramePixels::Rgba(_) => return Err(GpuError::Invalid("an RGBA frame needs no conversion".to_owned())),
            _ => return Err(GpuError::Unsupported("this kind of video frame".to_owned())),
        };
        target.upload_data(context, &rgba, size)
    }
}

/// The opaque RGBA of a YUV frame of `size`, whose samples at each `x` and
/// row `sample` reads.
fn yuv_to_rgba(size: Size, color: YuvColor, sample: impl Fn(usize, usize) -> (u8, u8, u8)) -> Vec<u8> {
    let rows = color.to_rgb_rows();
    let mut rgba = Vec::with_capacity(size.width as usize * size.height as usize * 4);
    for row in 0..size.height as usize {
        for x in 0..size.width as usize {
            let (y, u, v) = sample(x, row);
            let yuv = [f32::from(y) / 255.0, f32::from(u) / 255.0, f32::from(v) / 255.0];
            for weights in rows {
                let value = weights[0] * yuv[0] + weights[1] * yuv[1] + weights[2] * yuv[2] + weights[3];
                rgba.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            rgba.push(255);
        }
    }
    rgba
}
