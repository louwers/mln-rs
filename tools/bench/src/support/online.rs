//! A map that loads over the simulated network ([`super::network`]), and
//! what a user sees of it frame by frame: how much of the view each frame
//! shows, from the renderer's covers (`mln_engine::bench::Coverage`).

use std::sync::Arc;

use mln_engine::RenderMode;
use mln_engine::bench::Coverage;
use mln_engine::camera::CameraOptions;
use mln_engine::map::{MapMode, RenderFrameStatus};
use mln_storage::Cache;

use super::cache::DiskCache;
use super::map::{Frontend, HeadlessMap};
use super::network::{Network, Server};
use crate::scenario::Bench;

/// How many frames [`OnlineMap::settle`] may draw.
const SETTLE_LIMIT: u64 = 3_600;

/// What a run's frames showed: the view's area that nothing drew and that
/// a stand-in drew, summed over the frames, and when the view was first
/// drawn and first complete.
#[derive(Default)]
pub(crate) struct Sight {
    frames: u64,
    /// Thousandths of the view, summed over the frames.
    blank: u64,
    stand_in: u64,
    first_paint: Option<u64>,
    /// The frame from which `frames_to_complete` counts.
    rest: u64,
    complete: Option<u64>,
}

impl Sight {
    /// Counts a frame whose coverage is `coverage`, and which had loaded
    /// everything it needed if `full`. A frame without a coverage, before
    /// the style and its sources loaded, shows nothing.
    fn observe(&mut self, full: bool, coverage: Option<Coverage>) {
        self.frames += 1;
        let thousandths = |share: f64| (share * 1000.0).round() as u64;
        let (shown, whole, blank, stand_in) = match coverage {
            Some(coverage) => (
                coverage.ideal + coverage.stand_in > 0.0,
                coverage.blank == 0.0 && coverage.stand_in == 0.0,
                thousandths(coverage.blank),
                thousandths(coverage.stand_in),
            ),
            None => (false, false, 1000, 0),
        };
        self.blank += blank;
        self.stand_in += stand_in;
        if shown && self.first_paint.is_none() {
            self.first_paint = Some(self.frames);
        }
        // A tile whose load failed counts as loaded, so a frame can be
        // `Full` with a parent standing in for it, however small.
        let complete = full && shown && whole;
        if complete && self.frames > self.rest && self.complete.is_none() {
            self.complete = Some(self.frames - self.rest);
        }
    }

    /// Counts `frames_to_complete` from the next frame: the camera has
    /// come to rest.
    pub(crate) fn rest(&mut self) {
        self.rest = self.frames;
        self.complete = None;
    }

    /// Whether a frame since the camera came to rest was complete.
    pub(crate) fn is_complete(&self) -> bool {
        self.complete.is_some()
    }

    /// Declares in `bench`:
    /// - `blank_area_frames` and `stand_in_area_frames`: the share of the
    ///   view that nothing drew, and that a parent or children of a tile
    ///   that hadn't loaded drew, summed over the frames, in thousandths of
    ///   the view: a wholly blank frame adds 1000;
    /// - `frames_to_first_paint`, with `first_paint`: the frames until one
    ///   drew a tile;
    /// - `frames_to_complete`, with `complete`: the frames from the
    ///   camera's rest until one showed everything the view needs: each
    ///   tile it wants drawn at its own zoom, and everything else loaded
    ///   (`RenderMode::Full`), or all the frames since, if none did.
    pub(crate) fn declare(&self, bench: &mut Bench, first_paint: bool, complete: bool) {
        bench.count("blank_area_frames", self.blank);
        bench.count("stand_in_area_frames", self.stand_in);
        if first_paint {
            bench.count("frames_to_first_paint", self.first_paint.unwrap_or(self.frames));
        }
        if complete {
            bench.count("frames_to_complete", self.complete.unwrap_or(self.frames - self.rest));
        }
    }
}

