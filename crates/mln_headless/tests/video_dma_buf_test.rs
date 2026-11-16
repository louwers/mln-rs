// A video frame in a dmabuf through Map -> HeadlessFrontend: an NV12 frame
// of four colored quarters in a udmabuf, which a test player hands the map.
// A backend that imports dmabufs (OpenGL, Vulkan) draws it; one that
// doesn't refuses GPU frames, and the player sends the frame in memory
// instead. Either way each
// quarter keeps its color and place.
#![cfg(target_os = "linux")]
#![expect(unsafe_code, reason = "udmabuf and memfd are system calls")]

use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_actor::RunLoop;
use mln_gfx::video::{CpuPlane, DmaBufPlane, FramePixels, GpuFrame, VideoFrame, YuvColor, YuvMatrix, YuvRange};
use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapMode, MapOptions};
use mln_util::size::Size;
use mln_video::{OpenRequest, VideoPlayer, VideoPlayers};
use parking_lot::Mutex;

const QUARTERS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
const FULL_BT601: YuvColor = YuvColor { matrix: YuvMatrix::Bt601, range: YuvRange::Full };
const DRM_FORMAT_NV12: u32 = u32::from_le_bytes(*b"NV12");
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

fn yuv([r, g, b]: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let u = 128.0 + (b - y) / (2.0 * (1.0 - 0.114));
    let v = 128.0 + (r - y) / (2.0 * (1.0 - 0.299));
    [y, u, v].map(|value| value.round().clamp(0.0, 255.0) as u8)
}

fn quarter(size: Size, x: u32, y: u32) -> [u8; 3] {
    QUARTERS[usize::from(x >= size.width / 2) + 2 * usize::from(y >= size.height / 2)]
}

/// The frame's NV12 bytes, rows `stride` apart: the luma plane, then the
/// chroma plane at `stride * height`.
fn nv12(size: Size, stride: usize) -> Vec<u8> {
    let (width, height) = (size.width as usize, size.height as usize);
    let mut bytes = vec![0; stride * height + stride * height.div_ceil(2)];
    for y in 0..height {
        for x in 0..width {
            bytes[y * stride + x] = yuv(quarter(size, x as u32, y as u32))[0];
        }
    }
    for y in 0..height.div_ceil(2) {
        for x in 0..width.div_ceil(2) {
            let [_, u, v] = yuv(quarter(size, x as u32 * 2, y as u32 * 2));
            bytes[stride * height + y * stride + x * 2] = u;
            bytes[stride * height + y * stride + x * 2 + 1] = v;
        }
    }
    bytes
}

/// `bytes` in a sealed memfd that udmabuf shares, or `None` without
/// udmabuf.
fn udmabuf(bytes: &[u8]) -> Option<OwnedFd> {
    #[repr(C)]
    struct Create {
        memfd: u32,
        flags: u32,
        offset: u64,
        size: u64,
    }
    const UDMABUF_CREATE: libc::c_ulong = 0x4018_7542;
    // SAFETY: plain system calls on descriptors this function owns.
    unsafe {
        let device = libc::open(c"/dev/udmabuf".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
        if device < 0 {
            return None;
        }
        let device = OwnedFd::from_raw_fd(device);
        let memfd = libc::memfd_create(c"video-test".as_ptr(), libc::MFD_ALLOW_SEALING | libc::MFD_CLOEXEC);
        let memfd = OwnedFd::from_raw_fd(memfd);
        let page = libc::sysconf(libc::_SC_PAGESIZE).max(4096) as usize;
        let size = bytes.len().div_ceil(page) * page;
        let mut padded = bytes.to_vec();
        padded.resize(size, 0);
        if libc::write(memfd.as_raw_fd(), padded.as_ptr().cast(), size) != size as isize
            || libc::fcntl(memfd.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_SHRINK) != 0
        {
            return None;
        }
        let create = Create { memfd: memfd.as_raw_fd() as u32, flags: 1, offset: 0, size: size as u64 };
        #[cfg_attr(target_env = "gnu", expect(trivial_numeric_casts, reason = "glibc's request is a c_ulong"))]
        let fd = libc::ioctl(device.as_raw_fd(), UDMABUF_CREATE as _, &create);
        (fd >= 0).then(|| OwnedFd::from_raw_fd(fd))
    }
}

/// Hands the map a dmabuf frame, and the same frame in memory if a
/// renderer refuses GPU frames.
#[derive(Debug)]
struct DmaBufPlayer {
    frames: Mutex<Option<(VideoFrame, VideoFrame)>>,
    refused: Arc<Mutex<Option<bool>>>,
}

struct Playing(Option<std::thread::JoinHandle<()>>);

impl VideoPlayer for Playing {
    fn play(&mut self) {}
    fn pause(&mut self) {}
    fn seek(&mut self, _seconds: f64) {}
}

impl Drop for Playing {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take() {
            let _ = thread.join();
        }
    }
}

