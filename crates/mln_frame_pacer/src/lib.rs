//! Frame pacing: a [`FramePacer`] calls back at a display's refreshes, at the
//! rate a [`Limit`] allows, so that a host draws each frame when the display
//! is ready for it rather than on a timer of its own.
//!
//! A [`VsyncSource`] reports each refresh. [`FramePacer::new`] follows the
//! platform's display: a `CADisplayLink` on macOS, iOS and tvOS. A host on
//! another platform's display, such as the C API's `AChoreographer` on
//! Android, gives [`FramePacer::with_source`] a source of its own.

use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use mln_util::Error;
use mln_util::time::Instant;
use parking_lot::Mutex;

#[cfg_attr(
    any(target_os = "macos", target_os = "ios", target_os = "tvos"),
    expect(unsafe_code, reason = "the display link through objc2 is the FFI here")
)]
mod apple;

// MARK: - Limit

/// A frame rate limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    frames_per_second: u32,
    period: Duration,
}

impl Limit {
    /// A limit of `frames_per_second`, which must be greater than zero.
    #[must_use]
    pub fn new(frames_per_second: u32) -> Option<Self> {
        let period = Duration::from_secs(1).checked_div(frames_per_second)?;
        Some(Self { frames_per_second, period })
    }

    /// The most frames per second.
    #[must_use]
    pub fn frames_per_second(self) -> u32 {
        self.frames_per_second
    }

    /// How early a refresh can come and still count as on time: a display
    /// that runs at the limit's rate reports refreshes with some jitter, and
    /// a pacer that skipped each one that came a little early would call
    /// back at half the rate.
    fn slack(self) -> Duration {
        self.period / 10
    }
}

/// Which refreshes a limit lets the pacer call back at.
///
/// It calls back on a grid of the limit's period, so that over any span it
/// calls back no more than the limit's rate, give or take one frame. A
/// refresh within the slack before the grid's next point takes that point,
/// and one that comes a whole period after it, as after an idle spell,
/// starts the grid again. A new limit's grid starts a period of it after the
/// last callback.
#[derive(Debug, Default)]
struct Throttle {
    limit: Option<Limit>,
    /// The grid's next point.
    next: Option<Duration>,
    /// The refresh that the pacer last called back at, with or without a
    /// limit.
    last: Option<Duration>,
}

impl Throttle {
    fn new(limit: Option<Limit>) -> Self {
        Self { limit, ..Self::default() }
    }

    /// Whether the pacer may call back at a refresh at `time`, which then
    /// counts as a callback.
    fn admit(&mut self, time: Duration) -> bool {
        let Some(limit) = self.limit else {
            self.last = Some(time);
            return true;
        };
        // A clock that moves backward starts the grid again.
        if self.last.is_some_and(|last| time < last) {
            self.next = None;
        }
        if self.next.is_some_and(|next| time + limit.slack() < next) {
            return false;
        }
        self.next = Some(match self.next {
            Some(next) if time < next + limit.period => next + limit.period,
            _ => time + limit.period,
        });
        self.last = Some(time);
        true
    }

    /// Takes `limit` from the next refresh on, counted from the last
    /// callback: a refresh within the new limit's period after it waits for
    /// the next.
    fn set_limit(&mut self, limit: Option<Limit>) {
        self.limit = limit;
        self.next = limit.zip(self.last).map(|(limit, last)| last + limit.period);
    }
}

// MARK: - Sources

/// A platform's source of display refreshes.
pub trait VsyncSource: Send + Sync {
    /// Arranges one call to [`Vsyncs::vsync`] at the display's next refresh.
    /// It runs on any thread, holding none of the pacer's locks, and doesn't
    /// block.
    fn schedule(&self);

    /// Follows a new frame rate limit, which the pacer's throttle applies to
    /// the refreshes either way. It runs on any thread, holding the pacer's
    /// limit lock and no other lock of the pacer's, and doesn't block. The
    /// default ignores it, for a source whose refreshes don't depend on it.
    fn set_limit(&self, _limit: Option<Limit>) {}
}

/// Where a [`VsyncSource`] reports refreshes: to its pacer, while it lives.
/// A source that the pacer drops can still report the refreshes it
/// scheduled, which then go nowhere.
#[derive(Clone, Default)]
pub struct Vsyncs(Arc<OnceLock<Weak<dyn Refreshes>>>);

impl Vsyncs {
    /// Reports a refresh at `time` on a monotonic clock. The pacer runs its
    /// callback on this thread before it returns, when the refresh is one
    /// that it calls back at.
    pub fn vsync(&self, time: Duration) {
        self.report(time, None);
    }

    /// Reports a refresh at `time` on a monotonic clock, as
    /// [`vsync`](Self::vsync) does, for which the display shows a frame at
    /// `shown_at`.
    pub fn vsync_shown_at(&self, time: Duration, shown_at: Instant) {
        self.report(time, Some(shown_at));
    }

