//! The `mln_diagnostic` out-parameter that functions write their failure
//! message to.

use crate::generated::types::{MLN_DIAGNOSTIC_MESSAGE_CAPACITY, MLN_STATUS_UNSUPPORTED, mln_diagnostic, mln_status};

/// Writes `message` to `diagnostic`, NUL-terminated, within the `size` it
/// declares: truncated at a UTF-8 character boundary when it doesn't fit, and
/// nothing when `diagnostic` is null or has no room for a message.
///
/// # Safety
///
/// `diagnostic` is null, or points to `size` writable bytes that start with an
/// initialized `size` field.
pub(crate) unsafe fn write(diagnostic: *mut mln_diagnostic, message: &str) {
    if diagnostic.is_null() {
        return;
    }
    let offset = core::mem::offset_of!(mln_diagnostic, message);
    // SAFETY: the caller's contract: `size` is initialized.
    let size = unsafe { (*diagnostic).size } as usize;
    if size <= offset {
        return;
    }
    let capacity = (size - offset).min(MLN_DIAGNOSTIC_MESSAGE_CAPACITY as usize);
    let mut length = message.len().min(capacity - 1);
    while !message.is_char_boundary(length) {
        length -= 1;
    }
    // SAFETY: `length + 1 <= capacity` bytes past `message`'s offset, which are
    // within both the caller's `size` and the struct.
    unsafe {
        let out = core::ptr::addr_of_mut!((*diagnostic).message).cast::<u8>();
        core::ptr::copy_nonoverlapping(message.as_ptr(), out, length);
        *out.add(length) = 0;
    }
}

/// The failure of a function this library doesn't implement yet: writes so to
/// `diagnostic` and returns `MLN_STATUS_UNSUPPORTED`.
///
/// # Safety
///
/// As for [`write`].
pub(crate) unsafe fn unsupported(diagnostic: *mut mln_diagnostic, function: &str) -> mln_status {
    // SAFETY: the caller's contract.
    unsafe { write(diagnostic, &format!("{function} is not implemented")) };
    MLN_STATUS_UNSUPPORTED
}
