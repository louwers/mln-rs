// The AVFoundation player (`platform_players` on Apple platforms), playing a
// fixture through a FileSource. AVFoundation steps players on the main
// thread, so this test brings its own harness: each test runs on a thread
// while the main thread runs its run loop.

fn main() {
    #[cfg(target_vendor = "apple")]
    apple::main();
}

#[cfg(target_vendor = "apple")]
mod apple {
    #![expect(unsafe_code, reason = "CoreFoundation's run loop and CoreVideo's buffers")]

    use std::ffi::c_void;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use mln_storage::{FileSource, Resource, Responder, Response};
    use mln_util::size::Size;
    use mln_video::{
        CpuPlane, FramePixels, FrameSink, FrameSlot, GpuFrame, Newest, OpenRequest, VideoFrame, VideoPlayer, YuvColor,
        YuvRange, platform_players,
    };
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
        CVPixelBufferGetHeightOfPlane, CVPixelBufferGetIOSurface, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVReturnSuccess,
    };

    /// A two-second, 10 fps, 128x96 H.264 video whose quarters are four
    /// colors for its first second, and the same colors in other places for
    /// its second.
    const QUARTERS: &[u8] = include_bytes!("fixtures/quarters-apple.mp4");
    const SIZE: Size = Size { width: 128, height: 96 };
    const FIRST: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [64, 128, 192]];
    const SECOND: [[u8; 3]; 4] = [FIRST[2], FIRST[0], FIRST[3], FIRST[1]];

    const TESTS: &[(&str, fn())] = &[
        (
            "frames_in_memory_arrive_with_the_videos_size_and_colors",
            frames_in_memory_arrive_with_the_videos_size_and_colors,
        ),
        ("frames_on_the_gpu_are_iosurface_backed_pixel_buffers", frames_on_the_gpu_are_iosurface_backed_pixel_buffers),
        ("a_paused_video_sends_the_frame_it_seeks_to", a_paused_video_sends_the_frame_it_seeks_to),
        ("a_refused_gpu_frame_comes_again_in_memory", a_refused_gpu_frame_comes_again_in_memory),
        ("the_video_loops", the_video_loops),
        ("a_file_that_isnt_a_video_fails", a_file_that_isnt_a_video_fails),
        ("the_first_url_that_plays_plays", the_first_url_that_plays_plays),
        ("dropping_a_player_stops_it_promptly", dropping_a_player_stops_it_promptly),
    ];

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
    }

    /// The test that needs the main thread to run no run loop.
    const WITHOUT_RUN_LOOP: &str = "a_main_thread_without_a_run_loop_fails_the_video";

    /// Lists the tests for `--list`, as libtest and cargo-nextest's custom
    /// harnesses do. Otherwise runs each test whose name has the first
    /// argument in it, or is it with `--exact`, on a thread while the main
    /// thread runs its run loop; then the one test that needs the main thread
    /// to run none.
    pub(crate) fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let has = |flag: &str| args.iter().any(|arg| arg == flag);
        if has("--list") {
            // None of the tests is ignored.
            if !has("--ignored") {
                for name in TESTS.iter().map(|&(name, _)| name).chain([WITHOUT_RUN_LOOP]) {
                    println!("{name}: test");
                }
            }
            return;
        }
        let exact = has("--exact");
        let filter = args.iter().find(|arg| !arg.starts_with('-')).cloned().unwrap_or_default();
        let mut failed = Vec::new();
        let mut ran = 0;
        let mut run = |name: &str, test: fn(), spin: bool| {
            if if exact { name != filter } else { !name.contains(&filter) } {
                return;
            }
            ran += 1;
            let thread = std::thread::Builder::new().name(name.to_owned()).spawn(test).expect("a test thread");
            while spin && !thread.is_finished() {
                // SAFETY: CoreFoundation's default mode, on the main thread.
                unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 0) };
            }
            let passed = thread.join().is_ok();
            println!("test {name} ... {}", if passed { "ok" } else { "FAILED" });
            if !passed {
                failed.push(name.to_owned());
            }
        };
        for &(name, test) in TESTS {
            run(name, test, true);
        }
        run(WITHOUT_RUN_LOOP, a_main_thread_without_a_run_loop_fails_the_video, false);
        println!("\n{} passed, {} failed", ran - failed.len(), failed.len());
        if !failed.is_empty() {
            println!("failures: {failed:?}");
            std::process::exit(1);
        }
    }

    /// Serves each URL's bytes as a server would: ranges cut at the end, and
    /// the size known. A URL it doesn't have never answers.
    struct Files(Vec<(&'static str, Vec<u8>)>);

    impl FileSource for Files {
        fn load(&self, resource: &Resource, responder: Responder) {
            let Some((_, bytes)) = self.0.iter().find(|(url, _)| *url == resource.url) else {
                std::mem::forget(responder);
                return;
            };
            let (first, last) = resource.data_range.unwrap_or((0, u64::MAX - 1));
            let data = bytes.iter().copied().skip(first as usize).take((last - first + 1) as usize).collect();
            let response =
                Response { data: Some(Arc::new(data)), total_size: Some(bytes.len() as u64), ..Response::default() };
            std::thread::spawn(move || responder.send(response));
        }

        fn can_request(&self, _resource: &Resource) -> bool {
            true
        }
    }

    /// Opens `urls` on the platform's player, with the frames going to a
    /// new slot, which refuses GPU frames first if `cpu`.
    fn open(urls: &[&str], cpu: bool) -> (Box<dyn VideoPlayer>, Arc<FrameSlot>) {
        let slot = FrameSlot::new();
        if cpu {
            slot.refuse_gpu_frames();
        }
        let files = Files(vec![
            ("https://example.com/quarters.mp4", QUARTERS.to_vec()),
            ("https://example.com/bogus.mp4", b"not a video at all, ".repeat(100)),
        ]);
        let player = platform_players()
            .open(OpenRequest {
                urls: urls.iter().map(|&url| url.to_owned()).collect(),
                file_source: Some(Arc::new(files)),
                frames: FrameSink::new(Arc::clone(&slot)),
                url_transform: None,
            })
            .expect("a player");
        (player, slot)
    }

    /// Waits up to ten seconds for `until` to return something.
    fn wait<T>(what: &str, mut until: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(value) = until() {
                return value;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits for a frame newer than generation `seen` that `accept` takes.
    fn frame_after(slot: &FrameSlot, seen: u64, accept: impl Fn(&VideoFrame) -> bool) -> Newest {
        wait("a frame", || slot.newer_than(seen).filter(|newest| newest.generation > seen && accept(&newest.frame)))
    }

    fn generation(slot: &FrameSlot) -> u64 {
        slot.newest().map_or(0, |newest| newest.generation)
    }

    /// The RGB color at `(x, y)` of NV12 planes.
    fn nv12_rgb(luma: &CpuPlane, uv: &CpuPlane, color: YuvColor, x: usize, y: usize) -> [u8; 3] {
        let sample = [
            f32::from(luma.data[y * luma.stride + x]) / 255.0,
            f32::from(uv.data[y / 2 * uv.stride + x / 2 * 2]) / 255.0,
            f32::from(uv.data[y / 2 * uv.stride + x / 2 * 2 + 1]) / 255.0,
            1.0,
        ];
        color.to_rgb_rows().map(|row| {
            let value: f32 = row.iter().zip(sample).map(|(weight, sample)| weight * sample).sum();
            (value * 255.0).round().clamp(0.0, 255.0) as u8
        })
    }

    /// Checks the center of each quarter of a `SIZE` frame, whose colors
    /// `rgb_at` reads, against `colors`, within what H.264 changes.
    fn assert_quarters(time: Option<f64>, colors: [[u8; 3]; 4], rgb_at: impl Fn(usize, usize) -> [u8; 3]) {
        let (width, height) = (SIZE.width as usize, SIZE.height as usize);
        for (index, (x, y)) in [(1, 1), (3, 1), (1, 3), (3, 3)].into_iter().enumerate() {
            let actual = rgb_at(width * x / 4, height * y / 4);
            let close = actual.iter().zip(colors[index]).all(|(&a, e)| a.abs_diff(e) <= 24);
            assert!(close, "quarter {index} at {time:?}s: {actual:?}, expected {:?}", colors[index]);
        }
    }

    /// Checks a frame in memory's size and quarters.
    fn assert_frame(frame: &VideoFrame, colors: [[u8; 3]; 4]) {
        assert_eq!(SIZE, frame.size);
        let FramePixels::Nv12 { y, uv, color } = &frame.pixels else {
            panic!("an NV12 frame in memory, not {:?}", frame.pixels);
        };
        assert_quarters(frame.time, colors, |px, py| nv12_rgb(y, uv, *color, px, py));
    }

    /// The colors of the frame at `time`.
    fn colors_at(time: f64) -> [[u8; 3]; 4] {
        if time < 1.0 { FIRST } else { SECOND }
    }

    fn frames_in_memory_arrive_with_the_videos_size_and_colors() {
        let (_player, slot) = open(&["https://example.com/quarters.mp4"], true);
        let newest = frame_after(&slot, 0, |_| true);
        assert_frame(&newest.frame, colors_at(newest.frame.time.expect("a frame time")));
    }

    fn frames_on_the_gpu_are_iosurface_backed_pixel_buffers() {
        let (_player, slot) = open(&["https://example.com/quarters.mp4"], false);
        let newest = frame_after(&slot, 0, |_| true);
        assert_eq!(SIZE, newest.frame.size);
        let FramePixels::Gpu(GpuFrame::CvPixelBuffer { buffer, color }) = &newest.frame.pixels else {
            panic!("a CVPixelBuffer frame, not {:?}", newest.frame.pixels);
        };
        // SAFETY: the frame holds the buffer.
        let buffer = unsafe { &*buffer.cast::<CVPixelBuffer>() };
        assert!(CVPixelBufferGetIOSurface(Some(buffer)).is_some(), "an IOSurface-backed buffer");
        assert_eq!(YuvRange::Limited, color.range);
        // SAFETY: a valid buffer, unlocked below.
        let locked = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        assert_eq!(kCVReturnSuccess, locked);
        let plane = |index| {
            let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, index);
            let rows = CVPixelBufferGetHeightOfPlane(buffer, index);
            let base = CVPixelBufferGetBaseAddressOfPlane(buffer, index).cast::<u8>();
            // SAFETY: the locked plane has `rows` rows of `stride` bytes.
            CpuPlane { data: unsafe { std::slice::from_raw_parts(base, stride * rows) }.to_vec(), stride }
        };
        let (y, uv) = (plane(0), plane(1));
        // SAFETY: locked above.
        unsafe { CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        let time = newest.frame.time.unwrap_or(0.0);
        assert_quarters(newest.frame.time, colors_at(time), |px, py| nv12_rgb(&y, &uv, *color, px, py));
    }

    fn a_paused_video_sends_the_frame_it_seeks_to() {
        let (mut player, slot) = open(&["https://example.com/quarters.mp4"], true);
        frame_after(&slot, 0, |_| true);
        player.pause();
        for target in [1.5, 0.25] {
            let seen = generation(&slot);
            player.seek(target);
            let newest = frame_after(&slot, seen, |frame| frame.time.is_some_and(|time| (time - target).abs() < 0.15));
            assert_frame(&newest.frame, colors_at(target));
        }
        // Paused, it sends no more frames.
        let seen = generation(&slot);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(seen, generation(&slot), "frames while paused");
    }

    /// A paused video whose renderer refuses its GPU frame sends that frame
    /// again in memory, as no other comes.
    fn a_refused_gpu_frame_comes_again_in_memory() {
        let (mut player, slot) = open(&["https://example.com/quarters.mp4"], false);
        frame_after(&slot, 0, |_| true);
        player.pause();
        player.seek(1.5);
        let paused = frame_after(&slot, 0, |frame| frame.time.is_some_and(|time| (time - 1.5).abs() < 0.15));
        assert!(matches!(paused.frame.pixels, FramePixels::Gpu(_)), "{:?}", paused.frame.pixels);
        slot.refuse_gpu_frames();
        let again = frame_after(&slot, paused.generation, |_| true);
        assert_eq!(paused.frame.time, again.frame.time);
        assert_frame(&again.frame, SECOND);
    }

    fn the_video_loops() {
        let (_player, slot) = open(&["https://example.com/quarters.mp4"], true);
        let first = frame_after(&slot, 0, |_| true);
        let late = frame_after(&slot, first.generation, |frame| frame.time.is_some_and(|time| time > 1.5));
        let late_time = late.frame.time.unwrap_or(0.0);
        frame_after(&slot, late.generation, |frame| frame.time.is_some_and(|time| time < late_time - 1.0));
    }

    fn a_file_that_isnt_a_video_fails() {
        let (_player, slot) = open(&["https://example.com/bogus.mp4"], true);
        let failure = wait("a failure", || slot.failure());
        assert!(failure.contains("bogus.mp4"), "{failure}");
        assert!(slot.newest().is_none());
    }

    fn the_first_url_that_plays_plays() {
        let (_player, slot) = open(&["https://example.com/bogus.mp4", "https://example.com/quarters.mp4"], true);
        let newest = frame_after(&slot, 0, |_| true);
        assert_eq!(SIZE, newest.frame.size);
        assert_eq!(None, slot.failure());
    }

    fn dropping_a_player_stops_it_promptly() {
        // One that plays, and one whose bytes never come.
        for url in ["https://example.com/quarters.mp4", "https://example.com/never.mp4"] {
            let (player, slot) = open(&[url], true);
            if url.contains("quarters") {
                frame_after(&slot, 0, |_| true);
            } else {
                std::thread::sleep(Duration::from_millis(200));
            }
            let start = Instant::now();
            drop(player);
            let elapsed = start.elapsed();
            assert!(elapsed < Duration::from_millis(500), "{url}: dropping took {elapsed:?}");
            let seen = generation(&slot);
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(seen, generation(&slot), "{url}: frames after the drop");
        }
    }

    /// Run without the main thread's run loop, as a command-line render
    /// would: the video fails rather than keep a still render waiting.
    fn a_main_thread_without_a_run_loop_fails_the_video() {
        let (_player, slot) = open(&["https://example.com/quarters.mp4"], true);
        let deadline = Instant::now() + Duration::from_secs(10);
        while slot.failure().is_none() {
            assert!(Instant::now() < deadline, "no failure");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(slot.failure().is_some_and(|failure| failure.contains("run loop")), "{:?}", slot.failure());
    }
}
