//! `render_session.h`: the life of a render session, its frame demands and
//! results, and the work it runs on its driver.

use core::ffi::c_char;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::thread::ThreadId;

use ::mln_runtime::{
    AttachOptions, ClipDepthRange, DemandMode, Driver, DriverWork, FrameDemand, FrameDisposition, FrameProjection,
    FrameResult, LogicalExtent, RenderSession, RenderSessionState, RenderTarget,
};
use mln_util::ErrorKind;
use mln_util::size::Size;
use parking_lot::Mutex;

use crate::abi::{self, Fields};
use crate::callback::Wake;
use crate::completion::{CompletionValue, Generations, PendingCompletion};
use crate::error::Error;
use crate::frame_pacer::{self, Pacer, SessionPacing};
use crate::generated::types::{
    MLN_CLIP_DEPTH_RANGE_NEGATIVE_ONE_TO_ONE, MLN_CLIP_DEPTH_RANGE_ZERO_TO_ONE, MLN_FRAME_DEMAND_FORCE,
    MLN_FRAME_DEMAND_WAIT_FOR_UPDATE, MLN_GPU_SYNC_CPU_COMPLETE, MLN_GPU_SYNC_KIND_FLAG_CPU_COMPLETE,
    MLN_GPU_SYNC_KIND_FLAG_D3D12_FENCE, MLN_GPU_SYNC_KIND_FLAG_METAL_SHARED_EVENT,
    MLN_GPU_SYNC_KIND_FLAG_OPEN_GL_FENCE, MLN_GPU_SYNC_KIND_FLAG_QUEUE_ORDER, MLN_GPU_SYNC_KIND_FLAG_SYNC_FD,
    MLN_GPU_SYNC_KIND_FLAG_VULKAN_TIMELINE_SEMAPHORE, MLN_RENDER_ABANDON_DISPOSITION_CLEAN,
    MLN_RENDER_ABANDON_DISPOSITION_QUARANTINED, MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD,
    MLN_RENDER_DRIVER_CORE_WORKER, MLN_RENDER_RESULT_FAILED, MLN_RENDER_RESULT_NO_UPDATE, MLN_RENDER_RESULT_RENDERED,
    MLN_RENDER_RESULT_SUPERSEDED, MLN_RENDER_RESULT_TARGET_NOT_READY, MLN_RENDER_SESSION_CAPABILITY_FRAME_ACQUISITION,
    MLN_RENDER_SESSION_CAPABILITY_PRESENTATION, MLN_RENDER_SESSION_CAPABILITY_READBACK,
    MLN_RENDER_SESSION_STATE_ABANDONED, MLN_RENDER_SESSION_STATE_ATTACHED, MLN_RENDER_SESSION_STATE_ATTACHING,
    MLN_RENDER_SESSION_STATE_DETACHED, MLN_RENDER_SESSION_STATE_DETACHING, MLN_RENDER_SESSION_STATE_FAILED,
    MLN_STATUS_INVALID_STATE, MLN_STATUS_NATIVE_ERROR, MLN_STATUS_OK, MLN_STATUS_TARGET_LOST, mln_completion,
    mln_diagnostic, mln_frame_demand, mln_frame_pacer, mln_frame_projection, mln_logical_extent, mln_map_view,
    mln_mat4, mln_render_abandon_result, mln_render_frame_batch, mln_render_frame_batch_view, mln_render_frame_result,
    mln_render_session, mln_render_session_attach_options, mln_render_session_capabilities,
    mln_render_session_snapshot, mln_status, mln_vec4,
};
use crate::handles::{self, HandleKind, HandleTable};
use crate::map::{self, MapObject};
use crate::{buffer, events, projection, render_target};

abi::versioned!(
    mln_render_session_attach_options,
    mln_render_session_capabilities,
    mln_render_session_snapshot,
    mln_frame_demand,
    mln_render_frame_batch_view,
    mln_render_abandon_result,
);

pub(crate) struct SessionObject {
    pub(crate) session: RenderSession,
    /// The session's map, whose snapshot gives the generations of the
    /// session's queries.
    pub(crate) map: ::mln_runtime::Map,
    /// The scale factor that the caller created the map with, which the
    /// session's extents and views report exactly.
    pub(crate) initial_scale_factor: f64,
    /// For a session attached without a redraw wake: its frame pacer, which
    /// takes the runtime's redraw wakes.
    pacing: Option<Arc<SessionPacing>>,
    /// Held so that its user data lives as long as the session's handle: a
    /// core worker never runs it, and a caller-driven session lets go of it
    /// once its driver work finishes.
    _driver_work_wake: Arc<Wake>,
}

impl Drop for SessionObject {
    /// A released session stops pacing, and lets go of its pacer.
    fn drop(&mut self) {
        if let Some(pacing) = &self.pacing {
            pacing.set(handles::NULL, None);
        }
    }
}

static SESSIONS: HandleTable<SessionObject> = HandleTable::new(HandleKind::RenderSession);

/// The results of one drain, which their batch handle owns.
struct FrameBatch(Box<[mln_render_frame_result]>);

// SAFETY: the results are plain values; their views point nowhere.
unsafe impl Send for FrameBatch {}
// SAFETY: as for `Send`.
unsafe impl Sync for FrameBatch {}

static BATCHES: HandleTable<FrameBatch> = HandleTable::new(HandleKind::RenderFrameBatch);

/// The session that `session` names.
pub(crate) fn resolve(session: mln_render_session) -> Result<Arc<SessionObject>, Error> {
    SESSIONS.get(session)
}

/// `MLN_FRAME_DEMAND_TOKEN_STILL_IMAGE`: the token of a still image's frame
/// results, which a host's demand can't use.
const STILL_IMAGE_TOKEN: u64 = u64::MAX;

// MARK: - Attach

const KNOWN_SYNC_KINDS: u32 = MLN_GPU_SYNC_KIND_FLAG_CPU_COMPLETE
    | MLN_GPU_SYNC_KIND_FLAG_OPEN_GL_FENCE
    | MLN_GPU_SYNC_KIND_FLAG_QUEUE_ORDER
    | MLN_GPU_SYNC_KIND_FLAG_VULKAN_TIMELINE_SEMAPHORE
    | MLN_GPU_SYNC_KIND_FLAG_SYNC_FD
    | MLN_GPU_SYNC_KIND_FLAG_METAL_SHARED_EVENT
    | MLN_GPU_SYNC_KIND_FLAG_D3D12_FENCE;

/// A caller's checked `mln_render_session_attach_options`.
pub(crate) struct AttachRequest {
    driver: Driver,
    frame_wake: Arc<Wake>,
    redraw_wake: Arc<Wake>,
    has_redraw_wake: bool,
    driver_work_wake: Arc<Wake>,
    producer_sync_kinds: ::mln_runtime::SyncKinds,
    pub(crate) keep_depth: bool,
}

