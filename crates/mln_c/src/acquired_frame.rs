//! The frames of a borrowed ring that the host acquires (`render_session.h`),
//! and their textures (`texture.h`).

use core::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use ::mln_runtime::{AcquiredFrame, ConsumerSync, FrameTexture, FrameTextureKind, GpuSync, SyncKind};
use mln_util::ErrorKind;
use mln_util::sync::MutexExt;

use crate::error::Error;
use crate::generated::types::{
    mln_acquired_frame, mln_diagnostic, mln_gpu_sync, mln_metal_texture_frame, mln_open_gl_texture_frame,
    mln_render_frame_result, mln_render_session, mln_status, mln_vulkan_texture_frame, mln_web_gpu_texture_frame,
};
use crate::handles::{self, HandleKind, HandleTable};
use crate::{abi, render_session};

abi::versioned!(
    mln_render_frame_result,
    mln_gpu_sync,
    mln_vulkan_texture_frame,
    mln_metal_texture_frame,
    mln_open_gl_texture_frame,
    mln_web_gpu_texture_frame
);

/// An acquired frame, until the host releases or discards it.
struct FrameObject {
    frame: Mutex<Held>,
    /// The views that keep the frame in place.
    views: AtomicU32,
    /// How many views are open on the session's frames.
    session_views: Arc<Mutex<u32>>,
    /// The scale factor that the session's map was created with, which the
    /// frame's view reports exactly.
    initial_scale_factor: f64,
}

/// What a frame's handle holds.
#[derive(Default)]
struct Held {
    /// The frame, until the host releases or discards it.
    frame: Option<AcquiredFrame>,
    /// A frame that the host discarded while a view was open on it, which
    /// the session takes back once the last view ends.
    discarded: Option<AcquiredFrame>,
}

impl Held {
    fn as_ref(&self) -> Option<&AcquiredFrame> {
        self.frame.as_ref()
    }

    fn take(&mut self) -> Option<AcquiredFrame> {
        self.frame.take()
    }
}

static FRAMES: HandleTable<FrameObject> = HandleTable::new(HandleKind::AcquiredFrame);

fn unsupported(message: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::new(ErrorKind::Unsupported, message)
}

fn target_lost() -> Error {
    Error::new(ErrorKind::TargetLost, "the render session failed, or was detached, abandoned, or disposed")
}

/// Runs `read` on the frame `frame` names.
fn with_frame<R>(
    frame: mln_acquired_frame,
    read: impl FnOnce(&FrameObject, &AcquiredFrame) -> Result<R, Error>,
) -> Result<R, Error> {
    let object = FRAMES.get(frame)?;
    let held = object.frame.lock_unpoisoned();
    let acquired = held.as_ref().ok_or_else(|| Error::invalid_state("acquired frame was already released"))?;
    read(&object, acquired)
}

/// Runs `read` on the frame `frame` names, while its session can still
/// render: the frame's objects are valid until then.
fn with_live_frame<R>(
    frame: mln_acquired_frame,
    read: impl FnOnce(&FrameObject, &AcquiredFrame) -> Result<R, Error>,
) -> Result<R, Error> {
    with_frame(frame, |object, acquired| {
        if !acquired.session_live() {
            return Err(target_lost());
        }
        read(object, acquired)
    })
}

/// `mln_render_session_acquire_frame()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_render_session_acquire_frame(
    session: mln_render_session,
    out_frame: *mut mln_acquired_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        let out_frame = unsafe { handles::out_handle(out_frame, "out_frame") }?;
        let object = render_session::resolve(session)?;
        let Some(frame) = object.session.acquire_frame()? else {
            return Ok(());
        };
        let frame = FrameObject {
            frame: Mutex::new(Held { frame: Some(frame), discarded: None }),
            views: AtomicU32::new(0),
            session_views: object.views.clone(),
            initial_scale_factor: object.initial_scale_factor,
        };
        // A frame that gets no handle drops, which gives it back.
        *out_frame = FRAMES.insert(Arc::new(frame))?;
        Ok(())
    })
}

/// `mln_acquired_frame_get_result()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_result(
    frame: mln_acquired_frame,
    out_result: *mut mln_render_frame_result,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_result), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_result) }?;
        let result = with_frame(frame, |object, acquired| {
            Ok(render_session::frame_result(acquired.result(), object.initial_scale_factor))
        })?;
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_result, &result) }
    })
}

