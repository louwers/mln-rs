//! Frame pacers: `mln_frame_pacer_create()` and
//! `mln_frame_pacer_set_options()` over `mln_frame_pacer`, and the sources of
//! display refreshes on Android and `OpenHarmony`.
//!
//! A [`Pacer`] keeps the render sessions that have something new to draw, and
//! at the next display refresh that its frame rate limit allows, demands a
//! frame without flags and with token 0 from each of them. Its
//! [`FramePacer`] follows the platform's display: a `CADisplayLink` of
//! `mln_frame_pacer`'s on Apple platforms, or a `VsyncSource` of this
//! crate's, which reports one refresh to its `Vsyncs` each time the pacer
//! schedules one.
//!
//! A session without a redraw wake of the host's has a [`SessionPacing`],
//! whose redraw wake the runtime runs when the map has something new to draw.
//! The pacing records that, and asks its pacer for a frame. A session holds
//! its pacer, so a pacer keeps pacing after its handle's release, until every
//! session that uses it is released or takes another pacer.
//!
//! Locks: a session's pacing lock is never held while it calls into its
//! pacer, and the pacer's locks are never held while it calls into a session
//! or its source, so neither waits on the other.

// Elsewhere no platform creates pacers, and only the tests' fake ones exist.
#![cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "tvos", target_os = "android", target_env = "ohos")),
    expect(dead_code, reason = "elsewhere no platform creates pacers, and only the tests' fake ones exist")
)]

pub(crate) mod android;
pub(crate) mod apple;
pub(crate) mod ohos;

use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use ::mln_frame_pacer::{FramePacer, Limit};
use parking_lot::Mutex;

use crate::abi::{self, Fields};
use crate::error::Error;
use crate::generated::types::{
    MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND,
    mln_diagnostic, mln_frame_pacer, mln_frame_pacer_options, mln_render_session, mln_status,
};
use crate::handles::{self, HandleKind, HandleTable};
use crate::render_session;

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
pub(crate) unsafe extern "C" fn mln_frame_pacer_create(
    options: *const mln_frame_pacer_options,
    out_pacer: *mut mln_frame_pacer,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
        {
            let frames = |limit, on_refresh| FramePacer::new(limit, on_refresh).map_err(Error::from);
            // SAFETY: the caller's contract.
            unsafe { create(options, out_pacer, frames) }
        }
        #[cfg(target_os = "android")]
        {
            let frames = |limit, on_refresh| {
                FramePacer::with_source(limit, on_refresh, android::choreographer::ChoreographerSource::new)
            };
            // SAFETY: the caller's contract.
            unsafe { create(options, out_pacer, frames) }
        }
        #[cfg(target_env = "ohos")]
        {
            let frames = |limit, on_refresh| {
                FramePacer::with_source(limit, on_refresh, ohos::native_vsync::NativeVsyncSource::new)
            };
            // SAFETY: the caller's contract.
            unsafe { create(options, out_pacer, frames) }
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
pub(crate) unsafe extern "C" fn mln_frame_pacer_set_options(
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

/// The callback of a [`Pacer`]'s [`FramePacer`].
pub(crate) type OnRefresh = Box<dyn Fn(Duration) + Send + Sync>;

/// What `mln_frame_pacer_create()` does on each platform with a source:
/// checks `options` and `out_pacer`, builds the platform's [`FramePacer`]
/// with `frames` from the limit that `options` sets and the new pacer's
/// callback, and writes the new pacer's handle to `out_pacer`.
///
/// # Safety
///
/// `options` is null or points to a readable struct of the size it declares,
/// and `out_pacer` is null or points to a writable, initialized handle.
pub(crate) unsafe fn create(
    options: *const mln_frame_pacer_options,
    out_pacer: *mut mln_frame_pacer,
    frames: impl FnOnce(Option<Limit>, OnRefresh) -> Result<Arc<FramePacer>, Error>,
) -> Result<(), Error> {
    // SAFETY: the caller's contract.
    let limit = unsafe { limit(options) }?;
    // SAFETY: the caller's contract.
    let out_pacer = unsafe { handles::out_handle(out_pacer, "out_pacer") }?;
    let pacer: Arc<OnceLock<Weak<Pacer>>> = Arc::default();
    let refreshes = Arc::clone(&pacer);
    let on_refresh: OnRefresh = Box::new(move |_| {
        if let Some(pacer) = refreshes.get().and_then(Weak::upgrade) {
            pacer.on_refresh();
        }
    });
    let new = Arc::new(Pacer { frames: frames(limit, on_refresh)?, waiting: Mutex::new(Vec::new()) });
    let _ = pacer.set(Arc::downgrade(&new));
    *out_pacer = PACERS.insert(new)?;
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

// MARK: - Pacer

/// A frame pacer: the sessions that wait for a refresh, and the
/// [`FramePacer`] that calls back at the refresh. Dropping it drops the
/// frame pacer, whose source stops.
pub(crate) struct Pacer {
    frames: Arc<FramePacer>,
    /// The sessions that asked for a frame since the last demands.
    waiting: Mutex<Vec<mln_render_session>>,
}

impl Pacer {
    /// Takes `limit` from the next refresh on, including one that is on its
    /// way already.
    pub(crate) fn set_limit(&self, limit: Option<Limit>) {
        self.frames.set_limit(limit);
    }

    /// Asks for a frame demand for `session` at the next refresh that the
    /// limit allows.
    fn request(&self, session: mln_render_session) {
        {
            let mut waiting = self.waiting.lock();
            if !waiting.contains(&session) {
                waiting.push(session);
            }
        }
        self.frames.request();
    }

    fn on_refresh(&self) {
        let due = std::mem::take(&mut *self.waiting.lock());
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
            let mut state = self.0.lock();
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
            let mut state = self.0.lock();
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
        self.0.lock().redraw = false;
    }

    /// Whether `pacer` paces the session and the map has something new to
    /// draw, which the pacer's demand then answers.
    pub(crate) fn take_redraw(&self, pacer: &Pacer) -> bool {
        let mut state = self.0.lock();
        let paced = state.pacer.as_deref().is_some_and(|own| core::ptr::eq(own, pacer));
        let take = paced && state.redraw;
        state.redraw &= !take;
        take
    }
}

#[cfg(test)]
pub(crate) mod tests;
