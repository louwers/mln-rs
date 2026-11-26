//! Frame pacing: a [`FramePacer`] calls back at a display's refreshes, at the
//! rate a [`Limit`] allows, so that a host draws each frame when the display
//! is ready for it rather than on a timer of its own.
//!
//! A [`VsyncSource`] reports each refresh. [`FramePacer::new`] follows the
//! platform's display: a `CADisplayLink` on macOS, iOS and tvOS, the calling
//! thread's `AChoreographer` on Android, and an `OH_NativeVSync` on
//! `OpenHarmony`. On macOS, iOS and tvOS, `FramePacer::for_view` follows
//! the display that a view's window is on. A host with a source of its own
//! gives it to [`FramePacer::with_source`].

use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use mln_style::error::Error;
use parking_lot::Mutex;

#[cfg_attr(target_os = "android", expect(unsafe_code, reason = "the choreographer and its looper are the FFI here"))]
mod android;
#[cfg_attr(
    any(target_os = "macos", target_os = "ios", target_os = "tvos"),
    expect(unsafe_code, reason = "the display link through objc2 is the FFI here")
)]
mod apple;
#[cfg_attr(target_env = "ohos", expect(unsafe_code, reason = "the native VSync is the FFI here"))]
mod ohos;

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
pub struct Vsyncs(Arc<OnceLock<Weak<FramePacer>>>);

impl Vsyncs {
    /// Reports a refresh at `time` on a monotonic clock. The pacer calls its
    /// waiters on this thread before it returns, when the refresh is one
    /// that it calls back at.
    pub fn vsync(&self, time: Duration) {
        if let Some(pacer) = self.0.get().and_then(Weak::upgrade) {
            pacer.on_vsync(time);
        }
    }
}

// MARK: - Pacer

/// What waits for a pacer's refresh.
pub trait RefreshWaiter: Send + Sync {
    /// The refresh at `time` on the source's monotonic clock. It runs on the
    /// source's thread, holding none of the pacer's locks.
    fn refresh(&self, time: Duration);
}

impl<F: Fn(Duration) + Send + Sync> RefreshWaiter for F {
    fn refresh(&self, time: Duration) {
        self(time);
    }
}

/// Calls each waiter that [requested](Self::request) a refresh once, at the
/// first refresh that its limit allows after the request. Dropping the pacer
/// drops its source, which stops.
pub struct FramePacer {
    state: Mutex<PacerState>,
    /// Held while the limit changes, so that the source ends with the
    /// throttle's limit.
    limit_lock: Mutex<()>,
    source: Box<dyn VsyncSource>,
}

struct PacerState {
    /// The waiters that asked for a refresh since the last callback.
    waiting: Vec<Arc<dyn RefreshWaiter>>,
    /// Whether the source has a refresh on its way.
    scheduled: bool,
    throttle: Throttle,
}

impl FramePacer {
    /// A pacer on the platform's main display. It calls back on a thread of
    /// its own, or on Android, on the calling thread's looper.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) elsewhere,
    /// including before macOS 14, with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) on Android when the
    /// calling thread has no `ALooper`, and with
    /// [`ErrorKind::Native`](mln_style::error::ErrorKind) without a display.
    pub fn new(limit: Option<Limit>) -> Result<Arc<Self>, Error> {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
        let source = |vsyncs, limit| apple::display_link::DisplayLinkSource::new(vsyncs, limit, None);
        #[cfg(target_os = "android")]
        let source = android::choreographer::ChoreographerSource::new;
        #[cfg(target_env = "ohos")]
        let source = ohos::native_vsync::NativeVsyncSource::new;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "tvos",
            target_os = "android",
            target_env = "ohos"
        )))]
        let source = |_, _| Err::<Unsupported, _>(Error::unsupported("the platform's display has no frame pacer"));
        Self::with_source(limit, source)
    }

    /// A pacer on the display that `view`'s window is on. On iOS and tvOS,
    /// make a new one when the scene moves to another screen.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) off the
    /// main thread or for a `UIView` in no window, and with
    /// [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) before macOS 14.
    ///
    /// # Safety
    ///
    /// `view` points to an `NSView` on macOS, or a `UIView` on iOS and tvOS.
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    #[expect(unsafe_code, reason = "the view is a pointer to an Objective-C object")]
    pub unsafe fn for_view(
        view: core::ptr::NonNull<core::ffi::c_void>,
        limit: Option<Limit>,
    ) -> Result<Arc<Self>, Error> {
        let view = apple::display_link::View(view);
        Self::with_source(limit, |vsyncs, limit| apple::display_link::DisplayLinkSource::new(vsyncs, limit, Some(view)))
    }

    /// A pacer on the source that `source` makes from where it reports
    /// refreshes and the limit.
    pub fn with_source<S: VsyncSource + 'static, E>(
        limit: Option<Limit>,
        source: impl FnOnce(Vsyncs, Option<Limit>) -> Result<S, E>,
    ) -> Result<Arc<Self>, E> {
        let vsyncs = Vsyncs::default();
        let source = source(vsyncs.clone(), limit)?;
        let pacer = Arc::new(Self {
            state: Mutex::new(PacerState { waiting: Vec::new(), scheduled: false, throttle: Throttle::new(limit) }),
            limit_lock: Mutex::new(()),
            source: Box::new(source),
        });
        // A new `Vsyncs` is unset.
        let _ = vsyncs.0.set(Arc::downgrade(&pacer));
        Ok(pacer)
    }

    /// Asks for a call to `waiter` at the next refresh that the limit allows.
    /// A waiter that asked since the last callback waits for that one.
    pub fn request(&self, waiter: &Arc<dyn RefreshWaiter>) {
        {
            let mut state = self.state.lock();
            if !state.waiting.iter().any(|waiting| Arc::ptr_eq(waiting, waiter)) {
                state.waiting.push(Arc::clone(waiter));
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

    fn on_vsync(&self, time: Duration) {
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
            Some(waiters) => waiters.iter().for_each(|waiter| waiter.refresh(time)),
            // Waits for the next refresh.
            None => self.source.schedule(),
        }
    }
}

/// The source of a platform without one, which no pacer gets.
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "android",
    target_env = "ohos"
)))]
enum Unsupported {}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "android",
    target_env = "ohos"
)))]
impl VsyncSource for Unsupported {
    fn schedule(&self) {
        match *self {}
    }
}

#[cfg(test)]
mod tests;