/// `mln_acquired_frame_get_producer_sync()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_producer_sync(
    frame: mln_acquired_frame,
    out_sync: *mut mln_gpu_sync,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_sync), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_sync) }?;
        let sync = with_live_frame(frame, |_, acquired| Ok(acquired.producer_sync()))?;
        let sync = mln_gpu_sync {
            size: size_of::<mln_gpu_sync>() as u32,
            kind: crate::gpu_device::sync_kind_value(sync.kind),
            handle: sync.handle,
            value: sync.value,
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_sync, &sync) }
    })
}

/// The host's consumer sync, which takes the sync file it names.
///
/// # Safety
///
/// A sync file's descriptor is the host's to hand over.
unsafe fn consumer_sync(sync: GpuSync) -> ConsumerSync {
    #[cfg(unix)]
    if sync.kind == SyncKind::SyncFd {
        use std::os::fd::FromRawFd as _;
        let file = i32::try_from(sync.handle)
            .ok()
            .filter(|fd| *fd >= 0)
            // SAFETY: the caller's contract.
            .map(|fd| unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) });
        return ConsumerSync { sync, sync_file: file };
    }
    ConsumerSync::of(sync)
}

/// `mln_acquired_frame_release()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_release(
    frame: mln_acquired_frame,
    consumer_completion: *const mln_gpu_sync,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        if frame == handles::NULL {
            return Err(Error::invalid_argument("frame must be a live acquired frame handle"));
        }
        if consumer_completion.is_null() {
            return Err(Error::invalid_argument("consumer_completion must not be null"));
        }
        // SAFETY: the caller's contract.
        let sync = unsafe { abi::read_sized(consumer_completion) }?;
        let kind = crate::gpu_device::sync_kind_of(sync.kind);
        if kind.is_some_and(|kind| kind.names_object() && kind != SyncKind::SyncFd) && sync.handle == 0 {
            return Err(Error::invalid_argument("consumer synchronization must have a nonzero handle"));
        }
        let object = FRAMES.get(frame)?;
        let mut held = object.frame.lock_unpoisoned();
        if object.views.load(Ordering::Acquire) != 0 {
            return Err(Error::new(ErrorKind::Busy, "acquired frame has active borrowed views"));
        }
        let acquired = held.as_ref().ok_or_else(|| Error::invalid_state("acquired frame was already released"))?;
        let Some(kind) = kind.filter(|kind| acquired.accepts(*kind)) else {
            return Err(unsupported("render backend does not support this gpu sync kind"));
        };
        let acquired = held.take().ok_or_else(|| Error::invalid_state("acquired frame was already released"))?;
        drop(held);
        let _ = FRAMES.remove(frame);
        // SAFETY: the host hands over a sync file with a release that
        // succeeds, which this is from here on.
        let consumer = unsafe { consumer_sync(GpuSync { kind, handle: sync.handle, value: sync.value }) };
        acquired.release(consumer).map_err(|(_, _, error)| Error::from(error))
    })
}

/// `mln_acquired_frame_discard()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_discard(
    frame: mln_acquired_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let object = FRAMES.get(frame)?;
        let mut held = object.frame.lock_unpoisoned();
        let acquired = held.take().ok_or_else(|| Error::invalid_state("acquired frame was already released"))?;
        let _ = FRAMES.remove(frame);
        // A view keeps the frame's sync objects in place until it ends.
        if object.views.load(Ordering::Acquire) != 0 {
            held.discarded = Some(acquired);
            return Ok(());
        }
        drop(held);
        acquired.discard();
        Ok(())
    })
}

/// A view's scope: the frame it holds in place.
struct ViewScope(Arc<FrameObject>);

/// `mln_acquired_frame_view_begin()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_view_begin(
    frame: mln_acquired_frame,
    out_scope: *mut *mut c_void,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        if out_scope.is_null() {
            return Err(Error::invalid_argument("out_scope must not be null"));
        }
        let object = FRAMES.get(frame)?;
        {
            let held = object.frame.lock_unpoisoned();
            let acquired = held.as_ref().ok_or_else(|| Error::invalid_state("acquired frame was already released"))?;
            // No abandon runs while the session's views are counted.
            let mut session_views = object.session_views.lock_unpoisoned();
            if !acquired.session_live() {
                return Err(target_lost());
            }
            *session_views += 1;
            object.views.fetch_add(1, Ordering::AcqRel);
        }
        let scope = Box::into_raw(Box::new(ViewScope(object)));
        // SAFETY: non-null, and the caller's contract.
        unsafe { out_scope.write(scope.cast()) };
        Ok(())
    })
}

