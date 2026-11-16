//! The GStreamer player, playing small videos that a test file source
//! serves. Each test passes, saying it was skipped, where GStreamer 1.x
//! isn't installed.
//!
//! The fixtures are WebM videos of 64 by 48 pixels at 10 frames a second:
//! half a second of red, then of green, then of blue. They were made with
//!
//! ```sh
//! gst-launch-1.0 concat name=c ! vp9enc deadline=1 target-bitrate=200000 ! webmmux ! filesink location=colors-vp9.webm \
//!   videotestsrc pattern=red num-buffers=5 ! video/x-raw,format=I420,width=64,height=48,framerate=10/1 ! c. \
//!   videotestsrc pattern=green num-buffers=5 ! video/x-raw,format=I420,width=64,height=48,framerate=10/1 ! c. \
//!   videotestsrc pattern=blue num-buffers=5 ! video/x-raw,format=I420,width=64,height=48,framerate=10/1 ! c.
//! ```
//!
//! and the same with `vp8enc` for `colors-vp8.webm`.

#![cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_storage::response::Reason;
use mln_storage::{FileSource, Resource, ResourceError, Responder, Response};
use mln_util::hash::HashMap;
use mln_video::{
    FramePixels, FrameSink, FrameSlot, GpuFrame, Newest, OpenRequest, VideoFrame, VideoPlayer, YuvColor,
    platform_players,
};

const VP9: &[u8] = include_bytes!("fixtures/colors-vp9.webm");
const VP8: &[u8] = include_bytes!("fixtures/colors-vp8.webm");

const RED: [f32; 3] = [1.0, 0.0, 0.0];
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];

/// Serves files by URL as a local file source does: ranges padded with
/// zeros, the size known. Other URLs aren't found.
struct Files(HashMap<String, Vec<u8>>);

