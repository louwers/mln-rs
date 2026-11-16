//! Video frames: what a platform's video player decodes, and a renderer
//! draws through [`Gpu::import_video_frame`](crate::gpu::Gpu::import_video_frame)
//! or by writing the frame's planes to textures.

use std::any::Any;
use std::sync::Arc;

use mln_util::size::Size;

use crate::gpu::TextureId;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_env = "ohos")]
mod ohos;

/// The matrix that turns a YUV frame's samples into RGB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum YuvMatrix {
    /// ITU-R BT.601, of standard-definition video.
    Bt601,
    /// ITU-R BT.709, of high-definition video.
    #[default]
    Bt709,
    /// ITU-R BT.2020, of ultra-high-definition video.
    Bt2020,
}

/// The range of a YUV frame's 8-bit samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum YuvRange {
    /// Luma from 16 to 235, chroma from 16 to 240: most video.
    #[default]
    Limited,
    /// Every sample from 0 to 255, as JPEG has them.
    Full,
}

/// How a YUV frame's samples are RGB colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct YuvColor {
    pub matrix: YuvMatrix,
    pub range: YuvRange,
}

impl YuvColor {
    /// The rows of the matrix that turns `(y, u, v, 1)`, each sample from 0
    /// to 1, into straight RGB: each row is the weights of the three samples
    /// and the offset.
    #[must_use]
    pub fn to_rgb_rows(self) -> [[f32; 4]; 3] {
        let (kr, kb) = match self.matrix {
            YuvMatrix::Bt601 => (0.299, 0.114),
            YuvMatrix::Bt709 => (0.2126, 0.0722),
            YuvMatrix::Bt2020 => (0.2627, 0.0593),
        };
        let kg = 1.0 - kr - kb;
        // The samples' scales and offsets, from the range into 0..1 for luma
        // and -0.5..0.5 for chroma.
        let (y_scale, y_offset, c_scale) = match self.range {
            YuvRange::Limited => (255.0 / 219.0, -16.0 / 255.0, 255.0 / 224.0),
            YuvRange::Full => (1.0, 0.0, 1.0),
        };
        let c_offset = -128.0 / 255.0;
        // R = Y' + 2(1 - kr) Cr; B = Y' + 2(1 - kb) Cb; G from the two.
        let r_v = 2.0 * (1.0 - kr);
        let b_u = 2.0 * (1.0 - kb);
        let g_u = -b_u * kb / kg;
        let g_v = -r_v * kr / kg;
        let row = |u: f64, v: f64| {
            let (y, u, v) = (y_scale, u * c_scale, v * c_scale);
            let offset = y * y_offset + (u + v) * c_offset;
            [y as f32, u as f32, v as f32, offset as f32]
        };
        [row(0.0, r_v), row(g_u, g_v), row(b_u, 0.0)]
    }
}

/// One plane of a frame in memory: rows from the top, `stride` bytes apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuPlane {
    pub data: Vec<u8>,
    /// The bytes from the start of one row to the start of the next, at
    /// least the plane's width in bytes.
    pub stride: usize,
}

/// A frame's pixels.
#[derive(Debug)]
#[non_exhaustive]
pub enum FramePixels {
    /// Premultiplied RGBA, four bytes a pixel. Decoders' frames are opaque,
    /// so their straight RGBA is premultiplied too.
    Rgba(CpuPlane),
    /// 8-bit 4:2:0 YUV: a full-size luma plane, and a plane of interleaved
    /// Cb and Cr samples at half the width and height (rounded up).
    Nv12 { y: CpuPlane, uv: CpuPlane, color: YuvColor },
    /// 8-bit 4:2:0 YUV in three planes: luma, then Cb and Cr at half the
    /// width and height (rounded up).
    I420 { y: CpuPlane, u: CpuPlane, v: CpuPlane, color: YuvColor },
    /// A frame in GPU memory, which only a backend that imports its kind
    /// reads ([`Gpu::import_video_frame`](crate::gpu::Gpu::import_video_frame)).
    Gpu(GpuFrame),
}

/// A decoded frame of a video.
#[derive(Debug)]
pub struct VideoFrame {
    /// The frame's size in pixels.
    pub size: Size,
    pub pixels: FramePixels,
    /// The frame's time in the video, in seconds from its start, if the
    /// player knows it.
    pub time: Option<f64>,
}

