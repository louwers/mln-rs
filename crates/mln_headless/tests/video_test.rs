// Video sources through Map -> HeadlessFrontend: a player that hands the map
// one frame, of each layout a player sends from memory, whose four quarters
// are four colors, drawn over the whole view. Each quarter must keep its
// color and its place, on whichever backend the frontend renders. On Android,
// also frames in hardware buffers, and the platform's player (video_android.rs).

use std::sync::Arc;

use mln_actor::RunLoop;
use mln_gfx::video::{CpuPlane, FramePixels, VideoFrame, YuvColor, YuvMatrix, YuvRange};
use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapMode, MapOptions};
use mln_storage::FileSource;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_video::{OpenRequest, VideoPlayer, VideoPlayers};
use parking_lot::Mutex;

#[cfg(target_os = "android")]
mod video_android;

/// The quarters' colors: top left, top right, bottom left, bottom right. The
/// last saturates no channel, so a wrong YUV range or matrix shows in it.
const QUARTERS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [64, 128, 192]];

/// Hands each video the frame it holds, once.
#[derive(Debug)]
struct OneFrame(Mutex<Option<VideoFrame>>);

struct Still;

impl VideoPlayer for Still {
    fn play(&mut self) {}
    fn pause(&mut self) {}
    fn seek(&mut self, _seconds: f64) {}
}

impl VideoPlayers for OneFrame {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let frame = self.0.lock().take().ok_or("one video only")?;
        request.frames.push(frame);
        Ok(Box::new(Still))
    }
}

/// The color of the quarter that `(x, y)` of a `size` frame is in.
fn quarter(size: Size, x: u32, y: u32) -> [u8; 3] {
    let index = usize::from(x >= size.width / 2) + 2 * usize::from(y >= size.height / 2);
    QUARTERS[index]
}

/// Full-range BT.601 YUV of an RGB color, as JPEG encodes it.
fn yuv([r, g, b]: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let u = 128.0 + (b - y) / (2.0 * (1.0 - 0.114));
    let v = 128.0 + (r - y) / (2.0 * (1.0 - 0.299));
    [y, u, v].map(|value| value.round().clamp(0.0, 255.0) as u8)
}

const FULL_BT601: YuvColor = YuvColor { matrix: YuvMatrix::Bt601, range: YuvRange::Full };

fn rgba_frame(size: Size) -> VideoFrame {
    let mut data = Vec::new();
    for y in 0..size.height {
        for x in 0..size.width {
            data.extend(quarter(size, x, y));
            data.push(255);
        }
    }
    VideoFrame { size, pixels: FramePixels::Rgba(CpuPlane { data, stride: size.width as usize * 4 }), time: None }
}

/// The frame's planes: luma, then each chroma sample of the 2x2 block it
/// covers, with each row `padding` bytes longer than its samples.
fn yuv_planes(size: Size, padding: usize) -> (CpuPlane, CpuPlane, CpuPlane) {
    let chroma = Size::new(size.width.div_ceil(2), size.height.div_ceil(2));
    let plane = |width: u32, height: u32, sample: &dyn Fn(u32, u32) -> u8| {
        let stride = width as usize + padding;
        let mut data = vec![0; stride * height as usize];
        for y in 0..height {
            for x in 0..width {
                data[y as usize * stride + x as usize] = sample(x, y);
            }
        }
        CpuPlane { data, stride }
    };
    (
        plane(size.width, size.height, &|x, y| yuv(quarter(size, x, y))[0]),
        plane(chroma.width, chroma.height, &|x, y| yuv(quarter(size, x * 2, y * 2))[1]),
        plane(chroma.width, chroma.height, &|x, y| yuv(quarter(size, x * 2, y * 2))[2]),
    )
}

fn i420_frame(size: Size) -> VideoFrame {
    let (y, u, v) = yuv_planes(size, 3);
    VideoFrame { size, pixels: FramePixels::I420 { y, u, v, color: FULL_BT601 }, time: None }
}

