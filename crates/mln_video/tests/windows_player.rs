// Media Foundation's player (`platform_players` on Windows), playing
// tests/fixtures/quarters-windows.mp4 through a FileSource: a 128x128 H.264 video of
// one second at 30 frames a second whose quarters are red, green, blue and
// white for its first half, and mirrored left to right for its second.
// `write_fixture` made it.

#![cfg(windows)]
#![expect(unsafe_code, reason = "the tests read frames and write the fixture through the windows crate")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_storage::{FileSource, Resource, Responder, Response};
use mln_util::size::Size;
use mln_video::{FramePixels, FrameSink, FrameSlot, GpuFrame, Newest, OpenRequest, VideoPlayer, platform_players};

const FIXTURE: &[u8] = include_bytes!("fixtures/quarters-windows.mp4");
const SIZE: Size = Size::new(128, 128);
const FRAMES: u32 = 30;
/// The quarters' colors: top left, top right, bottom left, bottom right.
const QUARTERS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
/// How long a test waits for what a player does.
const PATIENCE: Duration = Duration::from_secs(20);

/// The color of `(x, y)` in the frame `index`, mirrored in the second half.
fn color(index: u32, x: u32, y: u32) -> [u8; 3] {
    let right = (x >= SIZE.width / 2) != (index >= FRAMES / 2);
    QUARTERS[usize::from(right) + 2 * usize::from(y >= SIZE.height / 2)]
}

/// Serves `bytes` as a local file: ranges padded with zeros, the size known.
struct Bytes(Arc<Vec<u8>>);