/// Checks a caller's attach options.
///
/// # Safety
///
/// `options` is null or points to a readable struct of the size it declares.
pub(crate) unsafe fn attach_options(options: *const mln_render_session_attach_options) -> Result<AttachRequest, Error> {
    // SAFETY: the caller's contract.
    let options = unsafe { abi::read_sized(options) }?;
    let driver = match options.driver {
        MLN_RENDER_DRIVER_CORE_WORKER => Driver::CoreWorker,
        MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD => Driver::CallerThread,
        _ => return Err(Error::invalid_argument("render driver kind is invalid")),
    };
    if options.producer_sync_kinds & !KNOWN_SYNC_KINDS != 0 {
        return Err(Error::invalid_argument("producer synchronization kinds are invalid"));
    }
    Ok(AttachRequest {
        driver,
        frame_wake: Arc::new(Wake::new(&options.frame_wake)?),
        redraw_wake: Arc::new(Wake::new(&options.redraw_wake)?),
        has_redraw_wake: options.redraw_wake.callback.is_some(),
        driver_work_wake: Arc::new(Wake::new(&options.driver_work_wake)?),
        producer_sync_kinds: crate::gpu_device::sync_kinds(options.producer_sync_kinds),
        keep_depth: options.keep_depth,
    })
}

impl AttachRequest {
    /// Fails with `MLN_STATUS_UNSUPPORTED` unless the caller asked for a core
    /// worker.
    pub(crate) fn require_core_worker(&self, message: &'static str) -> Result<(), Error> {
        if self.driver == Driver::CoreWorker { Ok(()) } else { Err(Error::new(ErrorKind::Unsupported, message)) }
    }

    fn wakes(&self) -> [&Wake; 3] {
        [&self.frame_wake, &self.redraw_wake, &self.driver_work_wake]
    }
}

/// The runtime's form of a caller's wake.
fn runtime_wake(wake: &Arc<Wake>) -> ::mln_runtime::Wake {
    let wake = Arc::clone(wake);
    Arc::new(move || wake.notify())
}

/// Attaches a session to `map`, gives it a handle, and calls `attached`
/// with the handle and the attach's completion before the session's thread
/// may deliver that, so that a callback attached there runs inside the
/// session's driver call. The request's references to the wakes go before
/// `attached` runs, which leaves the session's as the last ones.
pub(crate) fn attach(
    map: &MapObject,
    target: RenderTarget,
    request: AttachRequest,
    attached: impl FnOnce(mln_render_session, ::mln_runtime::Completion<()>),
) -> Result<(), Error> {
    let pacing = (!request.has_redraw_wake).then(|| Arc::new(SessionPacing::default()));
    let options = AttachOptions {
        driver: request.driver,
        frame_wake: Some(runtime_wake(&request.frame_wake)),
        redraw_wake: Some(match &pacing {
            Some(pacing) => pacing.wake(),
            None => runtime_wake(&request.redraw_wake),
        }),
        driver_work_wake: Some(runtime_wake(&request.driver_work_wake)),
        producer_sync_kinds: request.producer_sync_kinds,
    };
    // Accepted first, so that a wake that the session runs as it starts
    // reaches the caller.
    request.wakes().iter().for_each(|wake| wake.accept());
    let revoke = |request: &Option<AttachRequest>| {
        request.iter().flat_map(AttachRequest::wakes).for_each(Wake::revoke);
    };
    let driver_work_wake = Arc::clone(&request.driver_work_wake);
    let mut request = Some(request);
    let sent = map.map.attach_then(target, options, |session, completion| {
        let object = SessionObject {
            session: session.clone(),
            map: map.map.clone(),
            initial_scale_factor: map.scale_factor,
            pacing,
            _driver_work_wake: driver_work_wake,
        };
        match SESSIONS.insert(Arc::new(object)) {
            Ok(handle) => {
                request = None;
                attached(handle, completion);
                Ok(())
            }
            Err(error) => {
                revoke(&request);
                let _ = session.dispose();
                Err(error)
            }
        }
    });
    sent.unwrap_or_else(|error| {
        revoke(&request);
        Err(error.into())
    })
}

// MARK: - State

fn state(state: RenderSessionState) -> u32 {
    match state {
        RenderSessionState::Attaching => MLN_RENDER_SESSION_STATE_ATTACHING,
        RenderSessionState::Attached => MLN_RENDER_SESSION_STATE_ATTACHED,
        RenderSessionState::Detaching => MLN_RENDER_SESSION_STATE_DETACHING,
        RenderSessionState::Detached => MLN_RENDER_SESSION_STATE_DETACHED,
        RenderSessionState::Failed => MLN_RENDER_SESSION_STATE_FAILED,
        RenderSessionState::Abandoned => MLN_RENDER_SESSION_STATE_ABANDONED,
    }
}

/// The status that a session's failure reports.
fn failure_status(failure: Option<&mln_util::Error>) -> mln_status {
    match failure {
        None => MLN_STATUS_OK,
        Some(error) if error.kind == ErrorKind::TargetLost => MLN_STATUS_TARGET_LOST,
        Some(_) => MLN_STATUS_NATIVE_ERROR,
    }
}

/// `mln_render_session_get_capabilities()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_get_capabilities(
    session: mln_render_session,
    out_capabilities: *mut mln_render_session_capabilities,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_capabilities), {
        let capabilities = resolve(session)?.session.capabilities()?;
        let flag = |set: bool, bit: u32| if set { bit } else { 0 };
        let capabilities = mln_render_session_capabilities {
            size: size_of::<mln_render_session_capabilities>() as u32,
            driver: match capabilities.driver {
                Driver::CoreWorker => MLN_RENDER_DRIVER_CORE_WORKER,
                Driver::CallerThread => MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD,
            },
            texture_ring_depth: capabilities.texture_ring_depth,
            flags: flag(capabilities.frame_acquisition, MLN_RENDER_SESSION_CAPABILITY_FRAME_ACQUISITION)
                | flag(capabilities.readback, MLN_RENDER_SESSION_CAPABILITY_READBACK)
                | flag(capabilities.presentation, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION),
            // A session whose frames don't leave it has nothing to wait on.
            producer_sync_kind: capabilities
                .producer_sync_kind
                .map_or(MLN_GPU_SYNC_CPU_COMPLETE, crate::gpu_device::sync_kind_value),
            consumer_sync_kinds: crate::gpu_device::sync_flags(capabilities.consumer_sync_kinds),
            export_kind: capabilities
                .export_kind
                .map_or(crate::generated::types::MLN_GPU_EXPORT_KIND_NONE, render_target::export_kind_value),
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_capabilities, &capabilities) }
    })
}