/// `mln_acquired_frame_view_end()`.
///
/// # Safety
///
/// `scope` is null or a scope that `mln_acquired_frame_view_begin()` gave
/// and no call ended yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_view_end(scope: *mut c_void) {
    if scope.is_null() {
        return;
    }
    // SAFETY: the caller's contract: a scope of `view_begin`, ended once.
    let scope = unsafe { Box::from_raw(scope.cast::<ViewScope>()) };
    let object = &scope.0;
    let mut held = object.frame.lock_unpoisoned();
    {
        let mut session_views = object.session_views.lock_unpoisoned();
        *session_views = session_views.saturating_sub(1);
    }
    let discarded = (object.views.fetch_sub(1, Ordering::AcqRel) == 1).then(|| held.discarded.take()).flatten();
    drop(held);
    if let Some(frame) = discarded {
        frame.discard();
    }
}

/// The texture of the frame `frame` names, the scale factor it rendered at,
/// and its slot.
fn frame_texture(frame: mln_acquired_frame) -> Result<(FrameTexture, f64, u32), Error> {
    with_live_frame(frame, |object, acquired| {
        let texture = acquired.texture().cloned().ok_or_else(|| unsupported("the frame has no texture"))?;
        let scale_factor = acquired.result().view.as_ref().map_or(object.initial_scale_factor, |view| {
            crate::map::exact_scale_factor(object.initial_scale_factor, view.extent.scale_factor)
        });
        Ok((texture, scale_factor, acquired.slot() as u32))
    })
}

fn different_backend() -> Error {
    unsupported("the frame's texture is of a different render backend")
}

/// `mln_acquired_frame_get_vulkan_texture()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_vulkan_texture(
    frame: mln_acquired_frame,
    out_frame: *mut mln_vulkan_texture_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_frame) }?;
        let (texture, scale_factor, slot) = frame_texture(frame)?;
        #[allow(unreachable_patterns)]
        let record = match texture.kind {
            FrameTextureKind::Vulkan { image, view, device, format, layout } => mln_vulkan_texture_frame {
                size: size_of::<mln_vulkan_texture_frame>() as u32,
                width: texture.size.width,
                height: texture.size.height,
                scale_factor,
                slot,
                image,
                image_view: view,
                device: device as *mut c_void,
                format: format as u32,
                layout: layout as u32,
            },
            _ => return Err(different_backend()),
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_frame, &record) }
    })
}

/// Fails as a getter of a backend's texture does for a frame of another
/// backend.
fn other_backend<T: abi::Versioned>(frame: mln_acquired_frame, out_frame: *mut T) -> Result<(), Error> {
    // SAFETY: the caller's contract.
    unsafe { abi::check_out_size(out_frame) }?;
    frame_texture(frame)?;
    Err(different_backend())
}

/// `mln_acquired_frame_get_metal_texture()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_metal_texture(
    frame: mln_acquired_frame,
    out_frame: *mut mln_metal_texture_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_frame) }?;
        let (texture, scale_factor, slot) = frame_texture(frame)?;
        #[allow(unreachable_patterns)]
        let record = match texture.kind {
            FrameTextureKind::Metal { texture: object, device, pixel_format } => mln_metal_texture_frame {
                size: size_of::<mln_metal_texture_frame>() as u32,
                width: texture.size.width,
                height: texture.size.height,
                scale_factor,
                slot,
                texture: object as usize as *mut c_void,
                device: device as usize as *mut c_void,
                pixel_format,
            },
            _ => return Err(different_backend()),
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_frame, &record) }
    })
}

/// `mln_acquired_frame_get_open_gl_texture()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_open_gl_texture(
    frame: mln_acquired_frame,
    out_frame: *mut mln_open_gl_texture_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_frame) }?;
        let (texture, scale_factor, slot) = frame_texture(frame)?;
        #[allow(unreachable_patterns)]
        let record = match texture.kind {
            FrameTextureKind::OpenGl { texture: name, target } => mln_open_gl_texture_frame {
                size: size_of::<mln_open_gl_texture_frame>() as u32,
                width: texture.size.width,
                height: texture.size.height,
                scale_factor,
                slot,
                texture: name,
                target,
            },
            _ => return Err(different_backend()),
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_frame, &record) }
    })
}

/// `mln_acquired_frame_get_web_gpu_texture()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_web_gpu_texture(
    frame: mln_acquired_frame,
    out_frame: *mut mln_web_gpu_texture_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), { other_backend(frame, out_frame) })
}
