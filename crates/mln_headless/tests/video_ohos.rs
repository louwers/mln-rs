// Video frames in OpenHarmony native buffers, drawn by the backend the
// frontend renders on, or refused by one that doesn't import them; and
// OpenHarmony's player drawing a video through the map.

#![expect(unsafe_code, reason = "libnative_buffer is the FFI here")]

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mln_gfx::video::GpuFrame;
use mln_storage::response::{Reason, ResourceError, Response};
use mln_storage::{Resource, Responder};
use mln_video::FrameSink;

use super::*;

/// `NATIVEBUFFER_PIXEL_FMT_RGBA_8888`.
const FORMAT_RGBA_8888: i32 = 12;
/// `NATIVEBUFFER_PIXEL_FMT_YCBCR_420_SP`.
const FORMAT_YCBCR_420_SP: i32 = 24;
/// `NATIVEBUFFER_USAGE_CPU_WRITE`, `NATIVEBUFFER_USAGE_MEM_DMA` and
/// `NATIVEBUFFER_USAGE_HW_TEXTURE`.
const USAGE: i32 = (1 << 1) | (1 << 3) | (1 << 9);

/// `OH_NativeBuffer_Config`.
#[repr(C)]
#[derive(Default)]
struct Config {
    width: i32,
    height: i32,
    format: i32,
    usage: i32,
    stride: i32,
}

/// `OH_NativeBuffer_Plane`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Plane {
    offset: u64,
    row_stride: u32,
    column_stride: u32,
}

/// `OH_NativeBuffer_Planes`.
#[repr(C)]
#[derive(Default)]
struct Planes {
    count: u32,
    planes: [Plane; 4],
}

struct Functions {
    _library: libloading::Library,
    allocate: unsafe extern "C" fn(*const Config) -> *mut c_void,
    config: unsafe extern "C" fn(*mut c_void, *mut Config),
    map: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32,
    map_planes: unsafe extern "C" fn(*mut c_void, *mut *mut c_void, *mut Planes) -> i32,
    unmap: unsafe extern "C" fn(*mut c_void) -> i32,
}

/// The functions, loaded once: unloading the library would unload the code
/// of the buffers it made, which outlive a test.
fn functions() -> &'static Functions {
    static FUNCTIONS: OnceLock<Functions> = OnceLock::new();
    // SAFETY: a system library, and the SDK's signatures.
    FUNCTIONS.get_or_init(|| unsafe {
        let library = libloading::Library::new("libnative_buffer.so").unwrap();
        Functions {
            allocate: *library.get(b"OH_NativeBuffer_Alloc\0").unwrap(),
            config: *library.get(b"OH_NativeBuffer_GetConfig\0").unwrap(),
            map: *library.get(b"OH_NativeBuffer_Map\0").unwrap(),
            map_planes: *library.get(b"OH_NativeBuffer_MapPlanes\0").unwrap(),
            unmap: *library.get(b"OH_NativeBuffer_Unmap\0").unwrap(),
            _library: library,
        }
    })
}

/// A native buffer of `size` and `format` that the CPU writes and the GPU
/// samples, with the reference that a frame of it takes, or `None` where
/// the system allocates none.
fn allocate(functions: &Functions, size: Size, format: i32) -> Option<NonNull<c_void>> {
    let config = Config { width: size.width as i32, height: size.height as i32, format, usage: USAGE, stride: 0 };
    // SAFETY: a valid configuration.
    NonNull::new(unsafe { (functions.allocate)(&raw const config) })
}

