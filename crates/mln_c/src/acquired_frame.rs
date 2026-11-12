//! The frames of a borrowed ring that the host acquires (`render_session.h`),
//! and their textures (`texture.h`).

use core::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use ::mln_runtime::{
    AcquiredFrame, ConsumerSync, ExportedImage, FrameTexture, FrameTextureKind, FrameView, GpuSync, SyncKind,
};
use mln_util::ErrorKind;
use mln_util::sync::MutexExt;

use crate::error::Error;
use crate::generated::types::{
    mln_acquired_frame, mln_android_hardware_buffer_frame, mln_diagnostic, mln_dma_buf_frame, mln_dma_buf_plane,
    mln_dxgi_shared_texture_frame, mln_gpu_sync, mln_io_surface_frame, mln_metal_texture_frame, mln_opaque_fd_frame,
    mln_open_gl_texture_frame, mln_render_frame_result, mln_render_session, mln_status, mln_vulkan_texture_frame,
    mln_web_gpu_texture_frame,
};
use crate::handles::{self, HandleKind, HandleTable};
use crate::{abi, render_session};

abi::versioned!(
    mln_render_frame_result,
    mln_gpu_sync,
    mln_vulkan_texture_frame,
    mln_metal_texture_frame,
    mln_open_gl_texture_frame,
    mln_web_gpu_texture_frame,
    mln_io_surface_frame,
    mln_dma_buf_frame,
    mln_android_hardware_buffer_frame,
    mln_dxgi_shared_texture_frame,
    mln_opaque_fd_frame
);

/// An acquired frame, until the host releases or discards it.
struct FrameObject {
    frame: Mutex<Held>,
    /// The views that keep the frame in place.
    views: AtomicU32,
    /// The scale factor that the session's map was created with, which the
    /// frame's view reports exactly.
    initial_scale_factor: f64,
    /// The planes of an exported dmabuf, which `mln_dma_buf_frame.planes`
    /// points into while the frame lives.
    dma_buf_planes: Vec<mln_dma_buf_plane>,
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
        let dma_buf_planes = match frame.texture().map(|texture| &texture.kind) {
            Some(FrameTextureKind::Exported(ExportedImage::DmaBuf { planes, .. })) => planes
                .iter()
                .map(|plane| mln_dma_buf_plane { fd: plane.fd, offset: plane.offset, stride: plane.stride })
                .collect(),
            _ => Vec::new(),
        };
        let frame = FrameObject {
            frame: Mutex::new(Held { frame: Some(frame), discarded: None }),
            views: AtomicU32::new(0),
            initial_scale_factor: object.initial_scale_factor,
            dma_buf_planes,
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

/// The descriptor of a sync file that `handle` names, an `int` widened with
/// or without its sign: `None` for -1, as Vulkan exports for a semaphore
/// that is already signaled.
///
/// Fails with [`ErrorKind::InvalidArgument`] for another negative value, or
/// one that is no `int`.
fn sync_file_descriptor(handle: u64) -> Result<Option<i32>, Error> {
    let fd = i32::try_from(handle as i64).ok().or_else(|| (handle == u64::from(u32::MAX)).then_some(-1));
    match fd {
        Some(-1) => Ok(None),
        Some(fd) if fd >= 0 => Ok(Some(fd)),
        _ => Err(Error::invalid_argument("a sync file's handle must be a file descriptor, or -1")),
    }
}

/// The kind of the host's consumer `sync`, a known one, and a sync file of
/// -1 as a finished read.
fn consumer_kind(sync: GpuSync) -> Result<GpuSync, Error> {
    if sync.kind == SyncKind::SyncFd && sync_file_descriptor(sync.handle)?.is_none() {
        return Ok(GpuSync::of(SyncKind::CpuComplete));
    }
    Ok(sync)
}

/// The host's consumer sync, which takes the sync file it names.
///
/// # Safety
///
/// A sync file's descriptor, which [`consumer_kind`] checked, is the host's
/// to hand over.
unsafe fn consumer_sync(sync: GpuSync) -> ConsumerSync {
    #[cfg(unix)]
    if sync.kind == SyncKind::SyncFd {
        use std::os::fd::FromRawFd as _;
        let file = sync_file_descriptor(sync.handle)
            .ok()
            .flatten()
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
        // A sync file of -1 is a finished read, which every session takes.
        let consumer =
            kind.map(|kind| consumer_kind(GpuSync { kind, handle: sync.handle, value: sync.value })).transpose()?;
        let object = FRAMES.get(frame)?;
        let mut held = object.frame.lock_unpoisoned();
        if object.views.load(Ordering::Acquire) != 0 {
            return Err(Error::new(ErrorKind::Busy, "acquired frame has active borrowed views"));
        }
        let acquired = held.as_ref().ok_or_else(|| Error::invalid_state("acquired frame was already released"))?;
        let Some(consumer) = consumer.filter(|consumer| acquired.accepts(consumer.kind)) else {
            return Err(unsupported("render backend does not support this gpu sync kind"));
        };
        let acquired = held.take().ok_or_else(|| Error::invalid_state("acquired frame was already released"))?;
        drop(held);
        let _ = FRAMES.remove(frame);
        // SAFETY: the host hands over a sync file with a release that
        // succeeds, which this is from here on.
        let consumer = unsafe { consumer_sync(consumer) };
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

/// A view's scope: the frame it holds in place, and the session's count of
/// it.
struct ViewScope(Arc<FrameObject>, FrameView);

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
        let view = {
            let held = object.frame.lock_unpoisoned();
            let acquired = held.as_ref().ok_or_else(|| Error::invalid_state("acquired frame was already released"))?;
            // The session counts the view, and checks that it can still
            // render, in one step with an abandon's check of the count.
            let view = acquired.view()?;
            object.views.fetch_add(1, Ordering::AcqRel);
            view
        };
        let scope = Box::into_raw(Box::new(ViewScope(object, view)));
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
    let ViewScope(object, view) = *unsafe { Box::from_raw(scope.cast::<ViewScope>()) };
    let mut held = object.frame.lock_unpoisoned();
    drop(view);
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

/// The exported image of the frame `frame` names, the scale factor it
/// rendered at, its slot, and the planes of a dmabuf, which stay in place
/// while the frame lives.
fn exported_image(frame: mln_acquired_frame) -> Result<(FrameTexture, f64, u32, *const mln_dma_buf_plane), Error> {
    let object = FRAMES.get(frame)?;
    let (texture, scale_factor, slot) = frame_texture(frame)?;
    if !matches!(texture.kind, FrameTextureKind::Exported(_)) {
        return Err(unsupported("the frame's texture isn't of an exported ring"));
    }
    Ok((texture, scale_factor, slot, object.dma_buf_planes.as_ptr()))
}

fn other_export() -> Error {
    unsupported("the frame's ring shares its textures in another way")
}

/// `mln_acquired_frame_get_io_surface()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_io_surface(
    frame: mln_acquired_frame,
    out_frame: *mut mln_io_surface_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_frame) }?;
        let (texture, scale_factor, slot, _) = exported_image(frame)?;
        let FrameTextureKind::Exported(ExportedImage::IoSurface { surface }) = texture.kind else {
            return Err(other_export());
        };
        let record = mln_io_surface_frame {
            size: size_of::<mln_io_surface_frame>() as u32,
            width: texture.size.width,
            height: texture.size.height,
            scale_factor,
            slot,
            surface: surface as usize as *mut c_void,
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_frame, &record) }
    })
}