fn nv12_frame(size: Size) -> VideoFrame {
    let (y, u, v) = yuv_planes(size, 0);
    let stride = u.stride * 2 + 2;
    let rows = u.data.len() / u.stride;
    let mut uv = vec![0; stride * rows];
    for row in 0..rows {
        for x in 0..u.stride {
            uv[row * stride + x * 2] = u.data[row * u.stride + x];
            uv[row * stride + x * 2 + 1] = v.data[row * v.stride + x];
        }
    }
    VideoFrame {
        size,
        pixels: FramePixels::Nv12 { y, uv: CpuPlane { data: uv, stride }, color: FULL_BT601 },
        time: None,
    }
}

/// A map whose video covers the whole view.
struct VideoMap {
    map: Map,
    frontend: HeadlessFrontend,
    _run_loop: RunLoop,
}

/// At zoom 0 the world is 512 pixels wide: the video covers the view.
const VIEW: Size = Size::new(512, 512);

impl VideoMap {
    /// A map whose video `players` play, reading it through `file_source`,
    /// on the harness's backend.
    fn new(players: Arc<dyn VideoPlayers>, file_source: Option<Arc<dyn FileSource>>) -> VideoMap {
        let run_loop = RunLoop::new();
        let frontend = HeadlessFrontend::new(VIEW, 1.0, &run_loop).expect("a headless frontend");
        VideoMap::on(frontend, run_loop, players, file_source)
    }

    /// [`new`](Self::new) through `frontend` on `run_loop`.
    fn on(
        mut frontend: HeadlessFrontend,
        run_loop: RunLoop,
        players: Arc<dyn VideoPlayers>,
        file_source: Option<Arc<dyn FileSource>>,
    ) -> VideoMap {
        let size = VIEW;
        let options = MapOptions { map_mode: MapMode::Static, size, ..Default::default() };
        let mut map = match file_source {
            Some(file_source) => Map::with_file_source(file_source, options, &run_loop).unwrap(),
            None => Map::new(options, &run_loop).unwrap(),
        };
        map.set_video_players(Some(players));
        frontend.attach(&map);
        map.style_mut().load_json(
            r#"{
              "version": 8,
              "center": [0, 0],
              "zoom": 0,
              "sources": {
                "video": {
                  "type": "video",
                  "urls": ["fixture://video.mp4"],
                  "coordinates": [[-180, 85], [180, 85], [180, -85], [-180, -85]]
                }
              },
              "layers": [{"id": "video", "type": "raster", "source": "video", "paint": {"raster-fade-duration": 0}}]
            }"#,
        );
        let camera = map.style().get_default_camera();
        map.jump_to(&camera);
        VideoMap { map, frontend, _run_loop: run_loop }
    }

    fn render(&mut self) -> PremultipliedImage {
        self.frontend.render(&mut self.map).expect("render").image
    }
}

/// Renders a map whose video covers the whole view, with `frame`.
fn render(frame: VideoFrame) -> PremultipliedImage {
    VideoMap::new(Arc::new(OneFrame(Mutex::new(Some(frame)))), None).render()
}

/// Renders a map whose video covers the whole view, with `frame`, through
/// `frontend` on `run_loop`.
#[cfg(windows)]
fn render_with(frontend: HeadlessFrontend, run_loop: RunLoop, frame: VideoFrame) -> PremultipliedImage {
    VideoMap::on(frontend, run_loop, Arc::new(OneFrame(Mutex::new(Some(frame)))), None).render()
}

/// The colors at the center of each quarter of the view, in the order of
/// [`QUARTERS`].
fn quarter_colors(image: &PremultipliedImage) -> [[u8; 4]; 4] {
    let size = image.size;
    [(1, 1), (3, 1), (1, 3), (3, 3)].map(|(x, y)| {
        let (px, py) = (size.width * x / 4, size.height * y / 4);
        let offset = (py * size.width + px) as usize * 4;
        [0, 1, 2, 3].map(|channel| image.data[offset + channel])
    })
}

/// Whether each quarter of the view is opaque and within `tolerance` of
/// `expected`'s color in each channel.
fn shows_quarters(image: &PremultipliedImage, expected: [[u8; 3]; 4], tolerance: u8) -> bool {
    quarter_colors(image).iter().zip(expected).all(|(actual, expected)| {
        actual[3] == 255 && actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= tolerance)
    })
}