/// A frame of an RGBA native buffer whose quarters are [`QUARTERS`].
fn rgba_buffer_frame(size: Size) -> VideoFrame {
    let functions = functions();
    let buffer = allocate(functions, size, FORMAT_RGBA_8888).expect("an RGBA native buffer");
    let mut config = Config::default();
    let mut data = std::ptr::null_mut();
    // SAFETY: the buffer is alive and CPU-writable; its rows are `stride`
    // bytes apart while it is mapped.
    unsafe {
        (functions.config)(buffer.as_ptr(), &raw mut config);
        assert_eq!(0, (functions.map)(buffer.as_ptr(), &raw mut data));
        let data = data.cast::<u8>();
        for y in 0..size.height {
            for x in 0..size.width {
                let [r, g, b] = quarter(size, x, y);
                let pixel = data.add(y as usize * config.stride as usize + x as usize * 4);
                std::ptr::copy_nonoverlapping([r, g, b, 255].as_ptr(), pixel, 4);
            }
        }
        assert_eq!(0, (functions.unmap)(buffer.as_ptr()));
    }
    let color = YuvColor::default();
    VideoFrame {
        size,
        pixels: FramePixels::Gpu(GpuFrame::OhosNativeBuffer { buffer: buffer.as_ptr(), color, lease: None }),
        time: None,
    }
}

/// A frame of an NV12 native buffer whose quarters are [`QUARTERS`] in
/// BT.601's limited range, or `None` where the system allocates none.
fn nv12_buffer_frame(size: Size) -> Option<VideoFrame> {
    let functions = functions();
    let buffer = allocate(functions, size, FORMAT_YCBCR_420_SP)?;
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
    let mut planes = Planes::default();
    let mut data = std::ptr::null_mut();
    // SAFETY: the buffer is alive and CPU-writable; its planes hold their
    // samples at their strides while it is mapped.
    unsafe {
        let mapped = (functions.map_planes)(buffer.as_ptr(), &raw mut data, &raw mut planes);
        assert_eq!(0, mapped, "mapping the NV12 buffer's planes");
        assert!(planes.count >= 2, "an NV12 buffer of {} planes", planes.count);
        let data = data.cast::<u8>();
        let [luma, cb, cr, _] = planes.planes;
        // The Cr samples of a plane of interleaved chroma follow the Cb.
        let cr = if planes.count >= 3 { cr } else { Plane { offset: cb.offset + 1, ..cb } };
        for y in 0..size.height {
            for x in 0..size.width {
                let at = luma.offset as usize + (y * luma.row_stride + x * luma.column_stride) as usize;
                *data.add(at) = limited(quarter(size, x, y))[0];
            }
        }
        for y in 0..size.height.div_ceil(2) {
            for x in 0..size.width.div_ceil(2) {
                let [_, u, v] = limited(quarter(size, x * 2, y * 2));
                let at =
                    |plane: Plane| plane.offset as usize + (y * plane.row_stride + x * plane.column_stride) as usize;
                *data.add(at(cb)) = u;
                *data.add(at(cr)) = v;
            }
        }
        assert_eq!(0, (functions.unmap)(buffer.as_ptr()));
    }
    let color = YuvColor { matrix: YuvMatrix::Bt601, range: YuvRange::Limited };
    Some(VideoFrame {
        size,
        pixels: FramePixels::Gpu(GpuFrame::OhosNativeBuffer { buffer: buffer.as_ptr(), color, lease: None }),
        time: None,
    })
}

/// Hands each video the frame it holds, once, and keeps its sink. Once the
/// renderer refuses the frame, sends the same quarters in memory, as a
/// player does, so that a still render that waits for a frame it can draw
/// ends.
#[derive(Debug)]
struct OneGpuFrame {
    frame: Mutex<Option<VideoFrame>>,
    sink: Mutex<Option<FrameSink>>,
}

