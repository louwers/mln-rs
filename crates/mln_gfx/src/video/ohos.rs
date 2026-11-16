//! Unreferences the native buffers of video frames.

#![expect(unsafe_code, reason = "libnative_buffer is the FFI here")]

use std::ffi::c_void;

use ohos_window_sys::native_buffer::native_buffer::OH_NativeBuffer_Unreference;

/// Lets go of a reference on `buffer`, an `OH_NativeBuffer*`.
pub(super) fn unreference_native_buffer(buffer: *mut c_void) {
    if buffer.is_null() {
        return;
    }
    // SAFETY: the caller's reference on a live buffer.
    unsafe { OH_NativeBuffer_Unreference(buffer.cast()) };
}