/// `mln_render_session_get_snapshot()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_get_snapshot(
    session: mln_render_session,
    out_snapshot: *mut mln_render_session_snapshot,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_snapshot), {
        let object = resolve(session)?;
        let snapshot = object.session.snapshot()?;
        let snapshot = mln_render_session_snapshot {
            size: size_of::<mln_render_session_snapshot>() as u32,
            state: state(snapshot.state),
            extent: mln_logical_extent {
                width: snapshot.extent.size.width,
                height: snapshot.extent.size.height,
                scale_factor: map::exact_scale_factor(object.initial_scale_factor, snapshot.extent.scale_factor),
            },
            generation: snapshot.generation,
            frame_generation: snapshot.frame_generation,
            acquired_frame_count: snapshot.acquired_frames,
            failure_status: failure_status(snapshot.failure.as_ref()),
            has_target: snapshot.has_target,
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_snapshot, &snapshot) }
    })
}

/// `mln_render_session_get_failure()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_get_failure(
    session: mln_render_session,
    out_message: *mut c_char,
    message_capacity: usize,
    out_message_length: *mut usize,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let snapshot = resolve(session)?.session.snapshot()?;
        let message = snapshot.failure.as_ref().map_or("", |error| error.message.as_str());
        let names = ("out_message", "out_message_length");
        // SAFETY: the caller's contract.
        unsafe { buffer::write_text(message, out_message, message_capacity, out_message_length, names) }
    })
}

// MARK: - Frames

/// `mln_render_session_demand_frame()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_demand_frame(
    session: mln_render_session,
    demand: *const mln_frame_demand,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        let demand = unsafe { abi::read_sized(demand) }?;
        let flags = Fields::new(
            demand.flags,
            MLN_FRAME_DEMAND_FORCE | MLN_FRAME_DEMAND_WAIT_FOR_UPDATE,
            "mln_frame_demand.flags",
        )?;
        let mode = match (flags.get(MLN_FRAME_DEMAND_FORCE, ()), flags.get(MLN_FRAME_DEMAND_WAIT_FOR_UPDATE, ())) {
            (None, None) => DemandMode::IfChanged,
            (Some(()), None) => DemandMode::Force,
            (None, Some(())) => DemandMode::WaitForUpdate,
            (Some(()), Some(())) => {
                return Err(Error::invalid_argument("a frame demand that waits for a map update cannot force"));
            }
        };
        if demand.token == STILL_IMAGE_TOKEN {
            return Err(Error::invalid_argument("the token is reserved for still images"));
        }
        let object = resolve(session)?;
        if let Some(pacing) = &object.pacing {
            pacing.demanded();
        }
        object.session.demand_frame(FrameDemand::new(mode, demand.token))?;
        Ok(())
    })
}

/// The demand that `pacer` makes at a display refresh: a frame without flags
/// and with token 0, when `pacer` paces `session` and its map has something
/// new to draw. A session that has gone or failed since it asked refuses it.
pub(crate) fn demand_paced(session: mln_render_session, pacer: &Pacer) {
    let Ok(object) = resolve(session) else {
        return;
    };
    if object.pacing.as_ref().is_some_and(|pacing| pacing.take_redraw(pacer)) {
        let _ = object.session.demand_frame(FrameDemand::new(DemandMode::IfChanged, 0));
    }
}

/// `mln_render_session_set_frame_pacer()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_set_frame_pacer(
    session: mln_render_session,
    pacer: mln_frame_pacer,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let pacer = if pacer == handles::NULL { None } else { Some(frame_pacer::resolve(pacer)?) };
        let object = resolve(session)?;
        let snapshot = object.session.snapshot()?;
        match snapshot.state {
            RenderSessionState::Attaching | RenderSessionState::Attached => {}
            RenderSessionState::Failed => {
                let failure = snapshot.failure.unwrap_or_else(|| mln_util::Error::native("the render session failed"));
                return Err(failure.into());
            }
            _ => return Err(Error::invalid_state("the render session is not attached: it was detached or abandoned")),
        }
        let Some(pacing) = &object.pacing else {
            return Err(Error::invalid_state("a render session with a redraw wake takes no pacer"));
        };
        pacing.set(session, pacer);
        Ok(())
    })
}

/// `mln_frame_pacer_release()`.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn mln_frame_pacer_release(pacer: mln_frame_pacer) {
    frame_pacer::release(pacer);
}

fn disposition(disposition: FrameDisposition) -> u32 {
    match disposition {
        FrameDisposition::Rendered => MLN_RENDER_RESULT_RENDERED,
        FrameDisposition::NoUpdate => MLN_RENDER_RESULT_NO_UPDATE,
        FrameDisposition::TargetNotReady => MLN_RENDER_RESULT_TARGET_NOT_READY,
        FrameDisposition::Superseded => MLN_RENDER_RESULT_SUPERSEDED,
        FrameDisposition::Failed => MLN_RENDER_RESULT_FAILED,
    }
}

pub(crate) fn frame_result(result: &FrameResult, initial_scale_factor: f64) -> mln_render_frame_result {
    let view = match &result.view {
        Some(view) => {
            let mut view = projection::from_map_view(view);
            view.extent.scale_factor = map::exact_scale_factor(initial_scale_factor, view.extent.scale_factor);
            view
        }
        // SAFETY: zeros are a valid `mln_map_view`, of no size, as a result
        // that rendered nothing has.
        None => unsafe { core::mem::zeroed::<mln_map_view>() },
    };
    mln_render_frame_result {
        size: size_of::<mln_render_frame_result>() as u32,
        disposition: disposition(result.disposition),
        token: result.token.unwrap_or(STILL_IMAGE_TOKEN),
        map_update_generation: result.map_update_generation,
        generation: result.generation,
        frame_generation: result.frame_generation,
        needs_repaint: result.needs_repaint,
        view,
        mode: events::render_mode(result.mode),
        frame_projection: result.projection.as_ref().map_or_else(
            // SAFETY: zeros are a valid `mln_frame_projection`, as a result
            // that rendered nothing has.
            || unsafe { core::mem::zeroed::<mln_frame_projection>() },
            frame_projection,
        ),
    }
}

/// The `mln_frame_projection` of where a frame drew the map, which frame
/// results and custom layers report.
pub(crate) fn frame_projection(projection: &FrameProjection) -> mln_frame_projection {
    let column = |index: usize| {
        let [x, y, z, w] = [0, 1, 2, 3].map(|row| projection.matrix[4 * index + row]);
        mln_vec4 { x, y, z, w }
    };
    mln_frame_projection {
        matrix: mln_mat4 { x: column(0), y: column(1), z: column(2), w: column(3) },
        clip_depth_range: match projection.clip_depth_range {
            ClipDepthRange::ZeroToOne => MLN_CLIP_DEPTH_RANGE_ZERO_TO_ONE,
            ClipDepthRange::NegativeOneToOne => MLN_CLIP_DEPTH_RANGE_NEGATIVE_ONE_TO_ONE,
        },
        near_distance: projection.near_distance,
        far_distance: projection.far_distance,
        far_depth: projection.far_depth,
        globe: projection.globe,
    }
}