/// Sends the frame in memory once the renderer refuses the GPU one, until it
/// drops.
struct Fallback {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl VideoPlayer for Fallback {
    fn play(&mut self) {}
    fn pause(&mut self) {}
    fn seek(&mut self, _seconds: f64) {}
}

impl Drop for Fallback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

impl VideoPlayers for OneGpuFrame {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let frame = self.frame.lock().take().ok_or("one video only")?;
        let size = frame.size;
        request.frames.push(frame);
        *self.sink.lock() = Some(request.frames.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                if request.frames.gpu_frames_refused() {
                    request.frames.push(rgba_frame(size));
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Ok(Box::new(Fallback { stop, thread: Some(thread) }))
    }
}

/// Renders `frame`: the image, and whether the renderer refused the frame
/// (and drew the same quarters from memory).
fn render_gpu(frame: VideoFrame) -> (PremultipliedImage, bool) {
    let players = Arc::new(OneGpuFrame { frame: Mutex::new(Some(frame)), sink: Mutex::new(None) });
    let image = VideoMap::new(Arc::<OneGpuFrame>::clone(&players), None).render();
    let refused = players.sink.lock().as_ref().expect("the video opened").gpu_frames_refused();
    (image, refused)
}

fn backend() -> String {
    std::env::var("MLN_RENDER_BACKEND").unwrap_or_else(|_| "webgpu".to_owned())
}

/// Whether the default EGL display imports native buffers
/// (`EGL_OHOS_image_native_buffer`), as the OpenGL ES backend needs. The
/// emulator's software EGL doesn't.
fn egl_imports_native_buffers() -> bool {
    use khronos_egl as egl;
    // SAFETY: loads the system's EGL, which the OpenGL ES device loads too.
    let Ok(egl) = (unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required() }) else { return false };
    // SAFETY: the default display, which every EGL has.
    let Some(display) = (unsafe { egl.get_display(egl::DEFAULT_DISPLAY) }) else { return false };
    if egl.initialize(display).is_err() {
        return false;
    }
    let extensions = egl.query_string(Some(display), egl::EXTENSIONS);
    extensions.is_ok_and(|extensions| {
        extensions.to_string_lossy().split(' ').any(|extension| extension == "EGL_OHOS_image_native_buffer")
    })
}

/// Checks that OpenGL ES drew a frame that its EGL imports, and says which
/// backend refused one.
fn check_refusal(refused: bool) {
    if !refused {
        return;
    }
    if backend() == "gl" {
        assert!(!egl_imports_native_buffers(), "OpenGL ES refused the frame, though EGL imports native buffers");
        eprintln!("EGL imports no native buffers here: OpenGL ES refused the frame");
    } else {
        eprintln!("{} refused the native buffer frame", backend());
    }
}

#[test]
fn an_rgba_native_buffer_draws_in_place_on_opengl_es() {
    let (image, refused) = render_gpu(rgba_buffer_frame(Size::new(64, 64)));
    check_refusal(refused);
    assert_quarters(&image, "RGBA native buffer");
}

#[test]
fn an_nv12_native_buffer_draws_in_place_on_opengl_es() {
    let Some(frame) = nv12_buffer_frame(Size::new(64, 64)) else {
        eprintln!("OpenHarmony allocates no CPU-writable NV12 native buffer here");
        return;
    };
    let (image, refused) = render_gpu(frame);
    if refused {
        check_refusal(refused);
        assert_quarters(&image, "the frame in memory after a refusal");
    } else {
        // The external sampler converts as the driver knows the buffer.
        assert!(shows_quarters(&image, QUARTERS, 40), "NV12 native buffer: {:?}", quarter_colors(&image));
        eprintln!("NV12 native buffer: {:?}", quarter_colors(&image));
    }
}

/// `tests/fixtures/quarters-android.mp4` of `mln_video`, which `OpenHarmony`'s
/// decoder plays too: a second of 64x48 H.264, whose quarters are red,
/// green, blue and white for its first half and their opposites for its
/// second.
const VIDEO: &[u8] = include_bytes!("../../mln_video/tests/fixtures/quarters-android.mp4");
const FIRST_HALF: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
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
        if shows_quarters(&image, FIRST_HALF, 24) || shows_quarters(&image, SECOND_HALF, 24) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the video didn't draw: {:?}", quarter_colors(&map.render()));
}
