//! The windows that window targets present to.

pub use raw_window_handle;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// A native window, by its handle and its display's: what a backend makes
/// the surface that a window target presents to with. The handles must stay
/// valid while a target uses them.
pub trait Window: HasWindowHandle + HasDisplayHandle + Send + Sync {}

impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync + ?Sized> Window for T {}

/// Whether `window` has an area to present to, as far as its window system
/// says: a Win32 window doesn't while it is minimized or its client area is
/// empty. Other window systems give a client no such state, so their windows
/// always have one, and the extent of a window's surface, where the graphics
/// API reports it, says the rest.
#[must_use]
pub fn has_area(window: &dyn Window) -> bool {
    match window.window_handle().map(|handle| handle.as_raw()) {
        #[cfg(windows)]
        Ok(raw_window_handle::RawWindowHandle::Win32(handle)) => win32::has_area(handle.hwnd.get()),
        _ => true,
    }
}

// FFI to user32's queries of a window.
#[cfg(windows)]
#[allow(unsafe_code)]
mod win32 {
    /// `RECT`.
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn IsIconic(window: isize) -> i32;
        fn GetClientRect(window: isize, rect: *mut Rect) -> i32;
    }

    /// Whether `window` is neither minimized nor of an empty client area. A
    /// window that the queries fail for, such as a destroyed one, has none.
    pub(super) fn has_area(window: isize) -> bool {
        let mut rect = Rect::default();
        // SAFETY: plain queries of a window, which fail for a handle that
        // names none, into a local `RECT`.
        let (iconic, queried) = unsafe { (IsIconic(window), GetClientRect(window, &raw mut rect)) };
        iconic == 0 && queried != 0 && rect.right > rect.left && rect.bottom > rect.top
    }
}
