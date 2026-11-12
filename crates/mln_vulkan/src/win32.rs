// What Vulkan shares with D3D on Windows: the NT handles of D3D12 fences.

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// An NT handle, which dropping closes.
pub(crate) struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Takes `handle`, an NT handle that the caller owns.
    ///
    /// # Safety
    ///
    /// Nothing else closes `handle`.
    pub(crate) unsafe fn from_raw(handle: isize) -> OwnedHandle {
        OwnedHandle(HANDLE(handle as *mut c_void))
    }

    /// The handle, as an integer, as Vulkan takes it.
    pub(crate) fn as_raw(&self) -> isize {
        self.0.0 as isize
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is this one's to close.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

// SAFETY: an NT handle may be used and closed from any thread.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; nothing here changes the handle.
unsafe impl Sync for OwnedHandle {}