/// How a frame's planes in GPU memory hold its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaneLayout {
    /// One plane of RGBA (or BGRA, which the texture's format swizzles).
    Rgba,
    /// A luma plane and a plane of interleaved Cb and Cr at half size.
    Nv12,
    /// Luma, Cb and Cr planes, the chroma at half size.
    I420,
}

/// A frame in GPU memory, by the platform's handle on it.
#[derive(Debug)]
#[non_exhaustive]
pub enum GpuFrame {
    /// A Linux dmabuf of `fourcc` (a `DRM_FORMAT_*`) with `modifier`
    /// (`DRM_FORMAT_MOD_INVALID` for a layout only the driver knows), of
    /// `size` pixels, its planes in order. The descriptors stay open, and
    /// the producer doesn't write the memory again, while the frame lives.
    #[cfg(unix)]
    DmaBuf {
        fourcc: u32,
        modifier: u64,
        size: Size,
        planes: Vec<DmaBufPlane>,
        color: YuvColor,
        lease: Option<FrameLease>,
    },
    /// An `AHardwareBuffer*`, which the frame holds a reference on and
    /// releases when it drops. The producer doesn't write the buffer again
    /// while the frame lives.
    AndroidHardwareBuffer { buffer: *mut std::ffi::c_void, color: YuvColor, lease: Option<FrameLease> },
    /// An `OpenHarmony` `OH_NativeBuffer*`, which the frame holds a reference
    /// on and unreferences when it drops. The producer doesn't write the
    /// buffer again while the frame lives.
    #[cfg(target_env = "ohos")]
    OhosNativeBuffer { buffer: *mut std::ffi::c_void, color: YuvColor, lease: Option<FrameLease> },
    /// A `CVPixelBufferRef` backed by an IOSurface, which the frame
    /// retains: BGRA, or biplanar 4:2:0 YUV.
    CvPixelBuffer { buffer: *mut std::ffi::c_void, color: YuvColor },
    /// A D3D11 or D3D12 texture's shared NT handle, which the frame closes
    /// when it drops: BGRA ([`PlaneLayout::Rgba`]) or NV12. The producer's
    /// writes to the texture have finished when it hands the frame over, and
    /// it doesn't write the texture again: the importer waits for no keyed
    /// mutex or fence.
    #[cfg(windows)]
    DxgiSharedHandle { handle: std::os::windows::io::OwnedHandle, layout: PlaneLayout, color: YuvColor },
    /// A JS `ImageBitmap` or `VideoFrame`, in the browser.
    #[cfg(target_family = "wasm")]
    Web(WebImage),
}

#[cfg(target_os = "android")]
impl Drop for GpuFrame {
    fn drop(&mut self) {
        if let GpuFrame::AndroidHardwareBuffer { buffer, .. } = *self {
            android::release_hardware_buffer(buffer);
        }
    }
}

#[cfg(target_env = "ohos")]
impl Drop for GpuFrame {
    fn drop(&mut self) {
        if let GpuFrame::OhosNativeBuffer { buffer, .. } = *self {
            ohos::unreference_native_buffer(buffer);
        }
    }
}

/// One plane of a [`GpuFrame::DmaBuf`].
#[cfg(unix)]
#[derive(Debug)]
pub struct DmaBufPlane {
    pub fd: std::os::fd::OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

// SAFETY: the raw handles name reference-counted platform objects, which
// the frame owns a reference on and which any thread may use and release.
#[expect(unsafe_code, reason = "the frames' platform handles are raw pointers")]
unsafe impl Send for GpuFrame {}
// SAFETY: as for `Send`: a shared frame only reads the handles.
#[expect(unsafe_code, reason = "the frames' platform handles are raw pointers")]
unsafe impl Sync for GpuFrame {}

#[cfg(target_vendor = "apple")]
impl Drop for GpuFrame {
    fn drop(&mut self) {
        if let GpuFrame::CvPixelBuffer { buffer, .. } = *self
            && !buffer.is_null()
        {
            // SAFETY: the frame owns one retain of the buffer, which this
            // gives back once.
            #[expect(unsafe_code, reason = "releasing a CoreFoundation object")]
            unsafe {
                apple::CFRelease(buffer);
            }
        }
    }
}

/// The call that gives back a frame's `CVPixelBuffer`.
#[cfg(target_vendor = "apple")]
#[expect(unsafe_code, reason = "CoreFoundation's C API")]
mod apple {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub(super) fn CFRelease(cf: *mut std::ffi::c_void);
    }
}

