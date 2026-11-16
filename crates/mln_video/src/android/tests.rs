// The player on a device: tests/fixtures/quarters.mp4 is a second of 64x48
// H.264 at 30 frames a second, BT.601 limited range, whose quarters are red,
// green, blue and white for its first half and cyan, magenta, yellow and
// black for its second. It was encoded from raw frames with
// `gst-launch-1.0 filesrc location=quarters.yuv ! rawvideoparse width=64
// height=48 format=i420 framerate=30/1 ! openh264enc gop-size=10 !
// h264parse ! mp4mux faststart=true ! filesink location=quarters.mp4`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_storage::response::{Reason, ResourceError, Response};
use mln_storage::{FileSource, Resource, Responder};

use super::*;
use crate::slot::FrameSlot;

const VIDEO: &[u8] = include_bytes!("../../tests/fixtures/quarters.mp4");
const FIRST_HALF: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
const SECOND_HALF: [[u8; 3]; 4] = [[0, 255, 255], [255, 0, 255], [255, 255, 0], [0, 0, 0]];

/// Serves `bytes` for every URL as a local file would, or fails each load
/// when it has none.
struct Bytes(Option<&'static [u8]>);

impl FileSource for Bytes {
    fn load(&self, resource: &Resource, responder: Responder) {
        let response = match (self.0, resource.data_range) {
            (Some(bytes), Some((first, last))) => {
                let data = bytes.iter().copied().skip(first as usize).take((last - first + 1) as usize).collect();
                Response { data: Some(Arc::new(data)), total_size: Some(bytes.len() as u64), ..Response::default() }
            }
            _ => Response {
                error: Some(Box::new(ResourceError::new(Reason::NotFound, "no such video"))),
                ..Response::default()
            },
        };
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

/// A player of `file_source`'s video, and its frames, in memory with `cpu`.
fn open(file_source: impl FileSource + 'static, cpu: bool) -> (Box<dyn VideoPlayer>, Arc<FrameSlot>) {
    let slot = FrameSlot::new();
    if cpu {
        slot.refuse_gpu_frames();
    }
    let request = OpenRequest {
        urls: vec!["file:///quarters.mp4".to_owned()],
        file_source: Some(Arc::new(file_source)),
        frames: FrameSink::new(Arc::clone(&slot)),
    };
    let player = AndroidPlayers.open(request).unwrap();
    (player, slot)
}

/// Waits for a frame after generation `seen` that `accept` takes.
fn wait_for(slot: &FrameSlot, seen: u64, accept: impl Fn(&VideoFrame) -> bool) -> crate::Newest {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(failure) = slot.failure() {
            panic!("the video failed: {failure}");
        }
        if let Some(newest) = slot.newer_than(seen)
            && newest.generation > seen
            && accept(&newest.frame)
        {
            return newest;
        }
        assert!(Instant::now() < deadline, "no such frame came");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The RGB color at the center of each quarter of a frame in memory.
fn quarters(frame: &VideoFrame) -> [[f32; 3]; 4] {
    let (width, height) = (frame.size.width as usize, frame.size.height as usize);
    let sample = |x: usize, y: usize| -> ([u8; 3], YuvColor) {
        match &frame.pixels {
            FramePixels::I420 { y: luma, u, v, color } => {
                let chroma = (y / 2) * u.stride + x / 2;
                ([luma.data[y * luma.stride + x], u.data[chroma], v.data[(y / 2) * v.stride + x / 2]], *color)
            }
            FramePixels::Nv12 { y: luma, uv, color } => {
                let chroma = (y / 2) * uv.stride + (x / 2) * 2;
                ([luma.data[y * luma.stride + x], uv.data[chroma], uv.data[chroma + 1]], *color)
            }
            other => panic!("a frame in memory, not {other:?}"),
        }
    };
    [(1, 1), (3, 1), (1, 3), (3, 3)].map(|(qx, qy)| {
        let (yuv, color) = sample(width * qx / 4, height * qy / 4);
        let samples = [yuv[0], yuv[1], yuv[2]].map(|s| f32::from(s) / 255.0);
        color.to_rgb_rows().map(|row| {
            let rgb = row[0] * samples[0] + row[1] * samples[1] + row[2] * samples[2] + row[3];
            (rgb * 255.0).clamp(0.0, 255.0)
        })
    })
}

fn shows(frame: &VideoFrame, expected: [[u8; 3]; 4]) -> bool {
    quarters(frame)
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.iter().zip(expected).all(|(a, e)| (a - f32::from(e)).abs() < 24.0))
}

#[test]
fn frames_in_memory_show_the_video_and_loop() {
    let (_player, slot) = open(Bytes(Some(VIDEO)), true);
    let first = wait_for(&slot, 0, |_| true);
    assert_eq!(Size::new(64, 48), first.frame.size);
    assert!(shows(&first.frame, FIRST_HALF), "the first frame shows {:?}", quarters(&first.frame));
    let second_half = wait_for(&slot, first.generation, |frame| frame.time.is_some_and(|time| time >= 0.6));
    assert!(shows(&second_half.frame, SECOND_HALF), "a later frame shows {:?}", quarters(&second_half.frame));
    // The video starts over after a second.
    let again = wait_for(&slot, second_half.generation, |frame| frame.time.is_some_and(|time| time < 0.2));
    assert!(shows(&again.frame, FIRST_HALF));
}

#[test]
fn frames_come_on_time() {
    let (_player, slot) = open(Bytes(Some(VIDEO)), true);
    let first = wait_for(&slot, 0, |_| true);
    let start = Instant::now();
    wait_for(&slot, first.generation, |frame| frame.time.is_some_and(|time| time >= 0.5));
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(400) && elapsed < Duration::from_millis(1000),
        "half a second of video took {elapsed:?}"
    );
}

#[test]
fn frames_on_the_gpu_are_hardware_buffers_until_refused() {
    let (_player, slot) = open(Bytes(Some(VIDEO)), false);
    let first = wait_for(&slot, 0, |_| true);
    assert_eq!(Size::new(64, 48), first.frame.size);
    match &first.frame.pixels {
        FramePixels::Gpu(GpuFrame::AndroidHardwareBuffer { buffer, .. }) => assert!(!buffer.is_null()),
        other => panic!("a hardware buffer, not {other:?}"),
    }
    // Frames keep coming, and the old ones go.
    let later = wait_for(&slot, first.generation + 10, |_| true);
    drop(first);
    slot.refuse_gpu_frames();
    let in_memory = wait_for(&slot, later.generation, |frame| !matches!(frame.pixels, FramePixels::Gpu(_)));
    assert_eq!(Size::new(64, 48), in_memory.frame.size);
}

#[test]
fn a_paused_seek_shows_the_frame_there() {
    for cpu in [true, false] {
        let (mut player, slot) = open(Bytes(Some(VIDEO)), cpu);
        let first = wait_for(&slot, 0, |_| true);
        player.pause();
        // Frames stop.
        std::thread::sleep(Duration::from_millis(100));
        let paused = slot.newest().unwrap().generation;
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(paused, slot.newest().unwrap().generation, "frames came while paused");
        assert!(paused >= first.generation);

        player.seek(0.75);
        let seeked = wait_for(&slot, paused, |_| true);
        let time = seeked.frame.time.unwrap();
        assert!((0.74..0.8).contains(&time), "the seek showed the frame at {time}");
        if cpu {
            assert!(shows(&seeked.frame, SECOND_HALF), "the seek showed {:?}", quarters(&seeked.frame));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(seeked.generation, slot.newest().unwrap().generation, "frames came after a paused seek");

        // A seek past the end shows the last frame.
        player.seek(5.0);
        let last = wait_for(&slot, seeked.generation, |_| true);
        let time = last.frame.time.unwrap();
        assert!((0.95..1.0).contains(&time), "the seek past the end showed the frame at {time}");

        player.play();
        let first_again = wait_for(&slot, last.generation, |_| true);
        let start = Instant::now();
        let time = first_again.frame.time.unwrap();
        assert!(time < 0.1, "the video didn't start over: it showed the frame at {time}");
        // And plays on time from there.
        wait_for(&slot, first_again.generation, |frame| frame.time.is_some_and(|time| time >= 0.5));
        assert!(start.elapsed() >= Duration::from_millis(400), "half a second played in {:?}", start.elapsed());
    }
}

#[test]
fn a_file_that_isnt_a_video_fails() {
    for source in [Bytes(Some(&[7; 4096])), Bytes(None)] {
        let (_player, slot) = open(source, false);
        let deadline = Instant::now() + Duration::from_secs(10);
        while slot.failure().is_none() {
            assert!(Instant::now() < deadline, "the video didn't fail");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(slot.newest().is_none());
    }
}

#[test]
fn dropping_the_player_is_prompt() {
    let (player, _slot) = open(Never, false);
    std::thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    drop(player);
    assert!(start.elapsed() < Duration::from_millis(500), "a blocked player took {:?}", start.elapsed());

    let (player, slot) = open(Bytes(Some(VIDEO)), false);
    wait_for(&slot, 0, |_| true);
    let start = Instant::now();
    drop(player);
    assert!(start.elapsed() < Duration::from_millis(500), "a playing player took {:?}", start.elapsed());
}
