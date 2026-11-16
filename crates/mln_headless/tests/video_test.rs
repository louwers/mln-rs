// Video sources through Map -> HeadlessFrontend: a player that hands the map
// one frame, of each layout a player sends from memory, whose four quarters
// are four colors, drawn over the whole view. Each quarter must keep its
// color and its place, on whichever backend the frontend renders.

use std::sync::Arc;

use mln_actor::RunLoop;
use mln_gfx::video::{CpuPlane, FramePixels, VideoFrame, YuvColor, YuvMatrix, YuvRange};
use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapMode, MapOptions};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_video::{OpenRequest, VideoPlayer, VideoPlayers};
use parking_lot::Mutex;

/// The quarters' colors: top left, top right, bottom left, bottom right.
const QUARTERS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];

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

/// Renders a map whose video covers the whole view, with `frame`.
fn render(frame: VideoFrame) -> PremultipliedImage {
    // At zoom 0 the world is 512 pixels wide: the video covers the view.
    let size = Size::new(512, 512);
    let run_loop = RunLoop::new();
    let mut frontend = HeadlessFrontend::new(size, 1.0, &run_loop).expect("a headless frontend");
    let mut map = Map::new(MapOptions { map_mode: MapMode::Static, size, ..Default::default() }, &run_loop).unwrap();
    map.set_video_players(Some(Arc::new(OneFrame(Mutex::new(Some(frame))))));
    frontend.attach(&map);
    map.style_mut().load_json(
        r#"{
          "version": 8,
          "center": [0, 0],
          "zoom": 0,
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
    frontend.render(&mut map).expect("render").image
}

/// Checks the center of each quarter of the view.
fn assert_quarters(image: &PremultipliedImage, what: &str) {
    let size = image.size;
    for (x, y) in [(1, 1), (3, 1), (1, 3), (3, 3)] {
        let (px, py) = (size.width * x / 4, size.height * y / 4);
        let offset = (py * size.width + px) as usize * 4;
        let actual = &image.data[offset..offset + 4];
        let expected = QUARTERS[usize::from(x == 3) + 2 * usize::from(y == 3)];
        for channel in 0..3 {
            let difference = i32::from(actual[channel]).abs_diff(i32::from(expected[channel]));
            assert!(difference <= 3, "{what} at ({px}, {py}): {actual:?}, expected {expected:?}");
        }
        assert_eq!(255, actual[3], "{what} at ({px}, {py})");
    }
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