/// A browser image that a frame holds: an `ImageBitmap` or a `VideoFrame`,
/// which the frame closes when it drops.
#[cfg(target_family = "wasm")]
#[derive(Debug)]
pub struct WebImage(pub wasm_bindgen::JsValue);

#[cfg(target_family = "wasm")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    /// An `ImageBitmap` or a `VideoFrame`, as what closes.
    type Closable;
    #[wasm_bindgen(method, catch)]
    fn close(this: &Closable) -> Result<(), wasm_bindgen::JsValue>;
}

#[cfg(target_family = "wasm")]
impl Drop for WebImage {
    /// Lets go of the image's pixels now, rather than when JS collects it:
    /// a video's frames would otherwise pile up in GPU memory.
    fn drop(&mut self) {
        use wasm_bindgen::JsCast;
        // Something without a `close` has nothing to let go of.
        let _ = self.0.unchecked_ref::<Closable>().close();
    }
}

// SAFETY: the browser build runs the map and its renderer on one thread, so
// the image never leaves it.
#[cfg(target_family = "wasm")]
#[expect(unsafe_code, reason = "JS values are single-threaded")]
unsafe impl Send for WebImage {}
// SAFETY: as for `Send`.
#[cfg(target_family = "wasm")]
#[expect(unsafe_code, reason = "JS values are single-threaded")]
unsafe impl Sync for WebImage {}

/// A frame's planes as textures of a backend, from
/// [`Gpu::import_video_frame`](crate::gpu::Gpu::import_video_frame).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedFrame {
    pub layout: PlaneLayout,
    /// One sampled texture per plane of `layout`, which the caller
    /// destroys.
    pub planes: Vec<TextureId>,
    pub color: YuvColor,
    /// Whether the plane textures are GL `TEXTURE_EXTERNAL_OES` textures,
    /// which only `samplerExternalOES` samples.
    pub external: bool,
}

/// What keeps a producer from reusing a frame's memory for another frame,
/// such as the decoder's buffer that the frame is in: dropped with the
/// frame. A reference on the memory alone keeps it allocated, not
/// unchanged.
pub type FrameLease = Box<dyn Any + Send + Sync>;

/// What a backend holds while it reads an imported frame: dropped once the
/// GPU no longer reads it, which gives the frame back to its decoder.
pub type FrameKeepAlive = Arc<dyn Any + Send + Sync>;

impl GpuFrame {
    /// The kind of frame, for messages.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            #[cfg(unix)]
            GpuFrame::DmaBuf { .. } => "dmabuf",
            GpuFrame::AndroidHardwareBuffer { .. } => "Android hardware buffer",
            #[cfg(target_env = "ohos")]
            GpuFrame::OhosNativeBuffer { .. } => "OpenHarmony native buffer",
            GpuFrame::CvPixelBuffer { .. } => "CVPixelBuffer",
            #[cfg(windows)]
            GpuFrame::DxgiSharedHandle { .. } => "DXGI shared handle",
            #[cfg(target_family = "wasm")]
            GpuFrame::Web(_) => "browser image",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(color: YuvColor, [y, u, v]: [u8; 3]) -> [f32; 3] {
        let sample = [f32::from(y) / 255.0, f32::from(u) / 255.0, f32::from(v) / 255.0, 1.0];
        color.to_rgb_rows().map(|row| row.iter().zip(sample).map(|(w, s)| w * s).sum())
    }

    fn assert_near(actual: [f32; 3], expected: [f32; 3]) {
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 0.01, "{actual:?} isn't near {expected:?}");
        }
    }

    #[test]
    fn limited_range_black_and_white_are_the_range_ends() {
        let color = YuvColor::default();
        assert_near(rgb(color, [16, 128, 128]), [0.0, 0.0, 0.0]);
        assert_near(rgb(color, [235, 128, 128]), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn full_range_bt601_red_comes_back_red() {
        // JPEG's full-range BT.601 encoding of pure red.
        let color = YuvColor { matrix: YuvMatrix::Bt601, range: YuvRange::Full };
        assert_near(rgb(color, [76, 85, 255]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn limited_range_bt709_green_comes_back_green() {
        // BT.709's limited-range encoding of pure green.
        assert_near(rgb(YuvColor::default(), [173, 42, 26]), [0.0, 1.0, 0.0]);
    }
}
