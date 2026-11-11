//! MapLibre Native's plugin registration (`mln/plugin/plugin_api.h`), which the
//! library exports beside its own functions.

use crate::generated::types::{MLN_PLUGIN_STATUS_UNSUPPORTED_ABI, mln_plugin_descriptor_v1, mln_plugin_status};

/// `mln_plugin_register_v1()`. Registers nothing: this library takes no
/// plugin ABI version yet. It writes why to `error_message`, NUL-terminated
/// within `error_message_capacity` bytes, when that is non-null with room.
///
/// # Safety
///
/// `error_message` is null or points to `error_message_capacity` writable
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_plugin_register_v1(
    _descriptor: *const mln_plugin_descriptor_v1,
    error_message: *mut core::ffi::c_char,
    error_message_capacity: usize,
) -> mln_plugin_status {
    const MESSAGE: &[u8] = b"plugins are not implemented";
    if !error_message.is_null() && error_message_capacity > 0 {
        let length = MESSAGE.len().min(error_message_capacity - 1);
        // SAFETY: `length + 1 <= error_message_capacity` bytes, which the
        // caller's contract makes writable.
        unsafe {
            core::ptr::copy_nonoverlapping(MESSAGE.as_ptr(), error_message.cast::<u8>(), length);
            *error_message.add(length) = 0;
        }
    }
    MLN_PLUGIN_STATUS_UNSUPPORTED_ABI
}
