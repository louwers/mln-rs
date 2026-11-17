// When frames reached the display, for the frame report and the gesture
// test. The Metal overlay records each frame it draws over as it sends it,
// and Core Animation reports when the display showed it, on a thread of its
// own (`-[MTLDrawable addPresentedHandler:]`). Other backends record none.
//
// Times are seconds on the host's media clock (`CACurrentMediaTime`), which
// the presentation times are on.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;

/// Frames sent less than this apart belong to one run of frames, as of an
/// animation; a longer gap is the map at rest, not a frame late.
const RUN_GAP: f64 = 0.05;

/// A frame sent to the display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Presentation {
    pub(crate) sent: f64,
    /// When the display showed the frame, or `None` when it never did, as a
    /// frame that a later one replaced before the display could.
    pub(crate) shown: Option<f64>,
}

/// The frames sent to the display, oldest first.
#[derive(Clone, Default)]
pub(crate) struct Presentations(Arc<Mutex<VecDeque<Sent>>>);

/// A frame sent to the display, and the outcome that Core Animation
/// reported, once it did.
struct Sent {
    id: u64,
    sent: f64,
    reported: bool,
    shown: Option<f64>,
}

impl Presentations {
    /// Records a frame sent at `sent`, and returns where to report when the
    /// display showed it.
    #[cfg_attr(not(target_os = "macos"), expect(dead_code, reason = "only the Metal overlay records frames"))]
    pub(crate) fn send(&self, sent: f64) -> Shown {
        let mut frames = self.0.lock();
        let id = frames.back().map_or(0, |frame| frame.id + 1);
        frames.push_back(Sent { id, sent, reported: false, shown: None });
        Shown { frames: Arc::clone(&self.0), id }
    }

    /// Takes the frames whose outcome came, up to the first that waits for
    /// one.
    pub(crate) fn take(&self) -> Vec<Presentation> {
        let mut frames = self.0.lock();
        let mut taken = Vec::new();
        while let Some(&Sent { sent, reported: true, shown, .. }) = frames.front() {
            taken.push(Presentation { sent, shown });
            frames.pop_front();
        }
        taken
    }
}

/// Where a frame's presentation is reported.
pub(crate) struct Shown {
    frames: Arc<Mutex<VecDeque<Sent>>>,
    id: u64,
}

impl Shown {
    /// The display showed the frame at `at`, or never, for `None`.
    #[cfg_attr(not(target_os = "macos"), expect(dead_code, reason = "only the Metal overlay records frames"))]
    pub(crate) fn report(&self, at: Option<f64>) {
        let mut frames = self.frames.lock();
        if let Some(frame) = frames.iter_mut().rev().find(|frame| frame.id == self.id) {
            frame.reported = true;
            frame.shown = at;
        }
    }
}

/// How evenly a span of frames reached the display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cadence {
    pub(crate) frames: usize,
    /// The frames that the display never showed.
    pub(crate) dropped: usize,
    /// The median time between frames shown one after another in a run: the
    /// display's refresh interval when frames keep up with it.
    pub(crate) interval: f64,
    /// The intervals over one and a half times the median: a refresh that
    /// showed the frame before again.
    pub(crate) late: usize,
    pub(crate) longest: f64,
    /// The median time from sending a frame to the display showing it.
    pub(crate) latency: f64,
}

impl Cadence {
    /// The cadence of `frames`, or `None` without two frames shown in a run.
    pub(crate) fn of(frames: &[Presentation]) -> Option<Cadence> {
        let dropped = frames.iter().filter(|frame| frame.shown.is_none()).count();
        let shown: Vec<(f64, f64)> = frames.iter().filter_map(|frame| Some((frame.sent, frame.shown?))).collect();
        let mut intervals: Vec<f64> =
            shown.windows(2).filter(|pair| pair[1].0 - pair[0].0 < RUN_GAP).map(|pair| pair[1].1 - pair[0].1).collect();
        if intervals.is_empty() {
            return None;
        }
        let mut latencies: Vec<f64> = shown.iter().map(|(sent, shown)| shown - sent).collect();
        intervals.sort_by(f64::total_cmp);
        latencies.sort_by(f64::total_cmp);
        let interval = intervals[intervals.len() / 2];
        Some(Cadence {
            frames: frames.len(),
            dropped,
            interval,
            late: intervals.iter().filter(|&&each| each > 1.5 * interval).count(),
            longest: intervals[intervals.len() - 1],
            latency: latencies[latencies.len() / 2],
        })
    }
}

impl std::fmt::Display for Cadence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} frames shown every {:.2} ms, {} late (longest {:.2} ms), {} dropped, {:.2} ms from sent to shown",
            self.frames - self.dropped,
            self.interval * 1000.0,
            self.late,
            self.longest * 1000.0,
            self.dropped,
            self.latency * 1000.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFRESH: f64 = 1.0 / 120.0;

    fn frame(sent: f64, shown: Option<f64>) -> Presentation {
        Presentation { sent, shown }
    }

    #[test]
    fn frames_in_lockstep_have_no_late_ones() {
        let frames: Vec<_> =
            (0..10).map(|i| frame(f64::from(i) * REFRESH, Some(f64::from(i) * REFRESH + 0.02))).collect();
        let cadence = Cadence::of(&frames).unwrap();
        assert!((cadence.interval - REFRESH).abs() < 1e-9);
        assert_eq!((cadence.late, cadence.dropped), (0, 0));
        assert!((cadence.latency - 0.02).abs() < 1e-9);
    }

    #[test]
    fn a_frame_shown_a_refresh_late_counts_once() {
        let mut frames: Vec<_> = (0..10).map(|i| frame(f64::from(i) * REFRESH, Some(f64::from(i) * REFRESH))).collect();
        // The fifth frame missed its refresh: the display showed the fourth
        // again, and each frame after a refresh later.
        for frame in &mut frames[4..] {
            frame.shown = frame.shown.map(|shown| shown + REFRESH);
        }
        let cadence = Cadence::of(&frames).unwrap();
        assert_eq!((cadence.late, cadence.dropped), (1, 0));
        assert!((cadence.longest - 2.0 * REFRESH).abs() < 1e-9);
    }

    #[test]
    fn a_frame_never_shown_counts_as_dropped() {
        let mut frames: Vec<_> = (0..10).map(|i| frame(f64::from(i) * REFRESH, Some(f64::from(i) * REFRESH))).collect();
        frames[5].shown = None;
        let cadence = Cadence::of(&frames).unwrap();
        assert_eq!((cadence.frames, cadence.dropped, cadence.late), (10, 1, 1));
    }

    #[test]
    fn a_pause_between_runs_isnt_late() {
        let frames = [frame(0.0, Some(0.01)), frame(REFRESH, Some(0.01 + REFRESH)), frame(1.0, Some(1.01))];
        assert_eq!(Cadence::of(&frames).unwrap().late, 0);
    }

    #[test]
    fn frames_are_taken_once_their_outcome_comes() {
        let presentations = Presentations::default();
        let first = presentations.send(1.0);
        let second = presentations.send(2.0);
        second.report(Some(2.5));
        assert_eq!(presentations.take(), []);
        first.report(None);
        assert_eq!(presentations.take(), [frame(1.0, None), frame(2.0, Some(2.5))]);
    }
}
