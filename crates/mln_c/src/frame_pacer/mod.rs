//! Frame pacers: `mln_frame_pacer_create()` and
//! `mln_frame_pacer_set_options()`, the part that every platform shares, and
//! each platform's source of display refreshes.
//!
//! A [`Pacer`] keeps the render sessions that have something new to draw, and
//! at the next display refresh that its frame rate limit allows, demands a
//! frame without flags and with token 0 from each of them. A platform gives
//! it a [`VsyncSource`], which reports one refresh to [`Vsyncs`] each time the
//! pacer schedules one.
//!
//! A session without a redraw wake of the host's has a [`SessionPacing`],
//! whose redraw wake the runtime runs when the map has something new to draw.
//! The pacing records that, and asks its pacer for a frame. A session holds
//! its pacer, so a pacer keeps pacing after its handle's release, until every
//! session that uses it is released or takes another pacer.
//!
//! Locks: a session's pacing lock is never held while it calls into its
//! pacer, and the pacer's lock is never held while it calls into a session
//! or its source, so neither waits on the other. A pacer's limit lock orders
//! the changes of its limit, and is held while it tells its source, which
//! never calls back into the pacer then.

// Elsewhere no platform creates pacers, and only the tests' fake ones exist.
#![cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "tvos", target_os = "android", target_env = "ohos")),
    allow(dead_code)
)]

pub(crate) mod android;
pub(crate) mod apple;
pub(crate) mod ohos;

use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use crate::abi::{self, Fields};
use crate::error::Error;
use crate::generated::types::{
    MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND,
    mln_diagnostic, mln_frame_pacer, mln_frame_pacer_options, mln_render_session, mln_status,
};
use crate::handles::{self, HandleKind, HandleTable};
use crate::{lock, render_session};

abi::versioned!(mln_frame_pacer_options);

static PACERS: HandleTable<Pacer> = HandleTable::new(HandleKind::FramePacer);

/// The pacer that `pacer` names.
pub(crate) fn resolve(pacer: mln_frame_pacer) -> Result<Arc<Pacer>, Error> {
    PACERS.get(pacer)
}

/// Releases the handle `pacer`, as `mln_frame_pacer_release()` does. The
/// sessions that use the pacer keep it.
pub(crate) fn release(pacer: mln_frame_pacer) {
    if pacer != handles::NULL {
        let _ = PACERS.remove(pacer);
    }
}

/// `mln_frame_pacer_create()`: a pacer on the platform's source of display
/// refreshes, which is a `CADisplayLink` on macOS, iOS and tvOS, the calling
/// thread's `AChoreographer` on Android, and an `OH_NativeVSync` on
/// `OpenHarmony`. It fails with `MLN_STATUS_UNSUPPORTED` elsewhere.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_frame_pacer_create(
    options: *const mln_frame_pacer_options,
    out_pacer: *mut mln_frame_pacer,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
        {
            // SAFETY: the caller's contract.
            unsafe { create(options, out_pacer, apple::display_link::DisplayLinkSource::new) }
        }
        #[cfg(target_os = "android")]
        {
            // SAFETY: the caller's contract.
            unsafe { create(options, out_pacer, android::choreographer::ChoreographerSource::new) }
        }
        #[cfg(target_env = "ohos")]
        {
            // SAFETY: the caller's contract.
            unsafe { create(options, out_pacer, ohos::native_vsync::NativeVsyncSource::new) }
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "tvos",
            target_os = "android",
            target_env = "ohos"
        )))]
        {
            let _ = (options, out_pacer);
            Err(Error::new(
                mln_util::ErrorKind::Unsupported,
                "mln_frame_pacer_create needs macOS, iOS, tvOS, Android or OpenHarmony",
            ))
        }
    })
}

/// `mln_frame_pacer_set_options()`: changes the limit of the pacer that
/// `pacer` names, which its sessions follow from the next refresh.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_frame_pacer_set_options(
    pacer: mln_frame_pacer,
    options: *const mln_frame_pacer_options,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let pacer = resolve(pacer)?;
        // SAFETY: the caller's contract.
        let options = unsafe { abi::read_sized(options) }?;
        if let LimitChange::Set(limit) = limit_change(&options)? {
            pacer.set_limit(limit);
        }
        Ok(())
    })
}

/// What `mln_frame_pacer_create()` does on each platform with a source:
/// checks `options` and `out_pacer`, builds the platform's source with the
/// limit that `options` sets, and writes the new pacer's handle to
/// `out_pacer`.
///
/// # Safety
///
/// `options` is null or points to a readable struct of the size it declares,
/// and `out_pacer` is null or points to a writable, initialized handle.
pub(crate) unsafe fn create<S: VsyncSource + 'static>(
    options: *const mln_frame_pacer_options,
    out_pacer: *mut mln_frame_pacer,
    source: impl FnOnce(Vsyncs, Option<Limit>) -> Result<S, Error>,
) -> Result<(), Error> {
    // SAFETY: the caller's contract.
    let limit = unsafe { limit(options) }?;
    // SAFETY: the caller's contract.
    let out_pacer = unsafe { handles::out_handle(out_pacer, "out_pacer") }?;
    let vsyncs = Vsyncs::default();
    let source = source(vsyncs.clone(), limit)?;
    *out_pacer = PACERS.insert(Pacer::new(limit, vsyncs, Box::new(source)))?;
    Ok(())
}