impl VideoPlayers for DmaBufPlayer {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let (gpu, cpu) = self.frames.lock().take().ok_or("one video only")?;
        let frames = request.frames;
        frames.push(gpu);
        let refused = Arc::clone(&self.refused);
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(2) {
                if frames.gpu_frames_refused() {
                    *refused.lock() = Some(true);
                    frames.push(cpu);
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            *refused.lock() = Some(false);
        });
        Ok(Box::new(Playing(Some(thread))))
    }
}

#[test]
fn an_nv12_dmabuf_frame_draws_in_place() {
    let frame_size = Size::new(64, 64);
    let stride = 256;
    let bytes = nv12(frame_size, stride);
    let Some(fd) = udmabuf(&bytes) else {
        eprintln!("skipped: no udmabuf");
        return;
    };
    let planes = vec![
        DmaBufPlane { fd: fd.try_clone().unwrap(), offset: 0, stride: stride as u32 },
        DmaBufPlane { fd, offset: (stride * frame_size.height as usize) as u32, stride: stride as u32 },
    ];
    let gpu = VideoFrame {
        size: frame_size,
        pixels: FramePixels::Gpu(GpuFrame::DmaBuf {
            fourcc: DRM_FORMAT_NV12,
            modifier: DRM_FORMAT_MOD_LINEAR,
            size: frame_size,
            planes,
            color: FULL_BT601,
            lease: None,
        }),
        time: None,
    };
    let chroma = stride * frame_size.height as usize;
    let cpu = VideoFrame {
        size: frame_size,
        pixels: FramePixels::Nv12 {
            y: CpuPlane { data: bytes[..chroma].to_vec(), stride },
            uv: CpuPlane { data: bytes[chroma..].to_vec(), stride },
            color: FULL_BT601,
        },
        time: None,
    };
    let refused = Arc::new(Mutex::new(None));
    let players = Arc::new(DmaBufPlayer { frames: Mutex::new(Some((gpu, cpu))), refused: Arc::clone(&refused) });

    let size = Size::new(512, 512);
    let run_loop = RunLoop::new();
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop).expect("a headless frontend");
    let mut map = Map::new(MapOptions { map_mode: MapMode::Static, size, ..Default::default() }, &run_loop).unwrap();
    map.set_video_players(Some(players));
    frontend.attach(&map);
    map.style_mut().load_json(
        r#"{
          "version": 8,
          "sources": {
            "video": {
              "type": "video",
              "urls": ["fixture://video.webm"],
              "coordinates": [[-180, 85], [180, 85], [180, -85], [-180, -85]]
            }
          },
          "layers": [{"id": "video", "type": "raster", "source": "video", "paint": {"raster-fade-duration": 0}}]
        }"#,
    );
    let camera = map.style().get_default_camera();
    map.jump_to(&camera);
    let mut image = frontend.render(&mut map).expect("render").image;
    // A refused frame is drawn again once its copy in memory arrives.
    let start = Instant::now();
    while refused.lock().is_none() && start.elapsed() < Duration::from_secs(6) {
        std::thread::sleep(Duration::from_millis(20));
    }
    if *refused.lock() == Some(true) {
        eprintln!("the backend refused the dmabuf; drawn from memory");
        image = frontend.render(&mut map).expect("render").image;
    }
    for (x, y) in [(1, 1), (3, 1), (1, 3), (3, 3)] {
        let (px, py) = (size.width * x / 4, size.height * y / 4);
        let offset = (py * size.width + px) as usize * 4;
        let actual = &image.data[offset..offset + 4];
        let expected = QUARTERS[usize::from(x == 3) + 2 * usize::from(y == 3)];
        for channel in 0..3 {
            let difference = actual[channel].abs_diff(expected[channel]);
            assert!(difference <= 3, "at ({px}, {py}): {actual:?}, expected {expected:?}");
        }
    }
    let backend = std::env::var("MLN_RENDER_BACKEND").unwrap_or_default();
    if backend.starts_with("gl") || backend == "vulkan" {
        assert_eq!(Some(false), *refused.lock(), "{backend} imports dmabufs");
    }
}
