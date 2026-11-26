// The benchmark mode (`--benchmark`): the camera path of a Frame scenario of
// the benchmarks (tools/bench, mln_bench_workload), drawn in the window over
// the benchmarks' fixture snapshot, lap after lap, then a summary of the
// frames' times on standard output.
//
// The headless scenario measures a frame's work; this measures what a user
// sees of it: the frames a window presents, with the backend's surface and
// the display's compositor in the way. The map loads the path's start, then
// each frame jumps the camera to the path's next point. Frames come as fast
// as the surface takes them, with nothing on the network, no cache database
// and no system fonts, so that two runs on one machine draw the same frames.

use std::time::{Duration, Instant};

use mln_bench_workload::Snapshot;
use mln_bench_workload::paths::{FRAMES, Path};
use mln_engine::camera::CameraOptions;

/// What the benchmark mode draws, and the frames it has timed.
pub(crate) struct Benchmark {
    path: Path,
    snapshot: Snapshot,
    camera: Box<dyn Fn(usize) -> CameraOptions>,
    /// The frames to time: every lap's, or `None` to draw until the window
    /// closes.
    frames: Option<usize>,
    /// Whether the map has drawn the path's start loaded and settled, and
    /// frames are timed.
    measuring: bool,
    /// The path's frames demanded since the timing began.
    demanded: usize,
    /// The token of the first demand of a timed frame.
    first_token: Option<u64>,
    /// When the last timed frame came.
    last: Option<Instant>,
    /// The time between each timed frame and the one before it.
    intervals: Vec<Duration>,
    /// Timed frames that showed something not yet loaded.
    incomplete: usize,
}

impl Benchmark {
    /// `laps` of `path` over the installed snapshot, or laps until the
    /// window closes for 0. Fails when the snapshot isn't installed.
    pub(crate) fn new(path: Path, laps: usize) -> Result<Benchmark, String> {
        let snapshot = Snapshot::find()?;
        let camera = Box::new(path.camera(&snapshot));
        Ok(Benchmark {
            path,
            snapshot,
            camera,
            frames: (laps > 0).then(|| laps * FRAMES),
            measuring: false,
            demanded: 0,
            first_token: None,
            last: None,
            intervals: Vec::new(),
            incomplete: 0,
        })
    }

    /// The asset path that resolves the style's `asset://` URLs.
    pub(crate) fn asset_path(&self) -> std::path::PathBuf {
        self.snapshot.dir().to_owned()
    }

    /// The style the path is drawn over, as JSON.
    pub(crate) fn style(&self) -> String {
        self.path.style(&self.snapshot)
    }

    /// The camera at the path's start.
    pub(crate) fn start(&self) -> CameraOptions {
        (self.camera)(0)
    }

    pub(crate) fn name(&self) -> &'static str {
        self.path.name()
    }

    /// The camera of the next frame to demand, with the demand's `token`,
    /// once the timing has begun.
    pub(crate) fn next_camera(&mut self, token: u64) -> Option<CameraOptions> {
        if !self.measuring {
            return None;
        }
        self.first_token.get_or_insert(token);
        let lap = self.demanded / FRAMES;
        let mut frame = self.demanded % FRAMES;
        // The street path ends where it starts; the globe's turns back.
        if self.path == Path::Globe && lap % 2 == 1 {
            frame = FRAMES - frame;
        }
        self.demanded += 1;
        Some((self.camera)(frame))
    }

    /// Takes a rendered frame of the demand `token`, which came `at`, showed
    /// everything loaded if `complete`, and left transitions running, such as
    /// symbols fading in, if `animating`. The timing begins after the first
    /// frame of the path's start that is complete with nothing to animate.
    /// Returns whether the benchmark has timed all its frames.
    pub(crate) fn rendered(&mut self, token: Option<u64>, at: Instant, complete: bool, animating: bool) -> bool {
        let timed = matches!((token, self.first_token), (Some(token), Some(first)) if token >= first);
        if !timed {
            self.measuring |= complete && !animating;
            return false;
        }
        if let Some(last) = self.last.replace(at) {
            self.intervals.push(at - last);
            if !complete {
                self.incomplete += 1;
            }
        }
        self.frames.is_some_and(|frames| self.intervals.len() >= frames)
    }

    /// What the benchmark timed, with `window`, a description of the window
    /// and its backend.
    pub(crate) fn summary(&self, window: &str) -> String {
        let laps = self.intervals.len() as f64 / FRAMES as f64;
        let mut lines = vec![
            format!(
                "benchmark: {} path, {} frames timed ({laps:.1} laps of {FRAMES}), fixture snapshot {}",
                self.path.name(),
                self.intervals.len(),
                self.snapshot.manifest().snapshot,
            ),
            format!("  on {} {}, {window}", std::env::consts::OS, std::env::consts::ARCH),
        ];
        if self.intervals.is_empty() {
            lines.push("  no frames timed: the map never finished loading the path's start".to_owned());
            return lines.join("\n");
        }
        let mut sorted = self.intervals.clone();
        sorted.sort_unstable();
        let total: Duration = sorted.iter().sum();
        let mean = total / sorted.len() as u32;
        let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
        let percentile = |p: f64| sorted[((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1];
        lines.push(format!(
            "  time between frames: mean {:.2} ms ({:.0} fps), p50 {:.2} ms, p90 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
            ms(mean),
            sorted.len() as f64 / total.as_secs_f64(),
            ms(percentile(0.5)),
            ms(percentile(0.9)),
            ms(percentile(0.99)),
            ms(sorted[sorted.len() - 1]),
        ));
        lines.push(format!("  frames that showed something not yet loaded: {}", self.incomplete));
        lines.join("\n")
    }
}