/// The frame rate limit that a new pacer takes from `options`: none for
/// null options, or options that don't set one.
///
/// # Safety
///
/// `options` is null or points to a readable struct of the size it declares.
unsafe fn limit(options: *const mln_frame_pacer_options) -> Result<Option<Limit>, Error> {
    if options.is_null() {
        return Ok(None);
    }
    // SAFETY: the caller's contract.
    let options = unsafe { abi::read_sized(options) }?;
    Ok(match limit_change(&options)? {
        LimitChange::Keep => None,
        LimitChange::Set(limit) => limit,
    })
}

/// What options do to a pacer's frame rate limit.
enum LimitChange {
    /// Keep it, as options that set neither a limit nor its removal do.
    Keep,
    /// Set it, or remove it with none.
    Set(Option<Limit>),
}

/// What `options` does to a pacer's frame rate limit.
fn limit_change(options: &mln_frame_pacer_options) -> Result<LimitChange, Error> {
    let fields = Fields::new(
        options.fields,
        MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND | MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND,
        "mln_frame_pacer_options.fields",
    )?;
    let limit = fields.get(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, options.maximum_frames_per_second);
    match (limit, fields.get(MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND, ())) {
        (Some(_), Some(())) => Err(Error::invalid_argument(
            "MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND and MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND \
             are mutually exclusive",
        )),
        (Some(frames_per_second), None) => {
            Limit::new(frames_per_second).map(|limit| LimitChange::Set(Some(limit))).ok_or_else(|| {
                Error::invalid_argument("mln_frame_pacer_options.maximum_frames_per_second must be greater than zero")
            })
        }
        (None, Some(())) => Ok(LimitChange::Set(None)),
        (None, None) => Ok(LimitChange::Keep),
    }
}

// MARK: - Limit

/// A frame rate limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limit {
    frames_per_second: u32,
    period: Duration,
}

impl Limit {
    fn new(frames_per_second: u32) -> Option<Self> {
        let period = Duration::from_secs(1).checked_div(frames_per_second)?;
        Some(Self { frames_per_second, period })
    }

    /// The most frames per second.
    #[cfg_attr(not(any(target_os = "macos", target_os = "ios", target_os = "tvos")), allow(dead_code))]
    pub(crate) fn frames_per_second(self) -> u32 {
        self.frames_per_second
    }

    /// How early a refresh can come and still count as on time: a display
    /// that runs at the limit's rate reports refreshes with some jitter, and
    /// a pacer that skipped each one that came a little early would demand
    /// frames at half the rate.
    fn slack(self) -> Duration {
        self.period / 10
    }
}

/// Which refreshes a limit lets the pacer demand frames at.
///
/// It demands frames on a grid of the limit's period, so that over any span
/// it demands no more than the limit's rate, give or take one frame. A
/// refresh within the slack before the grid's next point takes that point,
/// and one that comes a whole period after it, as after an idle spell,
/// starts the grid again. A new limit's grid starts a period of it after the
/// last demand.
#[derive(Debug, Default)]
struct Throttle {
    limit: Option<Limit>,
    /// The grid's next point.
    next: Option<Duration>,
    /// The refresh that the pacer last demanded frames at, with or without
    /// a limit.
    last: Option<Duration>,
}

impl Throttle {
    fn new(limit: Option<Limit>) -> Self {
        Self { limit, ..Self::default() }
    }

    /// Whether the pacer may demand frames at a refresh at `time`, which
    /// then counts as a demand.
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

    /// Takes `limit` from the next refresh on, counted from the last demand:
    /// a refresh within the new limit's period after it waits for the next.
    fn set_limit(&mut self, limit: Option<Limit>) {
        self.limit = limit;
        self.next = limit.zip(self.last).map(|(limit, last)| last + limit.period);
    }
}

// MARK: - Pacer

/// A platform's source of display refreshes.
pub(crate) trait VsyncSource: Send + Sync {
    /// Arranges one call to [`Vsyncs::vsync`] at the display's next refresh.
    /// It runs on any thread, holding none of the library's locks, and doesn't
    /// block.
    fn schedule(&self);

    /// Follows a new frame rate limit, which the pacer's throttle applies to
    /// the refreshes either way. It runs on any thread, holding the pacer's
    /// limit lock and no other lock of the library's, and doesn't block. The
    /// default ignores it, for a source whose refreshes don't depend on it.
    fn set_limit(&self, _limit: Option<Limit>) {}
}

/// Where a [`VsyncSource`] reports refreshes: to its pacer, while it lives.
/// A source that the pacer drops can still report the refreshes it
/// scheduled, which then go nowhere.
#[derive(Clone, Default)]
pub(crate) struct Vsyncs(Arc<OnceLock<Weak<Pacer>>>);

