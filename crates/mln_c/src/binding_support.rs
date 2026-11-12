//! `binding_support.h`: what the bindings of maplibre-native-ffi use to tie a
//! handle's life to a host runtime's garbage collector, and to wake a Dart
//! isolate.

use core::ffi::c_void;

use mln_util::logging::{Event, Log};

use crate::error::Error;
use crate::generated::types::{mln_diagnostic, mln_render_abandon_result, mln_render_session, mln_status, mln_wake};
use crate::handles::{self, HandleKind};
use crate::{
    abi, delivery, gpu_device, map, projection, render_session, resources, runtime, style_expression, transforms,
};

/// A finalizer token: the owned handle that a host's finalizer disposes of.
struct OwnerToken {
    handle: u64,
}

/// `mln_binding_owner_token_create()`. The allocation can't fail: running out
/// of memory aborts the process.
#[unsafe(no_mangle)]
pub extern "C" fn mln_binding_owner_token_create(handle: u64) -> *mut c_void {
    Box::into_raw(Box::new(OwnerToken { handle })).cast()
}

/// `mln_binding_owner_token_destroy()`.
///
/// # Safety
///
/// `token` is null, or a token from [`mln_binding_owner_token_create`] that
/// wasn't destroyed or finalized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_binding_owner_token_destroy(token: *mut c_void) {
    // SAFETY: the caller's contract.
    drop(unsafe { take(token) });
}

/// `mln_binding_owner_finalize()`.
///
/// # Safety
///
/// As for [`mln_binding_owner_token_destroy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_binding_owner_finalize(token: *mut c_void) {
    // SAFETY: the caller's contract.
    let Some(token) = (unsafe { take(token) }) else { return };
    if let Err(error) = dispose_owner(token.handle) {
        Log::error(
            Event::General,
            format!(
                "owner finalization failed for handle {} (status {}): {}",
                token.handle,
                error.status(),
                error.message(),
            ),
        );
    }
}

/// The token behind `token`, which the caller owns from here on.
///
/// # Safety
///
/// As for [`mln_binding_owner_token_destroy`].
unsafe fn take(token: *mut c_void) -> Option<Box<OwnerToken>> {
    // SAFETY: the caller's contract: a non-null token came from
    // `Box::into_raw` in `mln_binding_owner_token_create`.
    (!token.is_null()).then(|| unsafe { Box::from_raw(token.cast::<OwnerToken>()) })
}

/// Disposes of the object that `handle` owns, of whichever kind. A host's
/// finalizers can run while the process exits, after the host's graphics
/// objects are gone, so a render session keeps its graphics objects.
fn dispose_owner(handle: u64) -> Result<(), Error> {
    let Some(kind) = HandleKind::of(handle) else {
        return Err(Error::invalid_argument(format!("{handle:#x} is not a handle that owns an object")));
    };
    match kind {
        HandleKind::Runtime => runtime::dispose(handle),
        HandleKind::Map => map::dispose(handle),
        HandleKind::MapProjection => projection::release(handle),
        HandleKind::RenderSession => render_session::dispose_keeping_graphics(handle),
        HandleKind::ResourceRequest => resources::release(handle),
        HandleKind::ResourceTransformRequest => transforms::release(handle),
        HandleKind::RenderFrameBatch => render_session::release_batch(handle),
        HandleKind::DeliveryQueue => delivery::release(handle),
        HandleKind::StyleExpression => style_expression::release(handle),
        HandleKind::GpuDevice => gpu_device::release(handle),
        HandleKind::AcquiredFrame | HandleKind::FramePacer => Err(Error::invalid_argument(format!(
            "{} handle {handle:#x} was never created by this process",
            kind.name()
        ))),
    }
}

/// `mln_binding_render_session_abandon_at_exit()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_binding_render_session_abandon_at_exit(
    session: mln_render_session,
    out_result: *mut mln_render_abandon_result,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_result), {
        // Checked before the abandon, which can't be undone.
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_result, &render_session::abandon_result(0)) }?;
        let kept = render_session::abandon_keeping_graphics(session)?;
        // SAFETY: the caller's contract, which the write above checked.
        unsafe { abi::write_sized(out_result, &render_session::abandon_result(kept)) }?;
        Ok(())
    })
}