/// `mln_render_session_drain_frame_results()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_drain_frame_results(
    session: mln_render_session,
    out_batch: *mut mln_render_frame_batch,
    out_view: *mut mln_render_frame_batch_view,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_view), {
        // SAFETY: the caller's contract.
        let out_batch = unsafe { handles::out_handle(out_batch, "out_batch") }?;
        let object = resolve(session)?;
        let mut view = mln_render_frame_batch_view {
            size: size_of::<mln_render_frame_batch_view>() as u32,
            result_size: size_of::<mln_render_frame_result>() as u32,
            results: core::ptr::null(),
            result_count: 0,
        };
        // A view too small fails before the drain takes the results.
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_view, &view) }?;
        let results = object.session.drain_frame_results()?;
        if results.is_empty() {
            return Ok(());
        }
        let results: Box<[_]> =
            results.iter().map(|result| frame_result(result, object.initial_scale_factor)).collect();
        let batch = Arc::new(FrameBatch(results));
        view.results = batch.0.as_ptr();
        view.result_count = batch.0.len();
        *out_batch = BATCHES.insert(batch)?;
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_view, &view) }
    })
}

/// `mln_render_frame_batch_release()`.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn mln_render_frame_batch_release(batch: mln_render_frame_batch) {
    if batch != handles::NULL {
        let _ = release_batch(batch);
    }
}

/// Releases `batch`, as `mln_render_frame_batch_release()` does.
pub(crate) fn release_batch(batch: mln_render_frame_batch) -> Result<(), Error> {
    BATCHES.remove(batch).map(drop)
}

// MARK: - Work

/// Sends a session operation, whose completion `value` gives the result of,
/// with the generations of the map's latest snapshot. The session's driver
/// delivers the completion inside its driver call, even when the operation
/// runs before this returns.
///
/// # Safety
///
/// `completion` is null or valid for the header's use of it.
pub(crate) unsafe fn operation<T: Send + 'static, V: CompletionValue>(
    session: mln_render_session,
    completion: *const mln_completion,
    send: impl FnOnce(&RenderSession) -> Result<::mln_runtime::Completion<T>, mln_util::Error>,
    value: impl FnOnce(T) -> V + Send + 'static,
) -> Result<(), Error> {
    let object = resolve(session)?;
    // SAFETY: the caller's contract.
    let pending = unsafe { PendingCompletion::new(completion) }?;
    let map = object.map.clone();
    send_then(&object.session, send, Some(pending), move |result| {
        let generations = map.snapshot().map_or(Generations::default(), |snapshot| Generations {
            snapshot: snapshot.generation,
            render_update: snapshot.update_generation,
        });
        (result.map(value).map_err(Error::from), generations)
    })
}

/// Sends a command to `session` with `send`, and completes `pending` with
/// its result, which `finish` turns into the completion's result and
/// generations. The session's driver delivers the completion inside its
/// driver call, even when the command runs before this returns: a
/// completion attached after the send returned would run on this thread
/// then. A send that fails leaves `pending` unaccepted.
pub(crate) fn send_then<T: Send + 'static, V: CompletionValue>(
    session: &RenderSession,
    send: impl FnOnce(&RenderSession) -> Result<::mln_runtime::Completion<T>, mln_util::Error>,
    pending: Option<PendingCompletion>,
    finish: impl FnOnce(Result<T, mln_util::Error>) -> (Result<V, Error>, Generations) + Send + 'static,
) -> Result<(), Error> {
    // The callback runs once when the send succeeds, and is dropped unrun,
    // with the completion unaccepted, when it fails.
    session.send_then(send, move |result| {
        if let Some(pending) = pending {
            let (result, generations) = finish(result);
            pending.accept().complete(result, generations);
        }
    })?;
    Ok(())
}

/// `mln_render_session_resize()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_resize(
    session: mln_render_session,
    extent: mln_logical_extent,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        render_target::physical_size(extent)?;
        let object = resolve(session)?;
        // SAFETY: the caller's contract.
        let pending = unsafe { PendingCompletion::optional(completion) }?;
        let size = Size::new(extent.width, extent.height);
        let map = object.map.clone();
        let resize = |session: &RenderSession| session.resize(LogicalExtent::new(size, extent.scale_factor));
        send_then(&object.session, resize, pending, move |result| {
            let generations = map::generations(&map, &result);
            (result.map(drop).map_err(Error::from), generations)
        })
    })
}

/// `mln_render_session_release_target()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_release_target(
    session: mln_render_session,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        unsafe { operation(session, completion, RenderSession::release_target, |()| ()) }
    })
}

/// `mln_render_session_barrier()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_barrier(
    session: mln_render_session,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        unsafe { operation(session, completion, RenderSession::barrier, |()| ()) }
    })
}

/// `mln_render_session_reduce_memory_use()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_reduce_memory_use(
    session: mln_render_session,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        unsafe { operation(session, completion, RenderSession::reduce_memory_use, |()| ()) }
    })
}

/// `mln_render_session_clear_data()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_clear_data(
    session: mln_render_session,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        unsafe { operation(session, completion, RenderSession::clear_data, |()| ()) }
    })
}

/// `mln_render_session_dump_debug_logs()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_dump_debug_logs(
    session: mln_render_session,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        unsafe { operation(session, completion, RenderSession::dump_debug_logs, |()| ()) }
    })
}

// MARK: - Driver work

thread_local! {
    /// The driver work of the caller-driven sessions that this thread
    /// services, from its first service of each until the work finishes.
    static DRIVERS: Drivers = Drivers::default();
}

/// A thread's driver work, which stays on the thread: the sessions'
/// renderers and graphics objects live there. A thread that ends with work
/// it didn't finish leaks it, with the graphics objects, since freeing them
/// needs the thread's other locals, which may be gone by then.
#[derive(Default)]
struct Drivers(RefCell<HashMap<mln_render_session, Rc<DriverWork>>>);

impl Drop for Drivers {
    fn drop(&mut self) {
        for (_, work) in self.0.get_mut().drain() {
            std::mem::forget(work);
        }
    }
}

/// The thread that services each caller-driven session, from its first
/// service until its driver work finishes, which outlasts a release of the
/// handle.
static SERVICING: Mutex<BTreeMap<mln_render_session, ThreadId>> = Mutex::new(BTreeMap::new());