/// A continuous map over a [`Network`], and a [`DiskCache`] if it has one,
/// on a frontend of its own. Each frame runs the work between frames, draws
/// the frame and starts the transfers it asked for, all at one time of the
/// network's clock, which then moves a frame on.
pub(crate) struct OnlineMap {
    // The map before the network, whose file source it holds.
    map: HeadlessMap,
    network: Network,
    sight: Sight,
}

impl OnlineMap {
    /// A map on `frontend` at `camera`, whose style loads from the server
    /// over `network`, through `cache` if there's one.
    pub(crate) fn new(
        frontend: Frontend,
        network: Network,
        cache: Option<Arc<DiskCache>>,
        camera: &CameraOptions,
    ) -> OnlineMap {
        mln_engine::bench::measure_coverage();
        let file_source = network.file_source(frontend.pool(), cache.map(|cache| -> Arc<dyn Cache> { cache }));
        let mut map = HeadlessMap::new(frontend, MapMode::Continuous, file_source);
        map.jump(camera);
        map.map().load_style_url(&Server::style_url());
        OnlineMap { map, network, sight: Sight::default() }
    }

    pub(crate) fn map(&mut self) -> &mut HeadlessMap {
        &mut self.map
    }

    pub(crate) fn network(&self) -> &Network {
        &self.network
    }

    pub(crate) fn sight(&mut self) -> &mut Sight {
        &mut self.sight
    }

    /// Draws a frame as one measured part of `bench`, and counts what it
    /// showed. The network's clock moves on outside the part.
    pub(crate) fn frame(&mut self, bench: &mut Bench) -> RenderFrameStatus {
        let status = bench.measure(|| self.step());
        self.sight.observe(status.mode == RenderMode::Full, mln_engine::bench::take_coverage());
        self.network.advance();
        status
    }

    /// Draws frames, unmeasured and uncounted, until one shows the view
    /// complete with nothing left to animate and nothing in flight.
    pub(crate) fn settle(&mut self) {
        for _ in 0..SETTLE_LIMIT {
            let status = self.step();
            mln_engine::bench::take_coverage();
            self.network.advance();
            if status.mode == RenderMode::Full && !status.needs_repaint && self.network.is_idle() {
                return;
            }
        }
        panic!("the view isn't settled after {SETTLE_LIMIT} frames");
    }

    /// The work between frames, the frame, and the transfers it asked for.
    fn step(&mut self) -> RenderFrameStatus {
        self.work();
        let status = self.map.draw_frame();
        self.work();
        status
    }

    /// Runs the network's answers and the online file source, and the
    /// map's background work, until neither has anything to do.
    fn work(&mut self) {
        loop {
            let mut any = self.network.pump();
            any |= self.map.wait_for_workers();
            if !any {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(ideal: f64, stand_in: f64, blank: f64) -> Coverage {
        Coverage { ideal, stand_in, blank }
    }

    #[test]
    fn sight_sums_what_the_frames_left_blank_and_stood_in() {
        let mut sight = Sight::default();
        sight.observe(false, None);
        sight.observe(false, Some(coverage(0.0, 0.0, 0.9)));
        sight.observe(false, Some(coverage(0.25, 0.5, 0.25)));
        // Loaded, but a failed tile's parent stands in for it.
        sight.observe(true, Some(coverage(0.9, 0.1, 0.0)));
        // A stand-in too small to count in the sums still isn't complete.
        sight.observe(true, Some(coverage(0.9996, 0.0004, 0.0)));
        sight.observe(true, Some(coverage(1.0, 0.0, 0.0)));
        assert_eq!((sight.blank, sight.stand_in), (1000 + 900 + 250, 500 + 100));
        assert_eq!(sight.first_paint, Some(3));
        assert_eq!(sight.complete, Some(6));

        sight.rest();
        assert!(!sight.is_complete());
        sight.observe(false, Some(coverage(1.0, 0.0, 0.0)));
        sight.observe(true, Some(coverage(1.0, 0.0, 0.0)));
        assert_eq!(sight.complete, Some(2), "counted from the rest");
    }
}
