//! Unreferences the native buffers of video frames. `libnative_buffer` is
//! looked up at run time, as the player that makes such frames looks up its
//! libraries.

#![expect(unsafe_code, reason = "libnative_buffer is the FFI here")]

use std::ffi::c_void;
use std::sync::OnceLock;

type Unreference = unsafe extern "C" fn(*mut c_void) -> i32;

/// `OH_NativeBuffer_Unreference`, or `None` without `libnative_buffer`. The
/// library stays loaded for the rest of the process.
fn unreference() -> Option<Unreference> {
    static UNREFERENCE: OnceLock<Option<Unreference>> = OnceLock::new();
    *UNREFERENCE.get_or_init(|| {
        // SAFETY: libnative_buffer is a system library, whose initializers
        // any process may run, and the symbol has this signature.
        unsafe {
            let library = libloading::Library::new("libnative_buffer.so").ok()?;
            let unreference: Unreference = *library.get(b"OH_NativeBuffer_Unreference\0").ok()?;
            std::mem::forget(library);
            Some(unreference)
        }
    })
}

/// Lets go of a reference on `buffer`, an `OH_NativeBuffer*`.
pub(super) fn unreference_native_buffer(buffer: *mut c_void) {
    if buffer.is_null() {
        return;
    }
    if let Some(unreference) = unreference() {
        // SAFETY: the caller's reference on a live buffer.
        unsafe { unreference(buffer) };
    }
}