/// Checks the center of each quarter of the view.
fn assert_quarters(image: &PremultipliedImage, what: &str) {
    assert!(shows_quarters(image, QUARTERS, 3), "{what}: {:?}, expected {QUARTERS:?}", quarter_colors(image));
}

#[test]
fn an_rgba_frame_draws_in_place() {
    assert_quarters(&render(rgba_frame(Size::new(16, 16))), "RGBA");
}

#[test]
fn an_i420_frame_draws_in_place() {
    assert_quarters(&render(i420_frame(Size::new(16, 16))), "I420");
}

#[test]
fn an_nv12_frame_draws_in_place() {
    assert_quarters(&render(nv12_frame(Size::new(16, 16))), "NV12");
}

#[test]
fn a_frame_of_odd_size_draws_in_place() {
    assert_quarters(&render(nv12_frame(Size::new(17, 15))), "odd NV12");
}

/// Frames in D3D11 textures that the frame shares through an NT handle, as
/// a Media Foundation player sends them, drawn on D3D12.
#[cfg(windows)]
#[expect(unsafe_code, reason = "the frames' textures are made through the windows crate's D3D11")]
mod d3d11 {
    use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
    use std::sync::Arc;

    use mln_actor::RunLoop;
    use mln_d3d12::windows::Win32::Foundation::HMODULE;
    use mln_d3d12::windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use mln_d3d12::windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_RESOURCE_MISC_SHARED,
        D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
        D3D11_USAGE_DEFAULT, D3D11CreateDevice, ID3D11Texture2D,
    };
    use mln_d3d12::windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC,
    };
    use mln_d3d12::windows::Win32::Graphics::Dxgi::{
        DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIResource1,
    };
    use mln_d3d12::windows::core::Interface as _;
    use mln_gfx::video::{FramePixels, GpuFrame, PlaneLayout, VideoFrame, YuvColor};
    use mln_headless::HeadlessFrontend;
    use mln_runtime::{BackendType, Device, DeviceOptions};
    use mln_util::image::PremultipliedImage;
    use mln_util::size::Size;

    use super::{FULL_BT601, VIEW, assert_quarters, quarter, render_with, yuv};

    /// A shared D3D11 texture of `format` and `size` that holds `data`,
    /// rows `pitch` bytes apart (NV12's chroma rows after its luma rows),
    /// and the NT handle that shares it.
    fn shared_texture(format: DXGI_FORMAT, size: Size, data: &[u8], pitch: usize) -> OwnedHandle {
        let (mut device, mut context) = (None, None);
        // SAFETY: the call fills the device and its context.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                Some(&raw mut context),
            )
        }
        .expect("a D3D11 device");
        let (device, context) = (device.expect("a device"), context.expect("a context"));
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.width,
            Height: size.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
        };
        let initial =
            D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr().cast(), SysMemPitch: pitch as u32, SysMemSlicePitch: 0 };
        let mut texture: Option<ID3D11Texture2D> = None;
        // SAFETY: the descriptions and data outlive the calls; the handle is
        // then this function's, and the frame's once it owns it.
        unsafe {
            device
                .CreateTexture2D(&raw const desc, Some(&raw const initial), Some(&raw mut texture))
                .expect("a texture");
            let resource: IDXGIResource1 = texture.expect("a texture").cast().expect("a DXGI resource");
            let handle = resource
                .CreateSharedHandle(None, (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0, None)
                .expect("a shared handle");
            // The texture's contents are written as it's made: the frame's
            // reader has nothing to wait for once the device has its work.
            context.Flush();
            OwnedHandle::from_raw_handle(handle.0)
        }
    }

    fn render_on_d3d12(frame: VideoFrame) -> PremultipliedImage {
        let run_loop = RunLoop::new();
        let device = Device::with_backend(BackendType::D3D12, DeviceOptions::default()).expect("a D3D12 device");
        let frontend = HeadlessFrontend::with_device(device, VIEW, 1.0, Arc::default(), &run_loop).expect("a frontend");
        render_with(frontend, run_loop, frame)
    }

    #[test]
    fn a_shared_nv12_texture_draws_in_place_on_d3d12() {
        let size = Size::new(16, 16);
        let pitch = size.width as usize;
        let luma = pitch * size.height as usize;
        let mut data = vec![0; luma * 3 / 2];
        for y in 0..size.height {
            for x in 0..size.width {
                data[y as usize * pitch + x as usize] = yuv(quarter(size, x, y))[0];
            }
        }
        for y in 0..size.height / 2 {
            for x in 0..size.width / 2 {
                let [_, u, v] = yuv(quarter(size, x * 2, y * 2));
                let at = luma + y as usize * pitch + x as usize * 2;
                data[at] = u;
                data[at + 1] = v;
            }
        }
        let handle = shared_texture(DXGI_FORMAT_NV12, size, &data, pitch);
        let pixels =
            FramePixels::Gpu(GpuFrame::DxgiSharedHandle { handle, layout: PlaneLayout::Nv12, color: FULL_BT601 });
        assert_quarters(&render_on_d3d12(VideoFrame { size, pixels, time: None }), "shared NV12");
    }

    #[test]
    fn a_shared_bgra_texture_draws_in_place_on_d3d12() {
        let size = Size::new(16, 16);
        let mut data = Vec::new();
        for y in 0..size.height {
            for x in 0..size.width {
                let [r, g, b] = quarter(size, x, y);
                data.extend([b, g, r, 255]);
            }
        }
        let handle = shared_texture(DXGI_FORMAT_B8G8R8A8_UNORM, size, &data, size.width as usize * 4);
        let pixels = FramePixels::Gpu(GpuFrame::DxgiSharedHandle {
            handle,
            layout: PlaneLayout::Rgba,
            color: YuvColor::default(),
        });
        assert_quarters(&render_on_d3d12(VideoFrame { size, pixels, time: None }), "shared BGRA");
    }
}