/// The integer message of Dart's native API, version 2: a `Dart_CObject` of
/// type `Dart_CObject_kInt64`. The union's size is the whole value union's,
/// whose largest member is five words.
#[repr(C)]
struct DartIntegerMessage {
    kind: i32,
    value: DartValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
union DartValue {
    as_int64: i64,
    _reserved: [usize; 5],
}

/// `Dart_CObject_kInt64`.
const DART_INT64: i32 = 3;

/// `Dart_PostCObject`: posts a message to a port, and returns whether the
/// port took it.
type DartPost = unsafe extern "C" fn(port: i64, message: *mut DartIntegerMessage) -> bool;

/// A Dart wake's user data: where it posts.
struct DartPort {
    post: DartPost,
    port: i64,
}

impl DartPort {
    fn notify(&self, value: i64) {
        let mut message = DartIntegerMessage { kind: DART_INT64, value: DartValue { as_int64: value } };
        // SAFETY: `post` is `NativeApi.postCObject`, which the creator passed
        // and which lives as long as the process, and the message is a valid
        // integer message for the call. A closed port only drops it.
        let _ = unsafe { (self.post)(self.port, &raw mut message) };
    }
}

/// The wake's callback: posts 0.
unsafe extern "C" fn dart_wake(user_data: *mut c_void) {
    // SAFETY: the wake's user data, a live `DartPort` until its release.
    unsafe { &*user_data.cast::<DartPort>() }.notify(0);
}

/// The wake's release: posts 1, then frees the user data.
unsafe extern "C" fn dart_wake_release(user_data: *mut c_void) {
    // SAFETY: the wake's user data, which `mln_binding_dart_wake_create`
    // made with `Box::into_raw` and which the library releases once.
    let port = unsafe { Box::from_raw(user_data.cast::<DartPort>()) };
    port.notify(1);
}

/// `mln_binding_dart_wake_create()`.
///
/// # Safety
///
/// `post_cobject` is null or Dart's `NativeApi.postCObject`, and each other
/// pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_binding_dart_wake_create(
    post_cobject: *mut c_void,
    port: i64,
    out_wake: *mut mln_wake,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(post_cobject, out_wake), {
        if port == 0 {
            return Err(Error::invalid_argument("port must not be zero"));
        }
        // SAFETY: the caller's contract: a non-null `post_cobject` is the
        // address of a function of `DartPost`'s signature.
        let post = unsafe { core::mem::transmute::<*mut c_void, DartPost>(post_cobject) };
        let user_data = Box::into_raw(Box::new(DartPort { post, port }));
        let wake = mln_wake {
            callback: Some(dart_wake),
            user_data: user_data.cast(),
            release_user_data: Some(dart_wake_release),
            queue: handles::NULL,
        };
        // SAFETY: the caller's contract.
        unsafe { out_wake.write(wake) };
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::diagnostic::tests::{diagnostic, message};
    use crate::generated::types::{
        MLN_RENDER_ABANDON_DISPOSITION_QUARANTINED, MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_INVALID_STATE,
        MLN_STATUS_OK,
    };
    use crate::render_session::abandon_result;

    const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();

    /// The messages that `record_post` took, by port.
    static POSTED: Mutex<Vec<(i64, i32, i64)>> = Mutex::new(Vec::new());

    unsafe extern "C" fn record_post(port: i64, message: *mut DartIntegerMessage) -> bool {
        // SAFETY: the library's integer message, live for the call.
        let message = unsafe { &*message };
        // SAFETY: an integer message's value is its `as_int64`.
        let value = unsafe { message.value.as_int64 };
        crate::lock(&POSTED).push((port, message.kind, value));
        false
    }

    #[test]
    fn a_dart_wake_posts_zero_per_wake_and_one_on_release() {
        let post = record_post as DartPost as *mut c_void;
        let mut wake = mln_wake { callback: None, user_data: core::ptr::null_mut(), release_user_data: None, queue: 0 };
        let mut out = diagnostic();
        // SAFETY: each pointer is null or to a local.
        unsafe {
            let status = mln_binding_dart_wake_create(post, 0, &raw mut wake, &raw mut *out);
            assert_eq!((status, message(&out)), (MLN_STATUS_INVALID_ARGUMENT, "port must not be zero"));
            let null = mln_binding_dart_wake_create(core::ptr::null_mut(), 41, &raw mut wake, NO_DIAGNOSTIC);
            assert_eq!(null, MLN_STATUS_INVALID_ARGUMENT);
            let no_out = mln_binding_dart_wake_create(post, 41, core::ptr::null_mut(), NO_DIAGNOSTIC);
            assert_eq!(no_out, MLN_STATUS_INVALID_ARGUMENT);

            assert_eq!(mln_binding_dart_wake_create(post, 41, &raw mut wake, NO_DIAGNOSTIC), MLN_STATUS_OK);
            assert_eq!(wake.queue, handles::NULL);
            wake.callback.unwrap()(wake.user_data);
            wake.release_user_data.unwrap()(wake.user_data);
        }
        let posted: Vec<_> = crate::lock(&POSTED).iter().copied().filter(|(port, ..)| *port == 41).collect();
        assert_eq!(posted, [(41, DART_INT64, 0), (41, DART_INT64, 1)]);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_integer_message_has_dart_s_layout() {
        assert_eq!(core::mem::offset_of!(DartIntegerMessage, value), 8);
        assert_eq!(size_of::<DartIntegerMessage>(), 48);
    }

    #[test]
    fn a_token_disposes_of_its_owner_only_when_finalized() {
        let runtime = runtime::tests::create();
        let destroyed = runtime::tests::create();
        // SAFETY: tokens from `mln_binding_owner_token_create`, each used once.
        unsafe {
            mln_binding_owner_token_destroy(mln_binding_owner_token_create(destroyed));
            mln_binding_owner_finalize(mln_binding_owner_token_create(runtime));
            mln_binding_owner_token_destroy(core::ptr::null_mut());
            mln_binding_owner_finalize(core::ptr::null_mut());
        }
        assert!(runtime::dispose(destroyed).is_ok());
        assert_eq!(runtime::dispose(runtime).unwrap_err().status(), MLN_STATUS_INVALID_STATE);
    }

    #[test]
    fn finalization_rejects_handles_that_own_nothing() {
        assert_eq!(dispose_owner(handles::NULL).unwrap_err().status(), MLN_STATUS_INVALID_ARGUMENT);
        assert_eq!(dispose_owner(0xFF00_0000_0000_0001).unwrap_err().status(), MLN_STATUS_INVALID_ARGUMENT);
        let frame = (HandleKind::AcquiredFrame as u64) << 56 | 1;
        let error = dispose_owner(frame).unwrap_err();
        assert!(error.message().contains("mln_acquired_frame"), "{error}");
    }

    #[test]
    fn abandon_at_exit_keeps_a_rendered_sessions_graphics() {
        let (runtime, map, session) = render_session::tests::rendered_session();
        let mut result = abandon_result(0);
        // SAFETY: each pointer is null or to a local.
        unsafe {
            let status = mln_binding_render_session_abandon_at_exit(session, core::ptr::null_mut(), NO_DIAGNOSTIC);
            assert_eq!(status, MLN_STATUS_INVALID_ARGUMENT);
            let status = mln_binding_render_session_abandon_at_exit(session, &raw mut result, NO_DIAGNOSTIC);
            assert_eq!(status, MLN_STATUS_OK);
        }
        assert_eq!(
            (result.disposition, result.quarantined_resource_count),
            (MLN_RENDER_ABANDON_DISPOSITION_QUARANTINED, 2)
        );
        // SAFETY: as above.
        let again = unsafe { mln_binding_render_session_abandon_at_exit(session, &raw mut result, NO_DIAGNOSTIC) };
        assert_eq!(again, MLN_STATUS_INVALID_STATE);
        render_session::tests::release_session(session);
        crate::map::tests::release_map(map);
        runtime::tests::release(runtime);
    }

    #[test]
    fn a_finalized_session_is_released_and_frees_its_map() {
        let (runtime, map, session) = render_session::tests::rendered_session();
        // SAFETY: a token from `mln_binding_owner_token_create`, used once.
        unsafe { mln_binding_owner_finalize(mln_binding_owner_token_create(session)) };
        assert_eq!(render_session::tests::snapshot(session).unwrap_err(), MLN_STATUS_INVALID_STATE);
        crate::map::tests::release_map(map);
        runtime::tests::release(runtime);
    }
}