impl FileSource for Files {
    fn load(&self, resource: &Resource, responder: Responder) {
        let response = match (self.0.get(&resource.url), resource.data_range) {
            (Some(bytes), Some((first, last))) => {
                let mut data: Vec<u8> =
                    bytes.iter().copied().skip(first as usize).take((last - first + 1) as usize).collect();
                data.resize((last - first + 1) as usize, 0);
                Response { data: Some(Arc::new(data)), total_size: Some(bytes.len() as u64), ..Response::default() }
            }
            _ => Response {
                error: Some(Box::new(ResourceError::new(Reason::NotFound, "not found"))),
                ..Response::default()
            },
        };
        std::thread::spawn(move || responder.send(response));
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

fn files(entries: &[(&str, &[u8])]) -> Arc<dyn FileSource> {
    Arc::new(Files(entries.iter().map(|(url, bytes)| ((*url).to_owned(), bytes.to_vec())).collect()))
}

/// A player of `urls`, sending frames in memory unless `gpu`, or `None`
/// where GStreamer isn't installed.
fn open(file_source: Arc<dyn FileSource>, urls: &[&str], gpu: bool) -> Option<(Box<dyn VideoPlayer>, Arc<FrameSlot>)> {
    let slot = FrameSlot::new();
    if !gpu {
        slot.refuse_gpu_frames();
    }
    let request = OpenRequest {
        urls: urls.iter().map(|url| (*url).to_owned()).collect(),
        file_source: Some(file_source),
        frames: FrameSink::new(Arc::clone(&slot)),
    };
    match platform_players().open(request) {
        Ok(player) => Some((player, slot)),
        Err(error) if error.contains("isn't installed") => {
            eprintln!("skipped: {error}");
            None
        }
        Err(error) => panic!("the player didn't open: {error}"),
    }
}

/// The first frame within `timeout` that `wanted` accepts.
fn wait_for(slot: &FrameSlot, timeout: Duration, wanted: impl Fn(&VideoFrame) -> bool) -> Option<Newest> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(failure) = slot.failure() {
            panic!("the video failed: {failure}");
        }
        if let Some(newest) = slot.newest().filter(|newest| wanted(&newest.frame)) {
            return Some(newest);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

fn time_between(frame: &VideoFrame, start: f64, end: f64) -> bool {
    frame.time.is_some_and(|time| (start..end).contains(&time))
}

/// The straight RGB of the pixel at `x`, `y` of a frame in memory.
fn rgb_at(frame: &VideoFrame, x: usize, y: usize) -> [f32; 3] {
    let yuv = |color: YuvColor, samples: [u8; 3]| {
        let sample = samples.map(|s| f32::from(s) / 255.0);
        color.to_rgb_rows().map(|row| row[0] * sample[0] + row[1] * sample[1] + row[2] * sample[2] + row[3])
    };
    match &frame.pixels {
        FramePixels::Rgba(plane) => {
            let at = y * plane.stride + x * 4;
            [0, 1, 2].map(|channel| f32::from(plane.data[at + channel]) / 255.0)
        }
        FramePixels::Nv12 { y: luma, uv, color } => {
            let at = (y / 2) * uv.stride + (x / 2) * 2;
            yuv(*color, [luma.data[y * luma.stride + x], uv.data[at], uv.data[at + 1]])
        }
        FramePixels::I420 { y: luma, u, v, color } => {
            let at = |plane: &mln_video::CpuPlane| plane.data[(y / 2) * plane.stride + x / 2];
            yuv(*color, [luma.data[y * luma.stride + x], at(u), at(v)])
        }
        _ => panic!("a frame in memory, not on the GPU"),
    }
}

fn assert_color(frame: &VideoFrame, expected: [f32; 3]) {
    let actual = rgb_at(frame, 32, 24);
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 0.1, "the frame at {:?}s is {actual:?}, not {expected:?}", frame.time);
    }
}

#[test]
fn frames_arrive_in_memory_at_the_videos_size_and_colors() {
    for (name, bytes) in [("vp9", VP9), ("vp8", VP8)] {
        let url = format!("https://example.com/{name}.webm");
        let Some((_player, slot)) = open(files(&[(&url, bytes)]), &[&url], false) else { return };
        let first = wait_for(&slot, Duration::from_secs(5), |_| true).expect("a first frame");
        assert_eq!((64, 48), (first.frame.size.width, first.frame.size.height));
        assert!(!matches!(first.frame.pixels, FramePixels::Gpu(_)));
        assert_color(&first.frame, RED);
        let green = wait_for(&slot, Duration::from_secs(3), |frame| time_between(frame, 0.55, 0.95));
        assert_color(&green.expect("a green frame").frame, GREEN);
        let blue = wait_for(&slot, Duration::from_secs(3), |frame| time_between(frame, 1.05, 1.45));
        assert_color(&blue.expect("a blue frame").frame, BLUE);
    }
}

#[test]
fn the_video_loops() {
    let Some((_player, slot)) = open(files(&[("v", VP9)]), &["v"], false) else { return };
    wait_for(&slot, Duration::from_secs(5), |frame| time_between(frame, 1.3, 2.0)).expect("the last frames");
    let again = wait_for(&slot, Duration::from_secs(3), |frame| time_between(frame, 0.0, 0.3));
    assert_color(&again.expect("the first frames again").frame, RED);
}

#[test]
fn a_paused_video_seeks_to_the_frame_there_and_stays() {
    let Some((mut player, slot)) = open(files(&[("v", VP9)]), &["v"], false) else { return };
    wait_for(&slot, Duration::from_secs(5), |_| true).expect("a first frame");
    player.pause();
    player.seek(1.2);
    let there =
        wait_for(&slot, Duration::from_secs(3), |frame| time_between(frame, 1.15, 1.25)).expect("the frame at 1.2s");
    assert_color(&there.frame, BLUE);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(there.generation, slot.newest().unwrap().generation, "a paused video sent frames");

    player.seek(0.6);
    let there =
        wait_for(&slot, Duration::from_secs(3), |frame| time_between(frame, 0.55, 0.65)).expect("the frame at 0.6s");
    assert_color(&there.frame, GREEN);
    player.play();
    wait_for(&slot, Duration::from_secs(3), |frame| time_between(frame, 0.75, 2.0))
        .expect("frames after playing again");
}

#[test]
fn the_first_url_that_plays_plays() {
    let source = files(&[
        ("not-video", b"this isn't a video, but text: a video player can't play it".repeat(50).as_slice()),
        ("v", VP9),
    ]);
    let Some((_player, slot)) = open(source, &["missing", "not-video", "v"], false) else { return };
    let first = wait_for(&slot, Duration::from_secs(5), |_| true).expect("a frame of the last URL");
    assert_color(&first.frame, RED);
}

#[test]
fn a_video_without_a_url_that_plays_fails_through_the_sink() {
    let source = files(&[("not-video", b"this isn't a video".repeat(100).as_slice())]);
    let Some((_player, slot)) = open(source, &["missing", "not-video"], false) else { return };
    let deadline = Instant::now() + Duration::from_secs(5);
    while slot.failure().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let failure = slot.failure().expect("a failure");
    assert!(failure.contains("missing: ") && failure.contains("not-video: "), "{failure}");
    assert!(slot.newest().is_none());
}

#[test]
fn a_map_without_a_file_source_fails_to_open() {
    let request =
        OpenRequest { urls: vec!["v".to_owned()], file_source: None, frames: FrameSink::new(FrameSlot::new()) };
    assert!(platform_players().open(request).is_err());
}

#[test]
fn dropping_the_player_stops_it_promptly() {
    let Some((player, slot)) = open(files(&[("v", VP9)]), &["v"], false) else { return };
    wait_for(&slot, Duration::from_secs(5), |_| true).expect("a first frame");
    let start = Instant::now();
    drop(player);
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(500), "dropping took {elapsed:?}");
    let generation = slot.newest().unwrap().generation;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(generation, slot.newest().unwrap().generation, "a dropped player sent frames");
}