/// Frames in IOSurface-backed `CVPixelBuffer`s, which the Metal backend
/// imports and the others refuse.
#[cfg(target_vendor = "apple")]
mod pixel_buffers {
    #![expect(unsafe_code, reason = "CoreVideo's C API")]

    use std::ptr::NonNull;

    use mln_gfx::video::GpuFrame;
    use mln_video::FrameSink;
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFString, CFType};
    use objc2_core_video::{
        CVAttachmentMode, CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrixKey,
        kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey, kCVPixelFormatType_32BGRA,
        kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVReturnSuccess,
    };

    use super::*;

    /// A retained IOSurface-backed, Metal-compatible pixel buffer of
    /// `format` and `size`, whose planes `fill` writes: each plane's index,
    /// bytes and stride.
    fn pixel_buffer(size: Size, format: u32, fill: &dyn Fn(usize, &mut [u8], usize)) -> NonNull<CVPixelBuffer> {
        let surface = CFDictionary::<CFString, CFType>::empty();
        // SAFETY: CoreVideo's constant keys.
        let keys = unsafe { [kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey] };
        let values: [&CFType; 2] = [surface.as_ref(), CFBoolean::new(true).as_ref()];
        let attributes = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        let mut buffer: *mut CVPixelBuffer = std::ptr::null_mut();
        // SAFETY: the attributes map strings to CF objects, as CoreVideo
        // reads them, and the out pointer is valid.
        let status = unsafe {
            CVPixelBufferCreate(
                None,
                size.width as usize,
                size.height as usize,
                format,
                Some(attributes.as_opaque()),
                NonNull::from(&mut buffer),
            )
        };
        assert_eq!(kCVReturnSuccess, status, "CVPixelBufferCreate");
        let buffer = NonNull::new(buffer).expect("a pixel buffer");
        // SAFETY: CoreVideo made the buffer, which this holds a retain of.
        let pixels = unsafe { buffer.as_ref() };
        // SAFETY: the buffer is valid, and unlocks below.
        assert_eq!(kCVReturnSuccess, unsafe { CVPixelBufferLockBaseAddress(pixels, CVPixelBufferLockFlags(0)) });
        let planes = if format == kCVPixelFormatType_32BGRA { 1 } else { 2 };
        for plane in 0..planes {
            let stride = CVPixelBufferGetBytesPerRowOfPlane(pixels, plane);
            let rows = if plane == 0 { size.height } else { size.height.div_ceil(2) } as usize;
            let base = CVPixelBufferGetBaseAddressOfPlane(pixels, plane).cast::<u8>();
            assert!(!base.is_null(), "plane {plane} has no address");
            // SAFETY: the locked plane has `rows` rows of `stride` bytes.
            fill(plane, unsafe { std::slice::from_raw_parts_mut(base, stride * rows) }, stride);
        }
        // SAFETY: locked above.
        assert_eq!(kCVReturnSuccess, unsafe { CVPixelBufferUnlockBaseAddress(pixels, CVPixelBufferLockFlags(0)) });
        buffer
    }

    /// Full-range NV12 quarters, whose attachment names BT.601.
    fn nv12_buffer(size: Size) -> NonNull<CVPixelBuffer> {
        let buffer = pixel_buffer(size, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, &|plane, data, stride| {
            for (y, row) in data.chunks_mut(stride).enumerate() {
                let y = y as u32;
                if plane == 0 {
                    for x in 0..size.width {
                        row[x as usize] = yuv(quarter(size, x, y))[0];
                    }
                } else {
                    for x in 0..size.width.div_ceil(2) {
                        let [_, u, v] = yuv(quarter(size, x * 2, y * 2));
                        row[x as usize * 2] = u;
                        row[x as usize * 2 + 1] = v;
                    }
                }
            }
        });
        // SAFETY: CoreVideo's constant strings, on a valid buffer.
        unsafe {
            buffer.as_ref().set_attachment(
                kCVImageBufferYCbCrMatrixKey,
                kCVImageBufferYCbCrMatrix_ITU_R_601_4.as_ref(),
                CVAttachmentMode::ShouldPropagate,
            );
        }
        buffer
    }

    fn bgra_buffer(size: Size) -> NonNull<CVPixelBuffer> {
        pixel_buffer(size, kCVPixelFormatType_32BGRA, &|_, data, stride| {
            for (y, row) in data.chunks_mut(stride).enumerate() {
                for x in 0..size.width {
                    let [r, g, b] = quarter(size, x, y as u32);
                    row[x as usize * 4..x as usize * 4 + 4].copy_from_slice(&[b, g, r, 255]);
                }
            }
        })
    }

    /// Hands each video one frame of the pixel buffer it holds, and keeps
    /// the video's sink.
    #[derive(Debug, Default)]
    struct OneGpuFrame {
        frame: Mutex<Option<VideoFrame>>,
        sink: Mutex<Option<FrameSink>>,
    }

    impl VideoPlayers for OneGpuFrame {
        fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
            let frame = self.frame.lock().take().ok_or("one video only")?;
            request.frames.push(frame);
            *self.sink.lock() = Some(request.frames);
            Ok(Box::new(Still))
        }
    }

    /// Draws `buffer`'s frame on Metal, whose quarters must keep their
    /// place; other backends must refuse it.
    fn draws_or_refuses(size: Size, buffer: NonNull<CVPixelBuffer>, what: &str) {
        let pixels = FramePixels::Gpu(GpuFrame::CvPixelBuffer {
            buffer: buffer.as_ptr().cast(),
            // The import reads the range from the pixel format and the
            // matrix from the buffer's attachment.
            color: YuvColor::default(),
        });
        let players = Arc::new(OneGpuFrame {
            frame: Mutex::new(Some(VideoFrame { size, pixels, time: None })),
            sink: Mutex::default(),
        });
        let shared: Arc<dyn VideoPlayers> = Arc::<OneGpuFrame>::clone(&players);
        let image = VideoMap::new(shared, None).render();
        let refused = players.sink.lock().as_ref().expect("a video opened").gpu_frames_refused();
        if std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("metal")) {
            assert!(!refused, "{what}: Metal refused the frame");
            assert_quarters(&image, what);
        } else {
            assert!(refused, "{what}: a backend without an import of CVPixelBuffers took one");
        }
    }

    #[test]
    fn an_nv12_pixel_buffer_draws_in_place() {
        let size = Size::new(16, 16);
        draws_or_refuses(size, nv12_buffer(size), "NV12 CVPixelBuffer");
    }

    #[test]
    fn a_bgra_pixel_buffer_draws_in_place() {
        let size = Size::new(16, 16);
        draws_or_refuses(size, bgra_buffer(size), "BGRA CVPixelBuffer");
    }
}