impl Vsyncs {
    /// Reports a refresh at `time` on a monotonic clock. The pacer demands
    /// frames from its sessions on this thread before it returns.
    pub(crate) fn vsync(&self, time: Duration) {
        if let Some(pacer) = self.0.get().and_then(Weak::upgrade) {
            pacer.on_vsync(time);
        }
    }
}

/// A frame pacer: the sessions that wait for a refresh, its limit, and its
/// platform's source of refreshes. Dropping it drops the source, which stops.
pub(crate) struct Pacer {
    state: Mutex<PacerState>,
    /// Held while the limit changes, so that the source ends with the
    /// throttle's limit.
    limit_lock: Mutex<()>,
    source: Box<dyn VsyncSource>,
}

struct PacerState {
    /// The sessions that asked for a frame since the last demands.
    waiting: Vec<mln_render_session>,
    /// Whether the source has a refresh on its way.
    scheduled: bool,
    throttle: Throttle,
}

impl Pacer {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the new pacer takes the handle that its source's refreshes reach it by"
    )]
    pub(crate) fn new(limit: Option<Limit>, vsyncs: Vsyncs, source: Box<dyn VsyncSource>) -> Arc<Self> {
        let pacer = Arc::new(Self {
            state: Mutex::new(PacerState { waiting: Vec::new(), scheduled: false, throttle: Throttle::new(limit) }),
            limit_lock: Mutex::new(()),
            source,
        });
        // A new `Vsyncs` is unset.
        let _ = vsyncs.0.set(Arc::downgrade(&pacer));
        pacer
    }

    /// Takes `limit` from the next refresh on, including one that is on its
    /// way already.
    pub(crate) fn set_limit(&self, limit: Option<Limit>) {
        let _changing = lock(&self.limit_lock);
        lock(&self.state).throttle.set_limit(limit);
        self.source.set_limit(limit);
    }

    /// Asks for a frame demand for `session` at the next refresh that the
    /// limit allows.
    fn request(&self, session: mln_render_session) {
        {
            let mut state = lock(&self.state);
            if !state.waiting.contains(&session) {
                state.waiting.push(session);
            }
            if std::mem::replace(&mut state.scheduled, true) {
                return;
            }
        }
        self.source.schedule();
    }

    fn on_vsync(&self, time: Duration) {
        let due = {
            let mut state = lock(&self.state);
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
        let Some(due) = due else {
            // Waits for the next refresh.
            return self.source.schedule();
        };
        for session in due {
            render_session::demand_paced(session, self);
        }
    }
}

// MARK: - Sessions

/// A render session's side of pacing: its pacer, and whether the map has
/// something new to draw that no demand has answered.
#[derive(Default)]
pub(crate) struct SessionPacing(Mutex<PacingState>);

#[derive(Default)]
struct PacingState {
    pacer: Option<Arc<Pacer>>,
    /// The session's handle, once it has a pacer.
    session: mln_render_session,
    redraw: bool,
}

impl SessionPacing {
    /// The redraw wake that the runtime runs for the session.
    pub(crate) fn wake(self: &Arc<Self>) -> ::mln_runtime::Wake {
        let pacing = Arc::clone(self);
        Arc::new(move || pacing.redraw())
    }

    /// The map has something new to draw.
    fn redraw(&self) {
        let request = {
            let mut state = lock(&self.0);
            state.redraw = true;
            state.pacer.clone().map(|pacer| (pacer, state.session))
        };
        if let Some((pacer, session)) = request {
            pacer.request(session);
        }
    }

    /// Paces `session` with `pacer`, or stops pacing it. A new pacer demands
    /// at its next refresh what the map had to draw.
    pub(crate) fn set(&self, session: mln_render_session, pacer: Option<Arc<Pacer>>) {
        let (previous, request) = {
            let mut state = lock(&self.0);
            state.session = session;
            let previous = std::mem::replace(&mut state.pacer, pacer);
            let request = state.pacer.clone().filter(|_| state.redraw);
            (previous, request)
        };
        // Dropped outside the lock, since the last session to drop a pacer
        // stops its source.
        drop(previous);
        if let Some(pacer) = request {
            pacer.request(session);
        }
    }

    /// A demand is on its way, which answers what the map had to draw. Runs
    /// before the demand, so that a redraw that comes after the demand asks
    /// for another.
    pub(crate) fn demanded(&self) {
        lock(&self.0).redraw = false;
    }

    /// Whether `pacer` paces the session and the map has something new to
    /// draw, which the pacer's demand then answers.
    pub(crate) fn take_redraw(&self, pacer: &Pacer) -> bool {
        let mut state = lock(&self.0);
        let paced = state.pacer.as_deref().is_some_and(|own| core::ptr::eq(own, pacer));
        let take = paced && state.redraw;
        state.redraw &= !take;
        take
    }
}

#[cfg(test)]
pub(crate) mod tests;
