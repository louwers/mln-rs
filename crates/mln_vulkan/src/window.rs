// A window's Vulkan surface, made from its raw-window-handle handles.

use std::ffi::CStr;
use std::sync::Arc;

use ash::vk;
use mln_gfx::Window;
use mln_gfx::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use mln_util::error::Error;

use crate::renderer_backend::SurfaceProvider;

/// The instance extension of the surfaces of windows on `display`.
fn platform_extension(display: RawDisplayHandle) -> Result<&'static CStr, Error> {
    match display {
        RawDisplayHandle::Wayland(_) => Ok(ash::khr::wayland_surface::NAME),
        RawDisplayHandle::Xlib(_) => Ok(ash::khr::xlib_surface::NAME),
        RawDisplayHandle::Xcb(_) => Ok(ash::khr::xcb_surface::NAME),
        RawDisplayHandle::Windows(_) => Ok(ash::khr::win32_surface::NAME),
        RawDisplayHandle::Android(_) => Ok(ash::khr::android_surface::NAME),
        other => Err(Error::unsupported(format!("No Vulkan surface for this display: {other:?}"))),
    }
}

/// A window as a surface provider: its surfaces are made from its handles.
pub(crate) struct WindowSurface {
    window: Arc<dyn Window>,
    extensions: Vec<&'static CStr>,
}

impl WindowSurface {
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// window of a kind Vulkan has no surface for, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) for a window without
    /// handles.
    pub(crate) fn new(window: Arc<dyn Window>) -> Result<WindowSurface, Error> {
        let display = window.display_handle().map_err(|e| Error::native(format!("The window has no display: {e}")))?;
        let extensions = vec![ash::khr::surface::NAME, platform_extension(display.as_raw())?];
        Ok(WindowSurface { window, extensions })
    }
}

impl SurfaceProvider for WindowSurface {
    fn instance_extensions(&self) -> Vec<&'static CStr> {
        self.extensions.clone()
    }

    fn create_surface(&self, entry: &ash::Entry, instance: &ash::Instance) -> Result<vk::SurfaceKHR, Error> {
        let window = &self.window;
        let display = window.display_handle().map_err(|e| Error::native(format!("The window has no display: {e}")))?;
        let handle = window.window_handle().map_err(|e| Error::native(format!("The window has no handle: {e}")))?;
        // SAFETY: the handles are the live window's and its display's (the
        // contract of `mln_gfx::Window`), and the instance was made with the
        // display's surface extension (`instance_extensions`).
        let surface = unsafe {
            match (display.as_raw(), handle.as_raw()) {
                (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
                    let create_info = vk::WaylandSurfaceCreateInfoKHR::default()
                        .display(display.display.as_ptr())
                        .surface(window.surface.as_ptr());
                    ash::khr::wayland_surface::Instance::new(entry, instance).create_wayland_surface(&create_info, None)
                }
                (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(window)) => {
                    let display = display.display.ok_or_else(|| Error::native("The window has no Xlib display"))?;
                    let create_info =
                        vk::XlibSurfaceCreateInfoKHR::default().dpy(display.as_ptr()).window(window.window);
                    ash::khr::xlib_surface::Instance::new(entry, instance).create_xlib_surface(&create_info, None)
                }
                (RawDisplayHandle::Xcb(display), RawWindowHandle::Xcb(window)) => {
                    let connection =
                        display.connection.ok_or_else(|| Error::native("The window has no XCB connection"))?;
                    let create_info = vk::XcbSurfaceCreateInfoKHR::default()
                        .connection(connection.as_ptr())
                        .window(window.window.get());
                    ash::khr::xcb_surface::Instance::new(entry, instance).create_xcb_surface(&create_info, None)
                }
                (RawDisplayHandle::Windows(_), RawWindowHandle::Win32(window)) => {
                    let create_info = vk::Win32SurfaceCreateInfoKHR::default()
                        .hinstance(window.hinstance.map_or(0, std::num::NonZeroIsize::get))
                        .hwnd(window.hwnd.get());
                    ash::khr::win32_surface::Instance::new(entry, instance).create_win32_surface(&create_info, None)
                }
                (RawDisplayHandle::Android(_), RawWindowHandle::AndroidNdk(window)) => {
                    let create_info =
                        vk::AndroidSurfaceCreateInfoKHR::default().window(window.a_native_window.as_ptr().cast());
                    ash::khr::android_surface::Instance::new(entry, instance).create_android_surface(&create_info, None)
                }
                (display, window) => {
                    return Err(Error::unsupported(format!(
                        "No Vulkan surface for this window: {window:?} on {display:?}"
                    )));
                }
            }
        };
        surface.map_err(|e| Error::native(format!("Failed to create a surface for the window: {e}")))
    }
}