/// Fails unless `session`'s driver work is this thread's, or no thread's yet.
fn check_servicing_thread(
    servicing: &BTreeMap<mln_render_session, ThreadId>,
    session: mln_render_session,
) -> Result<(), Error> {
    match servicing.get(&session) {
        Some(thread) if *thread != std::thread::current().id() => {
            Err(Error::wrong_thread("render session driver work belongs to the thread that first serviced it"))
        }
        _ => Ok(()),
    }
}

/// Takes `session`'s driver work for this thread, at its first service.
fn take_driver_work(session: mln_render_session) -> Result<Rc<DriverWork>, Error> {
    check_servicing_thread(&SERVICING.lock(), session)?;
    let object = resolve(session)?;
    if object.session.capabilities()?.driver == Driver::CoreWorker {
        return Err(Error::invalid_state("render session is driven by its own core worker"));
    }
    let mut servicing = SERVICING.lock();
    // Another thread may have taken it meanwhile.
    check_servicing_thread(&servicing, session)?;
    let work = Rc::new(object.session.take_driver_work()?);
    servicing.insert(session, std::thread::current().id());
    drop(servicing);
    DRIVERS.with(|drivers| drivers.0.borrow_mut().insert(session, Rc::clone(&work)));
    Ok(work)
}

/// `mln_render_session_service_driver_work()`: the first service on a
/// thread takes the session's driver work, and the thread keeps it until it
/// finishes, through a release of the handle.
///
/// # Safety
///
/// `out_diagnostic` is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_service_driver_work(
    session: mln_render_session,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, { service(session).or_else(|error| ended_service(session, error)) })
}

fn service(session: mln_render_session) -> Result<(), Error> {
    let work = match DRIVERS.with(|drivers| drivers.0.borrow().get(&session).cloned()) {
        Some(work) => work,
        None => take_driver_work(session)?,
    };
    // The service may run the host's callbacks, which may service sessions
    // of this thread: nothing here is borrowed meanwhile.
    let serviced = work.service();
    if work.is_finished() {
        DRIVERS.with(|drivers| drivers.0.borrow_mut().remove(&session));
        SERVICING.lock().remove(&session);
    }
    Ok(serviced?)
}

/// What a service of `session` that failed with `error` reports instead
/// when the session's driver work ended and its handle is live: nothing to
/// do for a detached session, and a lost target for an abandoned one.
fn ended_service(session: mln_render_session, error: Error) -> Result<(), Error> {
    if error.status() != MLN_STATUS_INVALID_STATE {
        return Err(error);
    }
    let Ok(object) = resolve(session) else { return Err(error) };
    if object.session.capabilities().map(|capabilities| capabilities.driver) != Ok(Driver::CallerThread) {
        return Err(error);
    }
    match object.session.snapshot().map(|snapshot| snapshot.state) {
        Ok(RenderSessionState::Detached) => Ok(()),
        Ok(RenderSessionState::Abandoned) => Err(Error::new(ErrorKind::TargetLost, "the render session was abandoned")),
        _ => Err(error),
    }
}

// MARK: - Lifecycle

/// `mln_render_session_detach()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_detach(
    session: mln_render_session,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let object = resolve(session)?;
        // SAFETY: the caller's contract.
        let pending = unsafe { PendingCompletion::new(completion) }?;
        send_then(&object.session, RenderSession::detach, Some(pending), |result| {
            (result.map_err(Error::from), Generations::default())
        })
    })
}

/// `mln_render_session_abandon()`: a session keeps its graphics objects
/// only when it is driven by the caller's thread and abandoned on another.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_abandon(
    session: mln_render_session,
    out_result: *mut mln_render_abandon_result,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_result), {
        // Checked before the abandon, which can't be undone.
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_result, &abandon_result(0)) }?;
        let kept = resolve(session)?.session.abandon()?;
        // SAFETY: the caller's contract, checked above.
        unsafe { abi::write_sized(out_result, &abandon_result(kept)) }
    })
}

/// What an abandon reports when it kept `kept` graphics objects until the
/// process exits.
pub(crate) fn abandon_result(kept: u32) -> mln_render_abandon_result {
    mln_render_abandon_result {
        size: size_of::<mln_render_abandon_result>() as u32,
        disposition: if kept == 0 {
            MLN_RENDER_ABANDON_DISPOSITION_CLEAN
        } else {
            MLN_RENDER_ABANDON_DISPOSITION_QUARANTINED
        },
        quarantined_resource_count: kept,
        reserved: 0,
    }
}

/// Abandons `session` without graphics calls, keeping its graphics objects
/// until the process exits, and returns how many it keeps.
pub(crate) fn abandon_keeping_graphics(session: mln_render_session) -> Result<u32, Error> {
    Ok(resolve(session)?.session.abandon_keeping_graphics()?)
}

/// Releases `session` in any state without graphics calls past the one in
/// progress, keeping its graphics objects until the process exits.
pub(crate) fn dispose_keeping_graphics(session: mln_render_session) -> Result<(), Error> {
    Ok(SESSIONS.remove(session)?.session.dispose_keeping_graphics()?)
}

/// `mln_render_session_release()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_release(
    session: mln_render_session,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        resolve(session)?;
        // SAFETY: the caller's contract.
        let pending = unsafe { PendingCompletion::new(completion) }?;
        // A release that the session refuses, such as while the host holds
        // one of its frames, keeps the handle.
        let object = resolve(session)?;
        let release = |object: &RenderSession| {
            let released = object.release()?;
            let _ = SESSIONS.remove(session);
            Ok(released)
        };
        send_then(&object.session, release, Some(pending), |result| {
            (result.map_err(Error::from), Generations::default())
        })
    })
}

