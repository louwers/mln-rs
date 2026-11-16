//! Releases the hardware buffers of video frames. Android 8.0 (API level 26)
//! added `AHardwareBuffer` to libnativewindow, so its function is looked up at
//! run time, and the library still loads on older versions, where no frame
//! holds a buffer.

#![expect(unsafe_code, reason = "libnativewindow is the FFI here")]

use std::ffi::c_void;
use std::sync::OnceLock;

type Release = unsafe extern "C" fn(*mut c_void);

/// `AHardwareBuffer_release`, or `None` before API level 26. The library
/// stays loaded for the rest of the process.
fn release() -> Option<Release> {
    static RELEASE: OnceLock<Option<Release>> = OnceLock::new();
    *RELEASE.get_or_init(|| {
        // SAFETY: libnativewindow is a system library, whose initializers
        // any process may run, and the symbol has this signature.
        unsafe {
            let library = libloading::Library::new("libnativewindow.so").ok()?;
            let release: Release = *library.get(b"AHardwareBuffer_release\0").ok()?;
            std::mem::forget(library);
            Some(release)
        }
    })
}

/// Releases a reference on `buffer`, an `AHardwareBuffer*`.
pub(super) fn release_hardware_buffer(buffer: *mut c_void) {
    if buffer.is_null() {
        return;
    }
    if let Some(release) = release() {
        // SAFETY: the caller's reference on a live buffer.
        unsafe { release(buffer) };
    }
}