#[test]
fn dropping_a_player_that_waits_for_its_bytes_is_prompt() {
    struct Never;
    impl FileSource for Never {
        fn load(&self, _resource: &Resource, responder: Responder) {
            std::mem::forget(responder);
        }
        fn can_request(&self, _resource: &Resource) -> bool {
            true
        }
    }
    let Some((player, _slot)) = open(Arc::new(Never), &["v"], false) else { return };
    std::thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    drop(player);
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(500), "dropping took {elapsed:?}");
}

/// Whether GStreamer has a VA-API VP9 decoder, which only registers for a
/// GPU that decodes VP9.
fn has_va_vp9_decoder() -> bool {
    std::process::Command::new("gst-inspect-1.0").arg("vavp9dec").output().is_ok_and(|output| output.status.success())
}

#[test]
fn a_va_decoder_sends_dmabufs_until_a_renderer_refuses_them() {
    if !has_va_vp9_decoder() {
        eprintln!("skipped: GStreamer has no VA-API VP9 decoder");
        return;
    }
    let Some((_player, slot)) = open(files(&[("v", VP9)]), &["v"], true) else { return };
    let first = wait_for(&slot, Duration::from_secs(5), |_| true).expect("a first frame");
    let FramePixels::Gpu(GpuFrame::DmaBuf { fourcc, modifier, planes, .. }) = &first.frame.pixels else {
        panic!("a dmabuf, not a frame in memory");
    };
    eprintln!(
        "dmabuf: {} modifier {modifier:#018x}, planes {:?}",
        String::from_utf8_lossy(&fourcc.to_le_bytes()),
        planes.iter().map(|plane| (plane.offset, plane.stride)).collect::<Vec<_>>()
    );
    assert!(!planes.is_empty());

    slot.refuse_gpu_frames();
    let in_memory = wait_for(&slot, Duration::from_secs(5), |frame| !matches!(frame.pixels, FramePixels::Gpu(_)))
        .expect("a frame in memory");
    assert_eq!((64, 48), (in_memory.frame.size.width, in_memory.frame.size.height));
    let time = in_memory.frame.time.expect("a time");
    let expected = if time < 0.5 {
        RED
    } else if time < 1.0 {
        GREEN
    } else {
        BLUE
    };
    assert_color(&in_memory.frame, expected);
}

/// Plays the file at `MLN_VIDEO_FILE` for `MLN_VIDEO_SECONDS` (3 by
/// default), with GPU frames unless `MLN_VIDEO_GPU=0`, and prints what
/// came: for trying the player on the videos and GPUs at hand.
#[test]
#[ignore = "plays a file that the environment names"]
fn plays_a_file() {
    let path = std::env::var("MLN_VIDEO_FILE").expect("MLN_VIDEO_FILE");
    let seconds: f64 = std::env::var("MLN_VIDEO_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let gpu = std::env::var("MLN_VIDEO_GPU").map_or(true, |value| value != "0");
    let bytes = std::fs::read(&path).expect("the file");
    let Some((_player, slot)) = open(files(&[("file", &bytes)]), &["file"], gpu) else { return };
    let start = Instant::now();
    let (mut seen, mut frames) = (0, 0);
    while start.elapsed().as_secs_f64() < seconds {
        if let Some(failure) = slot.failure() {
            panic!("the video failed: {failure}");
        }
        if let Some(newest) = slot.newer_than(seen) {
            seen = newest.generation;
            frames += 1;
            if frames == 1 {
                let kind = match &newest.frame.pixels {
                    FramePixels::Gpu(GpuFrame::DmaBuf { fourcc, modifier, planes, .. }) => format!(
                        "dmabuf {} {modifier:#018x} {:?}",
                        String::from_utf8_lossy(&fourcc.to_le_bytes()),
                        planes.iter().map(|plane| (plane.offset, plane.stride)).collect::<Vec<_>>()
                    ),
                    FramePixels::Nv12 { color, .. } => format!("NV12 {color:?}"),
                    FramePixels::I420 { color, .. } => format!("I420 {color:?}"),
                    FramePixels::Rgba(_) => "RGBA".to_owned(),
                    _ => "another kind".to_owned(),
                };
                eprintln!("first frame: {:?}, {kind}", newest.frame.size);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let last = slot.newest().map(|newest| newest.frame.time);
    eprintln!("{frames} frames in {seconds}s, the last at {last:?}");
    assert!(frames > 0);
}