impl FileSource for Bytes {
    fn load(&self, resource: &Resource, responder: Responder) {
        let (first, last) = resource.data_range.unwrap_or((0, self.0.len() as u64 - 1));
        let mut data: Vec<u8> = self.0.iter().copied().skip(first as usize).take((last - first + 1) as usize).collect();
        data.resize((last - first + 1) as usize, 0);
        let response =
            Response { data: Some(Arc::new(data)), total_size: Some(self.0.len() as u64), ..Response::default() };
        std::thread::spawn(move || responder.send(response));
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

/// Never answers.
struct Never;

impl FileSource for Never {
    fn load(&self, _resource: &Resource, responder: Responder) {
        std::mem::forget(responder);
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

fn open(file_source: Arc<dyn FileSource>, gpu: bool) -> (Arc<FrameSlot>, Box<dyn VideoPlayer>) {
    let slot = FrameSlot::new();
    if !gpu {
        slot.refuse_gpu_frames();
    }
    let request = OpenRequest {
        urls: vec!["https://example.com/quarters.mp4".to_owned()],
        file_source: Some(file_source),
        frames: FrameSink::new(Arc::clone(&slot)),
        url_transform: None,
    };
    let player = platform_players().open(request).expect("a player");
    (slot, player)
}

fn fixture() -> Arc<dyn FileSource> {
    Arc::new(Bytes(Arc::new(FIXTURE.to_vec())))
}

/// The first frame newer than `seen` that `accept` takes, within the
/// tests' patience.
fn wait_for(slot: &FrameSlot, seen: u64, accept: impl Fn(&Newest) -> bool) -> Newest {
    let start = Instant::now();
    let mut seen = seen;
    while start.elapsed() < PATIENCE {
        assert_eq!(None, slot.failure(), "the video failed");
        if let Some(newest) = slot.newer_than(seen) {
            if accept(&newest) {
                return newest;
            }
            seen = newest.generation;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no frame came");
}

/// The index of the fixture's frame at `time`.
fn index_at(time: f64) -> u32 {
    ((time * f64::from(FRAMES)).round() as u32).min(FRAMES - 1)
}

/// Checks the center of each quarter of `rgba`, rows `stride` bytes apart,
/// against the frame `index`.
fn assert_quarters(rgba: &[u8], stride: usize, index: u32) {
    for (x, y) in [(1, 1), (3, 1), (1, 3), (3, 3)] {
        let (px, py) = (SIZE.width * x / 4, SIZE.height * y / 4);
        let at = py as usize * stride + px as usize * 4;
        let actual = &rgba[at..at + 4];
        let expected = color(index, px, py);
        for channel in 0..3 {
            let difference = actual[channel].abs_diff(expected[channel]);
            assert!(difference <= 24, "frame {index} at ({px}, {py}): {actual:?}, expected {expected:?}");
        }
        assert_eq!(255, actual[3]);
    }
}

fn assert_cpu_frame(newest: &Newest) {
    let frame = &newest.frame;
    assert_eq!(SIZE, frame.size);
    let FramePixels::Rgba(plane) = &frame.pixels else { panic!("a frame in memory, not {:?}", frame.pixels) };
    assert_quarters(&plane.data, plane.stride, index_at(frame.time.expect("the frame's time")));
}

#[test]
fn frames_in_memory_have_the_videos_size_and_colors() {
    let (slot, _player) = open(fixture(), false);
    assert_cpu_frame(&wait_for(&slot, 0, |_| true));
    // A frame of the second half, which plays on.
    assert_cpu_frame(&wait_for(&slot, 0, |newest| newest.frame.time.is_some_and(|time| time > 0.6)));
}

#[test]
fn gpu_frames_share_textures_of_the_videos_colors() {
    let (slot, _player) = open(fixture(), true);
    let newest = wait_for(&slot, 0, |_| true);
    let frame = &newest.frame;
    assert_eq!(SIZE, frame.size);
    let FramePixels::Gpu(GpuFrame::DxgiSharedHandle { handle, .. }) = &frame.pixels else {
        panic!("a shared texture, not {:?}", frame.pixels)
    };
    let (rgba, stride) = d3d11::read_shared(handle, SIZE);
    assert_quarters(&rgba, stride, index_at(frame.time.expect("the frame's time")));
}

#[test]
fn a_paused_seek_sends_the_frame_there() {
    let (slot, mut player) = open(fixture(), false);
    let first = wait_for(&slot, 0, |_| true);
    player.pause();
    for time in [0.8, 0.2] {
        player.seek(time);
        let newest =
            wait_for(&slot, first.generation, |newest| newest.frame.time.is_some_and(|at| (at - time).abs() < 0.05));
        assert_cpu_frame(&newest);
    }
}

#[test]
fn a_file_that_isnt_video_fails() {
    let bogus: Vec<u8> = (0..20_000_u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    let (slot, _player) = open(Arc::new(Bytes(Arc::new(bogus))), true);
    let start = Instant::now();
    while slot.failure().is_none() {
        assert!(start.elapsed() < PATIENCE, "the bogus video didn't fail");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(slot.newest().is_none());
}

#[test]
fn dropping_a_player_is_prompt() {
    let (slot, player) = open(fixture(), true);
    wait_for(&slot, 0, |_| true);
    let start = Instant::now();
    drop(player);
    assert!(start.elapsed() < Duration::from_secs(1), "dropping a playing player took {:?}", start.elapsed());

    // A player whose bytes never come.
    let (_slot, player) = open(Arc::new(Never), true);
    std::thread::sleep(Duration::from_millis(200));
    let start = Instant::now();
    drop(player);
    assert!(start.elapsed() < Duration::from_secs(1), "dropping a waiting player took {:?}", start.elapsed());
}

/// Writes tests/fixtures/quarters-windows.mp4 with Media Foundation's H.264
/// encoder: `cargo test -p mln-rs-video --test windows_player -- --ignored`.
#[test]
#[ignore = "writes the fixture"]
fn write_fixture() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quarters-windows.mp4");
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the fixtures' directory");
    d3d11::write_h264(&path);
}

mod d3d11 {
    use std::os::windows::io::{AsRawHandle as _, OwnedHandle};
    use std::path::Path;

    use mln_util::size::Size;
    use windows::Win32::Foundation::{HANDLE, HMODULE};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
        D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device1,
        ID3D11Texture2D,
    };
    use windows::Win32::Media::MediaFoundation::{
        MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
        MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_YUV_MATRIX,
        MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL,
        MFMediaType_Video, MFNominalRange_16_235, MFSTARTUP_LITE, MFShutdown, MFStartup, MFVideoFormat_H264,
        MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoTransferMatrix_BT601, eAVEncH264VProfile_Base,
    };
    use windows::core::{HSTRING, Interface as _};

    use super::{FRAMES, SIZE, color};

    /// The texture of a shared handle's pixels as RGBA, and its stride.
    pub(super) fn read_shared(handle: &OwnedHandle, size: Size) -> (Vec<u8>, usize) {
        let (mut device, mut context) = (None, None);
        // SAFETY: the calls fill their out-pointers, and the mapping holds
        // `size.height` rows `RowPitch` bytes apart until the unmap.
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
            .expect("a D3D11 device");
            let (device, context) = (device.expect("a device"), context.expect("a context"));
            let device: ID3D11Device1 = device.cast().expect("ID3D11Device1");
            let shared: ID3D11Texture2D =
                device.OpenSharedResource1(HANDLE(handle.as_raw_handle())).expect("the shared texture");
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            shared.GetDesc(&raw mut desc);
            assert_eq!((size.width, size.height), (desc.Width, desc.Height));
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&raw const desc, None, Some(&raw mut staging)).expect("a staging texture");
            let staging = staging.expect("a staging texture");
            context.CopyResource(&staging, &shared);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped)).expect("a mapping");
            let stride = size.width as usize * 4;
            let mut rgba = vec![0; stride * size.height as usize];
            for (row, out) in rgba.chunks_exact_mut(stride).enumerate() {
                let bgra =
                    std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(row * mapped.RowPitch as usize), stride);
                for (pixel, bgra) in out.as_chunks_mut::<4>().0.iter_mut().zip(bgra.as_chunks::<4>().0) {
                    pixel.copy_from_slice(&[bgra[2], bgra[1], bgra[0], bgra[3]]);
                }
            }
            context.Unmap(&staging, 0);
            (rgba, stride)
        }
    }

    /// Limited-range BT.601 YUV of an RGB color.
    fn yuv([r, g, b]: [u8; 3]) -> [u8; 3] {
        let [r, g, b] = [r, g, b].map(|value| f64::from(value) / 255.0);
        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        let (u, v) = ((b - y) / 1.772, (r - y) / 1.402);
        [16.0 + 219.0 * y, 128.0 + 224.0 * u, 128.0 + 224.0 * v].map(|value| value.round().clamp(0.0, 255.0) as u8)
    }

    /// The NV12 bytes of frame `index`.
    fn nv12(index: u32) -> Vec<u8> {
        let (width, height) = (SIZE.width, SIZE.height);
        let mut data = vec![0; (width * height * 3 / 2) as usize];
        for y in 0..height {
            for x in 0..width {
                data[(y * width + x) as usize] = yuv(color(index, x, y))[0];
            }
        }
        for y in 0..height / 2 {
            for x in 0..width / 2 {
                let [_, u, v] = yuv(color(index, x * 2, y * 2));
                let at = (width * height + y * width + x * 2) as usize;
                data[at] = u;
                data[at + 1] = v;
            }
        }
        data
    }

    pub(super) fn write_h264(path: &Path) {
        let pack = |high: u32, low: u32| (u64::from(high) << 32) | u64::from(low);
        // SAFETY: plain calls of live Media Foundation objects; each buffer
        // is locked while it's written.
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_LITE).expect("Media Foundation");
            let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, None).expect("a writer");
            let output = MFCreateMediaType().expect("a media type");
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).unwrap();
            output.SetUINT32(&MF_MT_AVG_BITRATE, 200_000).unwrap();
            output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32).unwrap();
            let input = MFCreateMediaType().expect("a media type");
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
            for media_type in [&output, &input] {
                media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).unwrap();
                media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack(SIZE.width, SIZE.height)).unwrap();
                media_type.SetUINT64(&MF_MT_FRAME_RATE, pack(FRAMES, 1)).unwrap();
                media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).unwrap();
                media_type.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32).unwrap();
                media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32).unwrap();
            }
            let stream = writer.AddStream(&output).expect("a stream");
            writer.SetInputMediaType(stream, &input, None).expect("the input type");
            writer.BeginWriting().expect("writing");
            let duration = 10_000_000 / i64::from(FRAMES);
            for index in 0..FRAMES {
                let bytes = nv12(index);
                let buffer = MFCreateMemoryBuffer(bytes.len() as u32).expect("a buffer");
                let mut data = std::ptr::null_mut();
                buffer.Lock(&raw mut data, None, None).unwrap();
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
                buffer.Unlock().unwrap();
                buffer.SetCurrentLength(bytes.len() as u32).unwrap();
                let sample = MFCreateSample().expect("a sample");
                sample.AddBuffer(&buffer).unwrap();
                sample.SetSampleTime(i64::from(index) * duration).unwrap();
                sample.SetSampleDuration(duration).unwrap();
                writer.WriteSample(stream, &sample).expect("a frame");
            }
            writer.Finalize().expect("the file");
            MFShutdown().unwrap();
        }
    }
}
