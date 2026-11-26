// When the render thread starts a frame that is due: at the display's next
// refresh, which a frame pacer (mln_runtime::frame_pacer) reports, or, where the
// platform has no pacer, once a refresh interval passed since the last
// frame. Animations sample each frame at its start, so frames that start at
// the display's refreshes show them at evenly spaced times.

use std::time::{Duration, Instant};

/// How long a frame waits for the display's refresh before it starts
/// without, for a display that stops refreshing, as one that sleeps does.
const REFRESH_TIMEOUT: Duration = Duration::from_millis(100);

/// The display's refreshes, as a frame pacer reports them.
pub(crate) trait Refreshes {
    /// Asks for the display's next refresh.
    fn request(&self);

    /// Whether the display refreshed since the last call.
    fn take(&self) -> bool;
}

/// Whether a frame that is due can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ready {
    Now,
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

    /// Whether the frame that is due at `now` can start: the display
    /// refreshed since the frame asked it to, or it waited
    /// [`REFRESH_TIMEOUT`] for the refresh. Without refreshes, whether an
    /// interval passed since the last frame.
    pub(crate) fn ready(&mut self, now: Instant) -> Ready {
        let Some(refreshes) = &self.refreshes else {
            if now < self.next_frame_at {
                return Ready::Wait(self.next_frame_at);
            }
            self.next_frame_at = now + self.interval;
            return Ready::Now;
        };
        let requested = *self.requested.get_or_insert_with(|| {
            // A refresh that came before the frame asked, for an earlier
            // frame that started without it, is stale.
            refreshes.take();
            refreshes.request();
            now
        });
        let late = requested + REFRESH_TIMEOUT;
        if refreshes.take() || now >= late {
            self.requested = None;
            return Ready::Now;
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
        refreshed: Cell<bool>,
        requests: Cell<u32>,
    }

    impl Refreshes for &FakeRefreshes {
        fn request(&self) {
            self.requests.set(self.requests.get() + 1);
        }

        fn take(&self) -> bool {
            self.refreshed.replace(false)
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
        display.refreshed.set(true);
        assert_eq!(pacing.ready(start + 8 * MS), Ready::Now);
    }

    #[test]
    fn a_refresh_from_before_the_frame_asked_is_stale() {
        let display = FakeRefreshes::default();
        let start = Instant::now();
        let mut pacing = Pacing::new(Some(&display), 16 * MS, start);
        display.refreshed.set(true);
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
        assert_eq!(pacing.ready(start + REFRESH_TIMEOUT), Ready::Now);
        display.refreshed.set(true);
        let later = start + 10 * REFRESH_TIMEOUT;
        assert_eq!(pacing.ready(later), Ready::Wait(later + REFRESH_TIMEOUT));
        assert_eq!(display.requests.get(), 2);
    }

    #[test]
    fn without_refreshes_frames_come_an_interval_apart() {
        let start = Instant::now();
        let mut pacing = Pacing::<&FakeRefreshes>::new(None, 16 * MS, start);
        assert_eq!(pacing.ready(start), Ready::Now);
        assert_eq!(pacing.ready(start + 5 * MS), Ready::Wait(start + 16 * MS));
        assert_eq!(pacing.ready(start + 16 * MS), Ready::Now);
    }
}