    fn report(&self, time: Duration, shown_at: Option<Instant>) {
        if let Some(pacer) = self.0.get().and_then(Weak::upgrade) {
            pacer.on_vsync(time, shown_at);
        }
    }
}

/// A pacer, as its [`Vsyncs`] reach it.
trait Refreshes: Send + Sync {
    fn on_vsync(&self, time: Duration, shown_at: Option<Instant>);
}

// MARK: - Pacer

/// Calls back at the first refresh that its limit allows after a
/// [`request`](Self::request), with the waiters that asked since the last
/// callback, each once. The callback gets when the display shows a frame
/// drawn for the refresh, where the source knows, and runs on the source's
/// thread, holding none of the pacer's locks. Dropping the pacer drops its
/// source, which stops.
pub struct FramePacer<T = ()> {
    state: Mutex<PacerState<T>>,
    /// Held while the limit changes, so that the source ends with the
    /// throttle's limit.
    limit_lock: Mutex<()>,
    source: Box<dyn VsyncSource>,
    on_refresh: Box<dyn Fn(Option<Instant>, Vec<T>) + Send + Sync>,
}

struct PacerState<T> {
    /// The waiters that asked for a refresh since the last callback.
    waiting: Vec<T>,
    /// Whether the source has a refresh on its way.
    scheduled: bool,
    throttle: Throttle,
}

impl<T: PartialEq + Send + 'static> FramePacer<T> {
    /// A pacer on the platform's display: a `CADisplayLink` of the main
    /// display on macOS 14 and later, iOS and tvOS. Its thread runs the
    /// callback.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) elsewhere,
    /// including on older macOS, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) without a display.
    pub fn new(
        limit: Option<Limit>,
        on_refresh: impl Fn(Option<Instant>, Vec<T>) + Send + Sync + 'static,
    ) -> Result<Arc<Self>, Error> {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
        {
            Self::with_source(limit, on_refresh, apple::display_link::DisplayLinkSource::new)
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
        {
            let _ = (limit, on_refresh);
            Err(Error::unsupported("the platform's display has no frame pacer"))
        }
    }

    /// A pacer on the source that `source` makes from where it reports
    /// refreshes and the limit.
    pub fn with_source<S: VsyncSource + 'static, E>(
        limit: Option<Limit>,
        on_refresh: impl Fn(Option<Instant>, Vec<T>) + Send + Sync + 'static,
        source: impl FnOnce(Vsyncs, Option<Limit>) -> Result<S, E>,
    ) -> Result<Arc<Self>, E> {
        let vsyncs = Vsyncs::default();
        let source = source(vsyncs.clone(), limit)?;
        let pacer = Arc::new(Self {
            state: Mutex::new(PacerState { waiting: Vec::new(), scheduled: false, throttle: Throttle::new(limit) }),
            limit_lock: Mutex::new(()),
            source: Box::new(source),
            on_refresh: Box::new(on_refresh),
        });
        let weak = Arc::downgrade(&pacer);
        let refreshes: Weak<dyn Refreshes> = weak;
        // A new `Vsyncs` is unset.
        let _ = vsyncs.0.set(refreshes);
        Ok(pacer)
    }

    /// Asks for a callback for `waiter` at the next refresh that the limit
    /// allows.
    pub fn request(&self, waiter: T) {
        {
            let mut state = self.state.lock();
            if !state.waiting.contains(&waiter) {
                state.waiting.push(waiter);
            }
            if std::mem::replace(&mut state.scheduled, true) {
                return;
            }
        }
        self.source.schedule();
    }

    /// The frame rate limit.
    #[must_use]
    pub fn limit(&self) -> Option<Limit> {
        self.state.lock().throttle.limit
    }

    /// Takes `limit` from the next refresh on, including one that is on its
    /// way already.
    pub fn set_limit(&self, limit: Option<Limit>) {
        let _changing = self.limit_lock.lock();
        self.state.lock().throttle.set_limit(limit);
        self.source.set_limit(limit);
    }
}

impl<T: Clone> FramePacer<T> {
    /// The waiters that the next callback is for.
    #[must_use]
    pub fn waiting(&self) -> Vec<T> {
        self.state.lock().waiting.clone()
    }
}

impl<T: Send + 'static> Refreshes for FramePacer<T> {
    fn on_vsync(&self, time: Duration, shown_at: Option<Instant>) {
        let due = {
            let mut state = self.state.lock();
            if state.waiting.is_empty() {
                state.scheduled = false;
                return;
            }
            if state.throttle.admit(time) {
                state.scheduled = false;
                Some(std::mem::take(&mut state.waiting))
            } else {
                None
            }
        };
        match due {
            Some(waiters) => (self.on_refresh)(shown_at, waiters),
            // Waits for the next refresh.
            None => self.source.schedule(),
        }
    }
}

#[cfg(test)]
mod tests;
