// Video frames in Android hardware buffers, drawn by the backend the frontend
// renders on, or refused by one that doesn't import them; and Android's
// player drawing a video through the map.

#![expect(unsafe_code, reason = "libnativewindow is the FFI here")]

use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::time::Duration;

use mln_gfx::video::GpuFrame;
use mln_storage::response::{Reason, ResourceError, Response};
use mln_storage::{Resource, Responder};
use mln_video::FrameSink;

use super::*;

/// `AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM`.
const FORMAT_R8G8B8A8_UNORM: u32 = 1;
/// `AHARDWAREBUFFER_FORMAT_Y8Cb8Cr8_420`.
const FORMAT_Y8CB8CR8_420: u32 = 0x23;
/// `AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN`.
const USAGE_CPU_WRITE_OFTEN: u64 = 3 << 4;
/// `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE`.
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;

#[repr(C)]
#[derive(Default)]
struct Desc {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
    rfu0: u32,
    rfu1: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Plane {
    data: *mut u8,
    pixel_stride: u32,
    row_stride: u32,
}

#[repr(C)]
struct Planes {
    count: u32,
    planes: [Plane; 4],
}

struct Functions {
    _library: libloading::Library,
    allocate: unsafe extern "C" fn(*const Desc, *mut *mut c_void) -> c_int,
    describe: unsafe extern "C" fn(*const c_void, *mut Desc),
    lock: unsafe extern "C" fn(*mut c_void, u64, i32, *const c_void, *mut *mut c_void) -> c_int,
    lock_planes: unsafe extern "C" fn(*mut c_void, u64, i32, *const c_void, *mut Planes) -> c_int,
    unlock: unsafe extern "C" fn(*mut c_void, *mut i32) -> c_int,
}

fn functions() -> Functions {
    // SAFETY: a system library, and the NDK's signatures.
    unsafe {
        let library = libloading::Library::new("libnativewindow.so").unwrap();
        Functions {
            allocate: *library.get(b"AHardwareBuffer_allocate\0").unwrap(),
            describe: *library.get(b"AHardwareBuffer_describe\0").unwrap(),
            lock: *library.get(b"AHardwareBuffer_lock\0").unwrap(),
            lock_planes: *library.get(b"AHardwareBuffer_lockPlanes\0").unwrap(),
            unlock: *library.get(b"AHardwareBuffer_unlock\0").unwrap(),
            _library: library,
        }
    }
}

/// A hardware buffer of `size` and `format` that the CPU writes and the GPU
/// samples, with the reference that a frame of it takes, or `None` where
/// Android allocates none.
fn allocate(functions: &Functions, size: Size, format: u32) -> Option<NonNull<c_void>> {
    let desc = Desc {
        width: size.width,
        height: size.height,
        layers: 1,
        format,
        usage: USAGE_CPU_WRITE_OFTEN | USAGE_GPU_SAMPLED_IMAGE,
        ..Desc::default()
    };
    let mut buffer = std::ptr::null_mut();
    // SAFETY: a valid description and output.
    let result = unsafe { (functions.allocate)(&raw const desc, &raw mut buffer) };
    NonNull::new(buffer).filter(|_| result == 0)
}

/// A frame of an RGBA hardware buffer whose quarters are [`QUARTERS`].
fn rgba_buffer_frame(size: Size) -> VideoFrame {
    let functions = functions();
    let buffer = allocate(&functions, size, FORMAT_R8G8B8A8_UNORM).expect("an RGBA hardware buffer");
    let mut desc = Desc::default();
    let mut data = std::ptr::null_mut();
    // SAFETY: the buffer is alive and CPU-writable; the rows are `stride`
    // pixels apart while it is locked.
    unsafe {
        (functions.describe)(buffer.as_ptr(), &raw mut desc);
        assert_eq!(0, (functions.lock)(buffer.as_ptr(), USAGE_CPU_WRITE_OFTEN, -1, std::ptr::null(), &raw mut data));
        let data = data.cast::<u8>();
        for y in 0..size.height {
            for x in 0..size.width {
                let [r, g, b] = quarter(size, x, y);
                let pixel = data.add(((y * desc.stride + x) * 4) as usize);
                std::ptr::copy_nonoverlapping([r, g, b, 255].as_ptr(), pixel, 4);
            }
        }
        assert_eq!(0, (functions.unlock)(buffer.as_ptr(), std::ptr::null_mut()));
    }
    let color = YuvColor::default();
    VideoFrame {
        size,
        pixels: FramePixels::Gpu(GpuFrame::AndroidHardwareBuffer { buffer: buffer.as_ptr(), color }),
        time: None,
    }
}

/// A frame of a 4:2:0 YUV hardware buffer whose quarters are [`QUARTERS`] in
/// BT.601's limited range, or `None` where Android allocates none.
fn yuv_buffer_frame(size: Size) -> Option<VideoFrame> {
    let functions = functions();
    let buffer = allocate(&functions, size, FORMAT_Y8CB8CR8_420)?;
    let limited = |[r, g, b]: [u8; 3]| {
        let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        let samples = [
            16.0 + 219.0 * y / 255.0,
            128.0 + 224.0 * (b - y) / (1.772 * 255.0),
            128.0 + 224.0 * (r - y) / (1.402 * 255.0),
        ];
        samples.map(|sample| sample.round().clamp(0.0, 255.0) as u8)
    };
    let mut planes =
        Planes { count: 0, planes: [Plane { data: std::ptr::null_mut(), pixel_stride: 0, row_stride: 0 }; 4] };
    // SAFETY: the buffer is alive and CPU-writable; its planes hold their
    // samples at their strides while it is locked.
    unsafe {
        let locked =
            (functions.lock_planes)(buffer.as_ptr(), USAGE_CPU_WRITE_OFTEN, -1, std::ptr::null(), &raw mut planes);
        assert_eq!(0, locked, "locking the YUV buffer's planes");
        assert_eq!(3, planes.count);
        for (index, plane) in planes.planes[..3].iter().enumerate() {
            let (width, height, scale) = if index == 0 {
                (size.width, size.height, 1)
            } else {
                (size.width.div_ceil(2), size.height.div_ceil(2), 2)
            };
            for y in 0..height {
                for x in 0..width {
                    let sample = limited(quarter(size, x * scale, y * scale))[index];
                    *plane.data.add((y * plane.row_stride + x * plane.pixel_stride) as usize) = sample;
                }
            }
        }
        assert_eq!(0, (functions.unlock)(buffer.as_ptr(), std::ptr::null_mut()));
    }
    let color = YuvColor { matrix: YuvMatrix::Bt601, range: YuvRange::Limited };
    Some(VideoFrame {
        size,
        pixels: FramePixels::Gpu(GpuFrame::AndroidHardwareBuffer { buffer: buffer.as_ptr(), color }),
        time: None,
    })
}

/// Hands each video the frame it holds, once, and keeps its sink.
#[derive(Debug)]
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

/// Renders `frame`: the image, and whether the renderer refused the frame.
fn render_gpu(frame: VideoFrame) -> (PremultipliedImage, bool) {
    let players = Arc::new(OneGpuFrame { frame: Mutex::new(Some(frame)), sink: Mutex::new(None) });
    let image = VideoMap::new(Arc::<OneGpuFrame>::clone(&players), None).render();
    let refused = players.sink.lock().as_ref().expect("the video opened").gpu_frames_refused();
    (image, refused)
}

fn backend() -> String {
    std::env::var("MLN_RENDER_BACKEND").unwrap_or_else(|_| "webgpu".to_owned())
}

#[test]
fn an_rgba_hardware_buffer_draws_in_place_on_opengl_es() {
    let (image, refused) = render_gpu(rgba_buffer_frame(Size::new(64, 64)));
    if refused {
        assert_ne!("gl", backend(), "OpenGL ES refused the frame");
        eprintln!("{} refused the hardware buffer frame", backend());
    } else {
        assert_quarters(&image, "RGBA hardware buffer");
    }
}

#[test]
fn a_yuv_hardware_buffer_draws_in_place_on_opengl_es() {
    let Some(frame) = yuv_buffer_frame(Size::new(64, 64)) else {
        eprintln!("Android allocates no CPU-writable YUV hardware buffer here");
        return;
    };
    let (image, refused) = render_gpu(frame);
    if refused {
        assert_ne!("gl", backend(), "OpenGL ES refused the frame");
        eprintln!("{} refused the hardware buffer frame", backend());
    } else {
        // The external sampler converts as the driver knows the buffer.
        assert!(shows_quarters(&image, QUARTERS, 40), "YUV hardware buffer: {:?}", quarter_colors(&image));
        eprintln!("YUV hardware buffer: {:?}", quarter_colors(&image));
    }
}

/// `tests/fixtures/quarters.mp4` of `mln_video`: a second of 64x48 H.264, whose
/// quarters are [`QUARTERS`] for its first half and their opposites for its
/// second.
const VIDEO: &[u8] = include_bytes!("../../mln_video/tests/fixtures/quarters.mp4");
const SECOND_HALF: [[u8; 3]; 4] = [[0, 255, 255], [255, 0, 255], [255, 255, 0], [0, 0, 0]];

/// Serves [`VIDEO`] for ranges, and nothing else.
struct Video;

impl FileSource for Video {
    fn load(&self, resource: &Resource, responder: Responder) {
        let response = match resource.data_range {
            Some((first, last)) => Response {
                data: Some(Arc::new(
                    VIDEO.iter().copied().skip(first as usize).take((last - first + 1) as usize).collect(),
                )),
                total_size: Some(VIDEO.len() as u64),
                ..Response::default()
            },
            None => Response {
                error: Some(Box::new(ResourceError::new(Reason::NotFound, "no such resource"))),
                ..Response::default()
            },
        };
        std::thread::spawn(move || responder.send(response));
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

#[test]
fn the_platform_player_draws_a_video() {
    let mut map = VideoMap::new(mln_video::platform_players(), Some(Arc::new(Video)));
    // A backend that refuses the player's first frames draws the frames in
    // memory that follow.
    for _ in 0..50 {
        let image = map.render();
        if shows_quarters(&image, QUARTERS, 24) || shows_quarters(&image, SECOND_HALF, 24) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the video didn't draw: {:?}", quarter_colors(&map.render()));
}
