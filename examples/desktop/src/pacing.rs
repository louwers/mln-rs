// When the render thread starts a frame that is due: at the display's next
// refresh, which a frame pacer (mln_frame_pacer) reports, or, where the
// platform has no pacer, once a refresh interval passed since the last
// frame. Animations sample each frame at its start, so frames that start at
// the display's refreshes show them at evenly spaced times.

use std::time::{Duration, Instant};

/// How long a frame waits for the display's refresh before it starts
/// without, for a display that stops refreshing, as one that sleeps does.
const REFRESH_TIMEOUT: Duration = Duration::from_millis(100);

/// A refresh of the display, as a frame pacer reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Refresh {
    /// When the display shows a frame started at the refresh, where the
    /// pacer knows.
    pub(crate) shown_at: Option<Instant>,
}

/// The display's refreshes, as a frame pacer reports them.
pub(crate) trait Refreshes {
    /// Asks for the display's next refresh.
    fn request(&self);

    /// The refresh since the last call, if the display refreshed.
    fn take(&self) -> Option<Refresh>;
}

/// Whether a frame that is due can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ready {
    /// It can, and the display shows it at the instant, where that's known.
    Now(Option<Instant>),
    /// Not before the display refreshes or the instant comes.
    Wait(Instant),
}

pub(crate) struct Pacing<R> {
    refreshes: Option<R>,
    /// Without refreshes, frames come at most this often.
    interval: Duration,
    next_frame_at: Instant,
    /// When the frame that waits asked for the display's refresh.
    requested: Option<Instant>,
}

impl<R: Refreshes> Pacing<R> {
    pub(crate) fn new(refreshes: Option<R>, interval: Duration, now: Instant) -> Self {
        Pacing { refreshes, interval, next_frame_at: now, requested: None }
    }

    /// Whether the frame that is due at `now` can start: at a refresh whose
    /// frame the display hasn't shown yet, or once it waited
    /// [`REFRESH_TIMEOUT`] for one. Without refreshes, whether an interval
    /// passed since the last frame.
    ///
    /// A frame that starts asks for the next refresh at once, so that a frame
    /// that runs past that refresh starts right after, for the refresh after
    /// it, rather than skipping one.
    pub(crate) fn ready(&mut self, now: Instant) -> Ready {
        let Some(refreshes) = &self.refreshes else {
            if now < self.next_frame_at {
                return Ready::Wait(self.next_frame_at);
            }
            self.next_frame_at = now + self.interval;
            return Ready::Now(None);
        };
        if let Some(refresh) = refreshes.take() {
            // A refresh whose frame the display has shown is stale, as one
            // that a frame which started without it asked for. Without that
            // time, a refresh counts if the frame waits for it.
            let fresh = refresh.shown_at.map_or(self.requested.is_some(), |shown_at| shown_at > now);
            if fresh {
                self.requested = None;
                refreshes.request();
                return Ready::Now(refresh.shown_at);
            }
        }
        let requested = *self.requested.get_or_insert_with(|| {
            refreshes.request();
            now
        });
        let late = requested + REFRESH_TIMEOUT;
        if now >= late {
            self.requested = None;
            refreshes.request();
            return Ready::Now(None);
        }
        Ready::Wait(late)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[derive(Default)]
    struct FakeRefreshes {
        refreshed: Cell<Option<Refresh>>,
        requests: Cell<u32>,
    }

    impl FakeRefreshes {
        fn refresh(&self, shown_at: Option<Instant>) {
            self.refreshed.set(Some(Refresh { shown_at }));
        }
    }

    impl Refreshes for &FakeRefreshes {
        fn request(&self) {
            self.requests.set(self.requests.get() + 1);
        }

        fn take(&self) -> Option<Refresh> {
            self.refreshed.take()
        }
    }

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_frame_starts_at_the_refresh_it_asked_for() {
        let display = FakeRefreshes::default();
        let start = Instant::now();
        let mut pacing = Pacing::new(Some(&display), 16 * MS, start);
        assert_eq!(pacing.ready(start), Ready::Wait(start + REFRESH_TIMEOUT));
        assert_eq!(display.requests.get(), 1);
        // Woken by something else, the frame keeps waiting, and asks once.
        assert_eq!(pacing.ready(start + 2 * MS), Ready::Wait(start + REFRESH_TIMEOUT));
        assert_eq!(display.requests.get(), 1);
        display.refresh(Some(start + 16 * MS));
        assert_eq!(pacing.ready(start + 8 * MS), Ready::Now(Some(start + 16 * MS)));
        // And asks for the next one at once.
        assert_eq!(display.requests.get(), 2);
    }

    // A frame that runs past the next refresh starts right after, for the
    // refresh after that one, rather than waiting a refresh more.
    #[test]
    fn a_frame_that_runs_past_the_next_refresh_starts_right_after() {
        let display = FakeRefreshes::default();
        let start = Instant::now();
        let mut pacing = Pacing::new(Some(&display), 8 * MS, start);
        assert_eq!(pacing.ready(start), Ready::Wait(start + REFRESH_TIMEOUT));
        display.refresh(Some(start + 16 * MS));
        assert_eq!(pacing.ready(start + 8 * MS), Ready::Now(Some(start + 16 * MS)));
        // The frame took until the refresh at 16 ms passed.
        display.refresh(Some(start + 24 * MS));
        assert_eq!(pacing.ready(start + 18 * MS), Ready::Now(Some(start + 24 * MS)));
    }

    #[test]
    fn a_refresh_whose_frame_was_shown_is_stale() {
        let display = FakeRefreshes::default();
        let start = Instant::now() + 50 * MS;
        let mut pacing = Pacing::new(Some(&display), 16 * MS, start);
        display.refresh(Some(start.checked_sub(MS).unwrap()));
        assert_eq!(pacing.ready(start), Ready::Wait(start + REFRESH_TIMEOUT));
        // Without its display time, one that came before the frame asked is.
        let mut pacing = Pacing::new(Some(&display), 16 * MS, start);
        display.refresh(None);
        assert_eq!(pacing.ready(start), Ready::Wait(start + REFRESH_TIMEOUT));
    }

    // A refresh that a frame asked for, but that came only after the frame
    // started without it, doesn't start a later frame at once.
    #[test]
    fn a_frame_that_started_without_its_refresh_leaves_none_for_the_next() {
        let display = FakeRefreshes::default();
        let start = Instant::now();
        let mut pacing = Pacing::new(Some(&display), 16 * MS, start);
        assert_eq!(pacing.ready(start), Ready::Wait(start + REFRESH_TIMEOUT));
        assert_eq!(pacing.ready(start + REFRESH_TIMEOUT), Ready::Now(None));
        display.refresh(Some(start + REFRESH_TIMEOUT + 8 * MS));
        let later = start + 10 * REFRESH_TIMEOUT;
        assert_eq!(pacing.ready(later), Ready::Wait(later + REFRESH_TIMEOUT));
    }

    #[test]
    fn without_refreshes_frames_come_an_interval_apart() {
        let start = Instant::now();
        let mut pacing = Pacing::<&FakeRefreshes>::new(None, 16 * MS, start);
        assert_eq!(pacing.ready(start), Ready::Now(None));
        assert_eq!(pacing.ready(start + 5 * MS), Ready::Wait(start + 16 * MS));
        assert_eq!(pacing.ready(start + 16 * MS), Ready::Now(None));
    }
}