/// `mln_acquired_frame_get_dma_buf()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_dma_buf(
    frame: mln_acquired_frame,
    out_frame: *mut mln_dma_buf_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_frame) }?;
        let (texture, scale_factor, slot, planes) = exported_image(frame)?;
        let FrameTextureKind::Exported(ExportedImage::DmaBuf { fourcc, modifier, planes: listed }) = texture.kind
        else {
            return Err(other_export());
        };
        let record = mln_dma_buf_frame {
            size: size_of::<mln_dma_buf_frame>() as u32,
            width: texture.size.width,
            height: texture.size.height,
            scale_factor,
            slot,
            fourcc,
            modifier,
            planes,
            plane_count: listed.len(),
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_frame, &record) }
    })
}

/// `mln_acquired_frame_get_opaque_fd()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_opaque_fd(
    frame: mln_acquired_frame,
    out_frame: *mut mln_opaque_fd_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_frame) }?;
        let (texture, scale_factor, slot, _) = exported_image(frame)?;
        let FrameTextureKind::Exported(ExportedImage::OpaqueFd { fd, allocation_size, dedicated, format }) =
            texture.kind
        else {
            return Err(other_export());
        };
        let record = mln_opaque_fd_frame {
            size: size_of::<mln_opaque_fd_frame>() as u32,
            width: texture.size.width,
            height: texture.size.height,
            scale_factor,
            slot,
            fd,
            allocation_size,
            dedicated,
            format,
        };
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_frame, &record) }
    })
}

/// `mln_acquired_frame_get_android_hardware_buffer()`: no ring of this
/// library shares hardware buffers yet.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_android_hardware_buffer(
    frame: mln_acquired_frame,
    out_frame: *mut mln_android_hardware_buffer_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_frame) }?;
        exported_image(frame)?;
        Err(other_export())
    })
}

/// `mln_acquired_frame_get_dxgi_shared_texture()`: no ring of this library
/// shares DXGI textures yet.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_acquired_frame_get_dxgi_shared_texture(
    frame: mln_acquired_frame,
    out_frame: *mut mln_dxgi_shared_texture_frame,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_frame), {
        // SAFETY: the caller's contract.
        unsafe { abi::check_out_size(out_frame) }?;
        exported_image(frame)?;
        Err(other_export())
    })
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::fd::{IntoRawFd as _, OwnedFd};

    use super::*;

    fn sync_file(handle: u64) -> GpuSync {
        GpuSync { kind: SyncKind::SyncFd, handle, value: 0 }
    }

    #[test]
    fn a_sync_file_of_minus_one_is_a_finished_read() {
        // -1, widened with and without its sign.
        for handle in [u64::MAX, u64::from(u32::MAX)] {
            assert_eq!(GpuSync::of(SyncKind::CpuComplete), consumer_kind(sync_file(handle)).unwrap());
        }
        // Another negative value, or one that is no `int`.
        for handle in [(-2i64) as u64, 0x8000_0000, u64::from(u32::MAX) - 1, 1 << 40] {
            assert!(consumer_kind(sync_file(handle)).is_err(), "{handle:#x}");
        }
        let (reader, _writer) = std::io::pipe().unwrap();
        let fd = OwnedFd::from(reader).into_raw_fd();
        let sync = consumer_kind(sync_file(fd as u64)).unwrap();
        assert_eq!(sync_file(fd as u64), sync);
        // SAFETY: the descriptor is the test's to hand over.
        let consumer = unsafe { consumer_sync(sync) };
        assert_eq!(Some(fd), consumer.sync_file.as_ref().map(std::os::fd::AsRawFd::as_raw_fd));
    }
}