/// `mln_render_session_dispose()`.
///
/// # Safety
///
/// `out_diagnostic` is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_render_session_dispose(
    session: mln_render_session,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        SESSIONS.remove(session)?.session.dispose()?;
        Ok(())
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use core::ffi::c_void;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::diagnostic::tests::{diagnostic, message};
    use crate::generated::types::{
        MLN_RENDER_TARGET_KIND_OFFSCREEN, MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_INVALID_STATE, mln_completion_result,
        mln_logical_size, mln_map, mln_map_snapshot, mln_offscreen_target, mln_render_target, mln_render_target_data,
        mln_runtime, mln_texture_readback_result, mln_wake,
    };
    use crate::map::tests::{create_map, options, release_map, view};
    use crate::map::{mln_map_dispose, mln_map_get_snapshot, mln_map_release, mln_map_set_style_json};
    use crate::render_target::mln_map_attach;
    use crate::runtime::mln_runtime_release;
    use crate::runtime::tests::{Waiter, channel_completion, create, release};
    use crate::texture::mln_render_session_read_texture;

    const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();

    /// A header struct of zeros but its `size`.
    pub(crate) fn sized<T: abi::Versioned>() -> T {
        // SAFETY: zeros are a valid `T` (the trait's contract).
        let mut value: T = unsafe { core::mem::zeroed() };
        // SAFETY: `size` is the first field (the trait's contract).
        unsafe { (&raw mut value).cast::<u32>().write(size_of::<T>() as u32) };
        value
    }

    pub(crate) fn attach_options() -> mln_render_session_attach_options {
        mln_render_session_attach_options { driver: MLN_RENDER_DRIVER_CORE_WORKER, ..sized() }
    }

    pub(crate) fn snapshot(session: mln_render_session) -> Result<mln_render_session_snapshot, mln_status> {
        let mut snapshot = sized::<mln_render_session_snapshot>();
        // SAFETY: a local snapshot.
        match unsafe { mln_render_session_get_snapshot(session, &raw mut snapshot, NO_DIAGNOSTIC) } {
            MLN_STATUS_OK => Ok(snapshot),
            status => Err(status),
        }
    }

    /// Drains until a result arrives.
    pub(crate) fn wait_for_result(session: mln_render_session) -> mln_render_frame_result {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let mut batch = 0;
            let mut view = sized::<mln_render_frame_batch_view>();
            // SAFETY: local outputs.
            let status = unsafe {
                mln_render_session_drain_frame_results(session, &raw mut batch, &raw mut view, NO_DIAGNOSTIC)
            };
            assert_eq!(status, MLN_STATUS_OK);
            if batch != handles::NULL {
                assert_eq!((view.result_count, view.result_size as usize), (1, size_of::<mln_render_frame_result>()));
                // SAFETY: the batch's one result, live until its release.
                let result = unsafe { *view.results };
                mln_render_frame_batch_release(batch);
                return result;
            }
            assert!(view.results.is_null());
            assert!(Instant::now() < deadline, "no frame result arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(crate) type Pixel = (mln_status, Option<(u32, u32, [u8; 4])>);

    unsafe extern "C" fn send_pixel(user_data: *mut c_void, result: *const mln_completion_result) {
        // SAFETY: the sender that `channel_completion` boxed, and the
        // library's result, with one readback when it succeeded.
        unsafe {
            let sender = &*user_data.cast::<mpsc::Sender<Pixel>>();
            let result = &*result;
            let pixel = (result.status == MLN_STATUS_OK).then(|| {
                let readback = *result.value.cast::<mln_texture_readback_result>();
                let data = core::slice::from_raw_parts(readback.data.data.cast::<u8>(), 4);
                (readback.info.width, readback.info.height, [data[0], data[1], data[2], data[3]])
            });
            let _ = sender.send((result.status, pixel));
        }
    }

    /// The status of a readback, and the size and first pixel that it read.
    pub(crate) fn read_pixel(session: mln_render_session) -> Pixel {
        let (completion, receiver) = channel_completion::<Pixel>(send_pixel);
        // SAFETY: local arguments, and a completion whose sender the library
        // releases after its call.
        assert_eq!(
            unsafe { mln_render_session_read_texture(session, &raw const completion, NO_DIAGNOSTIC) },
            MLN_STATUS_OK
        );
        receiver.recv_timeout(Duration::from_secs(30)).unwrap()
    }

    pub(crate) fn demand(session: mln_render_session, flags: u32, token: u64) -> mln_status {
        let demand = mln_frame_demand { flags, token, ..sized() };
        // SAFETY: a local demand.
        unsafe { mln_render_session_demand_frame(session, &raw const demand, NO_DIAGNOSTIC) }
    }

    /// An offscreen target of `width` by `height` logical pixels.
    pub(crate) fn offscreen_target(width: u32, height: u32) -> mln_render_target {
        mln_render_target {
            kind: MLN_RENDER_TARGET_KIND_OFFSCREEN,
            data: mln_render_target_data {
                offscreen: mln_offscreen_target { logical_size: mln_logical_size { width, height } },
            },
            ..sized()
        }
    }

    /// A runtime, a map, and an offscreen session that rendered a frame of it.
    pub(crate) fn rendered_session() -> (mln_runtime, mln_map, mln_render_session) {
        let runtime = create();
        let map = create_map(runtime, &options(1.0));
        let target = offscreen_target(16, 16);
        let mut session = 0;
        let attached = Waiter::new();
        // SAFETY: local arguments.
        let status = unsafe {
            let options = attach_options();
            mln_map_attach(
                map,
                &raw const target,
                0,
                &raw const options,
                &raw mut session,
                &raw const attached.descriptor,
                NO_DIAGNOSTIC,
            )
        };
        assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
        assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, 1), MLN_STATUS_OK);
        assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED);
        (runtime, map, session)
    }

    /// Releases `session`, and waits until it let go of its map.
    pub(crate) fn release_session(session: mln_render_session) {
        let released = Waiter::new();
        // SAFETY: a local completion.
        assert_eq!(
            unsafe { mln_render_session_release(session, &raw const released.descriptor, NO_DIAGNOSTIC) },
            MLN_STATUS_OK
        );
        assert_eq!(released.status(), MLN_STATUS_OK);
    }

    #[test]
    fn an_offscreen_session_renders_reads_back_and_releases() {
        let runtime = create();
        let map = create_map(runtime, &options(1.0));
        let styled = Waiter::new();
        let style = r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"red"}}]}"#;
        // SAFETY: local arguments.
        let status = unsafe { mln_map_set_style_json(map, view(style), &raw const styled.descriptor, NO_DIAGNOSTIC) };
        assert_eq!((status, styled.status()), (MLN_STATUS_OK, MLN_STATUS_OK));

        let target = offscreen_target(64, 32);
        let mut session = 0;
        let attached = Waiter::new();
        // SAFETY: local arguments.
        let status = unsafe {
            let options = attach_options();
            mln_map_attach(
                map,
                &raw const target,
                0,
                &raw const options,
                &raw mut session,
                &raw const attached.descriptor,
                NO_DIAGNOSTIC,
            )
        };
        assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
        let attached = snapshot(session).unwrap();
        assert_eq!(attached.state, MLN_RENDER_SESSION_STATE_ATTACHED);
        assert_eq!((attached.extent.width, attached.extent.height, attached.failure_status), (64, 32, MLN_STATUS_OK));
        assert!(attached.has_target);
        let mut capabilities = sized::<mln_render_session_capabilities>();
        // SAFETY: local capabilities.
        let status = unsafe { mln_render_session_get_capabilities(session, &raw mut capabilities, NO_DIAGNOSTIC) };
        assert_eq!((status, capabilities.flags), (MLN_STATUS_OK, MLN_RENDER_SESSION_CAPABILITY_READBACK));

        assert_eq!(read_pixel(session).0, MLN_STATUS_INVALID_STATE);
        let both = MLN_FRAME_DEMAND_FORCE | MLN_FRAME_DEMAND_WAIT_FOR_UPDATE;
        assert_eq!(demand(session, both, 1), MLN_STATUS_INVALID_ARGUMENT);
        assert_eq!(demand(session, 0, u64::MAX), MLN_STATUS_INVALID_ARGUMENT);
        assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, 7), MLN_STATUS_OK);
        let result = wait_for_result(session);
        assert_eq!((result.token, result.disposition), (7, MLN_RENDER_RESULT_RENDERED));
        assert_eq!((result.view.extent.width, result.view.extent.scale_factor), (64, 1.0));
        assert_eq!(read_pixel(session), (MLN_STATUS_OK, Some((64, 32, [255, 0, 0, 255]))));
        let mut out = diagnostic();
        // SAFETY: a local diagnostic.
        let serviced = unsafe { mln_render_session_service_driver_work(session, &raw mut *out) };
        assert_eq!(serviced, MLN_STATUS_INVALID_STATE);
        assert!(message(&out).contains("core worker"), "{}", message(&out));

        let released = Waiter::new();
        // SAFETY: a local completion.
        assert_eq!(
            unsafe { mln_render_session_release(session, &raw const released.descriptor, NO_DIAGNOSTIC) },
            MLN_STATUS_OK
        );
        assert_eq!(released.status(), MLN_STATUS_OK);
        assert_eq!(snapshot(session).unwrap_err(), MLN_STATUS_INVALID_STATE);
        release_map(map);
        release(runtime);
    }

    #[test]
    fn an_attached_session_holds_its_map_until_it_ends() {
        let (runtime, map, session) = rendered_session();
        let map_snapshot = |map| {
            // SAFETY: zeros are a valid snapshot to overwrite.
            let mut snapshot: mln_map_snapshot = unsafe { core::mem::zeroed() };
            snapshot.size = size_of::<mln_map_snapshot>() as u32;
            // SAFETY: a local snapshot.
            unsafe { mln_map_get_snapshot(map, &raw mut snapshot, NO_DIAGNOSTIC) }
        };

        let refused = Waiter::new();
        let mut out = diagnostic();
        // SAFETY: local arguments.
        let status = unsafe { mln_map_release(map, &raw const refused.descriptor, &raw mut *out) };
        assert_eq!(status, MLN_STATUS_INVALID_STATE);
        assert!(message(&out).contains("map still has an attached render session"), "{}", message(&out));
        refused.unused();
        assert_eq!(map_snapshot(map), MLN_STATUS_OK);

        // A disposed map stays with its session, which keeps rendering it,
        // and with its runtime, until the session ends.
        // SAFETY: a null diagnostic.
        assert_eq!(unsafe { mln_map_dispose(map, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        assert_eq!(map_snapshot(map), MLN_STATUS_INVALID_STATE);
        assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, 2), MLN_STATUS_OK);
        let result = wait_for_result(session);
        assert_eq!((result.token, result.disposition), (2, MLN_RENDER_RESULT_RENDERED));
        let refused = Waiter::new();
        // SAFETY: local arguments.
        let status = unsafe { mln_runtime_release(runtime, &raw const refused.descriptor, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_INVALID_STATE);
        refused.unused();

        release_session(session);
        release(runtime);
    }

    #[test]
    fn attach_options_check_their_driver_and_sync_kinds() {
        // SAFETY: each pointer is to a local.
        let check = |options: mln_render_session_attach_options| unsafe { super::attach_options(&raw const options) };
        assert!(check(attach_options()).is_ok());
        let driver = check(mln_render_session_attach_options { driver: 9, ..attach_options() });
        assert!(driver.err().unwrap().message().contains("driver"));
        let kinds = check(mln_render_session_attach_options { producer_sync_kinds: 1 << 20, ..attach_options() });
        assert_eq!(kinds.err().unwrap().status(), MLN_STATUS_INVALID_ARGUMENT);
        let queued = mln_wake { callback: None, user_data: core::ptr::null_mut(), release_user_data: None, queue: 5 };
        let wake = check(mln_render_session_attach_options { frame_wake: queued, ..attach_options() });
        assert_eq!(wake.err().unwrap().status(), MLN_STATUS_INVALID_ARGUMENT);
        // SAFETY: null.
        assert!(unsafe { super::attach_options(core::ptr::null()) }.is_err());
    }

    #[test]
    fn a_result_that_rendered_nothing_has_no_view() {
        let result = FrameResult {
            disposition: FrameDisposition::Superseded,
            token: Some(3),
            map_update_generation: 4,
            generation: 0,
            frame_generation: 0,
            needs_repaint: false,
            view: None,
            mode: ::mln_map::RenderMode::Partial,
            projection: None,
        };
        let converted = frame_result(&result, 2.0);
        let fields = (converted.disposition, converted.token, converted.map_update_generation);
        assert_eq!(fields, (MLN_RENDER_RESULT_SUPERSEDED, 3, 4));
        assert_eq!(converted.view.size, 0);
        assert_eq!(converted.frame_projection.clip_depth_range, 0);
    }
}

#[cfg(test)]
mod driver_tests {
    use core::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::tests::{attach_options, demand, offscreen_target, sized, snapshot, wait_for_result};
    use super::*;
    use crate::diagnostic::tests::{diagnostic, message};
    use crate::generated::types::{
        MLN_STATUS_INVALID_STATE, MLN_STATUS_UNSUPPORTED, MLN_STATUS_WRONG_THREAD, mln_map, mln_wake,
    };
    use crate::map::tests::{create_map, options, release_map};
    use crate::render_target::mln_map_attach;
    use crate::runtime::tests::{Waiter, create, release};

    const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();

    unsafe extern "C" fn count(user_data: *mut c_void) {
        // SAFETY: the test's counter, which outlives the session.
        unsafe { &*user_data.cast::<AtomicUsize>() }.fetch_add(1, Ordering::SeqCst);
    }

    fn service(session: mln_render_session) -> mln_status {
        // SAFETY: a null diagnostic.
        unsafe { mln_render_session_service_driver_work(session, NO_DIAGNOSTIC) }
    }

    /// Waits until `done` holds.
    fn until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done() {
            assert!(Instant::now() < deadline, "it never happened");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Services `session` until `done` holds.
    fn service_until(session: mln_render_session, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done() {
            assert_eq!(service(session), MLN_STATUS_OK);
            assert!(Instant::now() < deadline, "the session never got there");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The status and message of a service of `session` on another thread.
    fn service_elsewhere(session: mln_render_session) -> (mln_status, String) {
        std::thread::spawn(move || {
            let mut out = diagnostic();
            // SAFETY: a local diagnostic.
            let status = unsafe { mln_render_session_service_driver_work(session, &raw mut *out) };
            (status, message(&out).to_owned())
        })
        .join()
        .unwrap()
    }

    /// A caller-driven offscreen session of `map`, whose driver-work wake
    /// counts into `wakes`. The C API drives an offscreen session on a core
    /// worker only, so this attaches it as `mln_map_attach()` does past that
    /// check.
    fn attach_caller_driven(
        map: mln_map,
        wakes: &'static AtomicUsize,
    ) -> (mln_render_session, ::mln_runtime::Completion<()>) {
        let wake = mln_wake {
            callback: Some(count),
            user_data: core::ptr::from_ref(wakes).cast_mut().cast(),
            release_user_data: None,
            queue: 0,
        };
        let options = mln_render_session_attach_options {
            driver: MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD,
            driver_work_wake: wake,
            ..attach_options()
        };
        // SAFETY: local options.
        let request = unsafe { super::attach_options(&raw const options) }.unwrap();
        let target = RenderTarget::Offscreen { size: Size::new(16, 16), device: None };
        let mut attached = None;
        attach(&map::resolve(map).unwrap(), target, request, |session, completion| {
            attached = Some((session, completion));
        })
        .unwrap();
        attached.unwrap()
    }

    #[test]
    fn a_caller_driven_session_runs_on_the_thread_that_services_it() {
        static WAKES: AtomicUsize = AtomicUsize::new(0);
        let runtime = create();
        let map = create_map(runtime, &options(1.0));
        let (session, attached) = attach_caller_driven(map, &WAKES);
        // The attach waits for a service, and the host hears of it once: from
        // the attach, or from the runtime's thread if the map's first update
        // reached the session first, which can be just after the attach
        // returns.
        assert_eq!(snapshot(session).unwrap().state, MLN_RENDER_SESSION_STATE_ATTACHING);
        until(|| WAKES.load(Ordering::SeqCst) == 1);
        service_until(session, || attached.is_complete());
        assert_eq!(snapshot(session).unwrap().state, MLN_RENDER_SESSION_STATE_ATTACHED);
        let mut capabilities = sized::<mln_render_session_capabilities>();
        // SAFETY: local capabilities.
        unsafe { mln_render_session_get_capabilities(session, &raw mut capabilities, NO_DIAGNOSTIC) };
        assert_eq!(capabilities.driver, MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD);

        assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, 1), MLN_STATUS_OK);
        assert_eq!(service(session), MLN_STATUS_OK);
        assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED);

        // Another thread can't take the work over, before or after a release.
        let (status, text) = service_elsewhere(session);
        assert_eq!(status, MLN_STATUS_WRONG_THREAD);
        assert!(text.contains("first serviced"), "{text}");

        // The release detaches the session at its next service, which still
        // takes the released handle, until the release completes.
        let released = Waiter::new();
        // SAFETY: a local completion.
        let status = unsafe { mln_render_session_release(session, &raw const released.descriptor, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        assert_eq!(service_elsewhere(session).0, MLN_STATUS_WRONG_THREAD);
        assert_eq!(service(session), MLN_STATUS_OK);
        assert_eq!(released.status(), MLN_STATUS_OK);
        assert_eq!(service(session), MLN_STATUS_INVALID_STATE);
        assert_eq!(service_elsewhere(session).0, MLN_STATUS_INVALID_STATE);
        release_map(map);
        release(runtime);
    }

    #[test]
    fn a_detached_caller_driven_session_has_no_work_until_it_is_released() {
        static WAKES: AtomicUsize = AtomicUsize::new(0);
        let runtime = create();
        let map = create_map(runtime, &options(1.0));
        let (session, attached) = attach_caller_driven(map, &WAKES);
        service_until(session, || attached.is_complete());

        // The detach runs at the next service, and the services after find
        // nothing to do.
        let detached = Waiter::new();
        // SAFETY: a local completion.
        let status = unsafe { mln_render_session_detach(session, &raw const detached.descriptor, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        assert_eq!(service(session), MLN_STATUS_OK);
        assert_eq!(detached.status(), MLN_STATUS_OK);
        assert_eq!(service(session), MLN_STATUS_OK);
        assert_eq!(service(session), MLN_STATUS_OK);
        // SAFETY: a null diagnostic.
        assert_eq!(unsafe { mln_render_session_dispose(session, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        assert_eq!(service(session), MLN_STATUS_INVALID_STATE);
        release_map(map);
        release(runtime);
    }

    #[test]
    fn an_abandon_reports_the_graphics_objects_that_a_caller_driven_session_keeps() {
        static WAKES: AtomicUsize = AtomicUsize::new(0);
        let runtime = create();
        let map = create_map(runtime, &options(1.0));
        let (session, attached) = attach_caller_driven(map, &WAKES);
        service_until(session, || attached.is_complete());
        assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, 1), MLN_STATUS_OK);
        assert_eq!(service(session), MLN_STATUS_OK);
        wait_for_result(session);

        // Off the servicing thread, the session keeps its renderer and its
        // target until the process exits.
        let abandoned = std::thread::spawn(move || {
            let mut result = sized::<mln_render_abandon_result>();
            // SAFETY: a local result.
            let status = unsafe { mln_render_session_abandon(session, &raw mut result, NO_DIAGNOSTIC) };
            (status, result.disposition, result.quarantined_resource_count)
        })
        .join()
        .unwrap();
        assert_eq!(abandoned, (MLN_STATUS_OK, MLN_RENDER_ABANDON_DISPOSITION_QUARANTINED, 2));
        // The next service lets go of the session, and each reports the lost
        // target until the handle is released.
        assert_eq!(service(session), MLN_STATUS_TARGET_LOST);
        assert_eq!(service(session), MLN_STATUS_TARGET_LOST);
        // SAFETY: a null diagnostic.
        assert_eq!(unsafe { mln_render_session_dispose(session, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        release_map(map);
        release(runtime);
    }

    #[test]
    fn an_offscreen_session_refuses_the_callers_thread() {
        let runtime = create();
        let map = create_map(runtime, &options(1.0));
        let target = offscreen_target(16, 16);
        let options =
            mln_render_session_attach_options { driver: MLN_RENDER_DRIVER_CALLER_GRAPHICS_THREAD, ..attach_options() };
        let (mut session, attached) = (0, Waiter::new());
        // SAFETY: local arguments.
        let status = unsafe {
            mln_map_attach(
                map,
                &raw const target,
                0,
                &raw const options,
                &raw mut session,
                &raw const attached.descriptor,
                NO_DIAGNOSTIC,
            )
        };
        assert_eq!(status, MLN_STATUS_UNSUPPORTED);
        attached.unused();
        release_map(map);
        release(runtime);
    }
}
