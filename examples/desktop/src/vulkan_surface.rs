// The window's Vulkan surface: made on the window's thread from the
// window's handles, for the instance of the backend that asks for it
// (`WindowSurface`, on the render thread). The backend owns the surface and
// destroys it, before the window closes.

// Vulkan through ash is the FFI here.
#![allow(unsafe_code)]

use std::ffi::CStr;
use std::sync::mpsc;

use ash::vk;
use mln_util::error::Error;
use mln_vulkan::renderer_backend::SurfaceProvider;
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use winit::window::Window;

use crate::backend::closed;
use crate::host::UserEvent;

/// The instance extensions a surface of `window` needs: VK_KHR_surface and
/// the one of the window's display.
pub(crate) fn instance_extensions(window: &Window) -> Result<Vec<&'static CStr>, String> {
    let display = window.display_handle().map_err(|e| format!("The window has no display handle: {e}"))?;
    let platform = match display.as_raw() {
        RawDisplayHandle::Wayland(_) => ash::khr::wayland_surface::NAME,
        RawDisplayHandle::Xlib(_) => ash::khr::xlib_surface::NAME,
        RawDisplayHandle::Xcb(_) => ash::khr::xcb_surface::NAME,
        RawDisplayHandle::Windows(_) => ash::khr::win32_surface::NAME,
        other => return Err(format!("No Vulkan surface for this display: {other:?}")),
    };
    Ok(vec![ash::khr::surface::NAME, platform])
}

/// A surface of `window` for `instance`, which was created with
/// `instance_extensions(window)`. Call on the window's thread.
pub(crate) fn create_surface(
    window: &Window,
    entry: &ash::Entry,
    instance: &ash::Instance,
) -> Result<vk::SurfaceKHR, String> {
    let display = window.display_handle().map_err(|e| format!("The window has no display handle: {e}"))?;
    let handle = window.window_handle().map_err(|e| format!("The window has no handle: {e}"))?;
    // SAFETY: the handles are the live window's and its display's, the
    // instance has the display's surface extension, and the window outlives
    // the surface: it closes after the render thread has dropped its
    // backends.
    let surface = unsafe {
        match (display.as_raw(), handle.as_raw()) {
            (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
                let create_info = vk::WaylandSurfaceCreateInfoKHR::default()
                    .display(display.display.as_ptr())
                    .surface(window.surface.as_ptr());
                ash::khr::wayland_surface::Instance::new(entry, instance).create_wayland_surface(&create_info, None)
            }
            (RawDisplayHandle::Xlib(display), RawWindowHandle::Xlib(window)) => {
                let display = display.display.ok_or("The window has no Xlib display")?;
                let create_info = vk::XlibSurfaceCreateInfoKHR::default().dpy(display.as_ptr()).window(window.window);
                ash::khr::xlib_surface::Instance::new(entry, instance).create_xlib_surface(&create_info, None)
            }
            (RawDisplayHandle::Xcb(display), RawWindowHandle::Xcb(window)) => {
                let connection = display.connection.ok_or("The window has no XCB connection")?;
                let create_info =
                    vk::XcbSurfaceCreateInfoKHR::default().connection(connection.as_ptr()).window(window.window.get());
                ash::khr::xcb_surface::Instance::new(entry, instance).create_xcb_surface(&create_info, None)
            }
            (RawDisplayHandle::Windows(_), RawWindowHandle::Win32(window)) => {
                let create_info = vk::Win32SurfaceCreateInfoKHR::default()
                    .hinstance(window.hinstance.map_or(0, std::num::NonZeroIsize::get))
                    .hwnd(window.hwnd.get());
                ash::khr::win32_surface::Instance::new(entry, instance).create_win32_surface(&create_info, None)
            }
            (display, window) => return Err(format!("No Vulkan surface for this window: {window:?} on {display:?}")),
        }
    };
    surface.map_err(|e| format!("Failed to create a surface for the window: {e}"))
}

/// The window as the Vulkan backend's surface provider: it asks the
/// window's thread through `proxy` and waits for the answer.
pub(crate) struct WindowSurface {
    proxy: EventLoopProxy<UserEvent>,
    extensions: Vec<&'static CStr>,
}

impl WindowSurface {
    pub(crate) fn new(proxy: &EventLoopProxy<UserEvent>) -> Result<WindowSurface, Error> {
        let (reply, extensions) = mpsc::channel();
        proxy.send_event(UserEvent::VulkanExtensions(reply)).map_err(|_| closed())?;
        let extensions = extensions.recv().map_err(|_| closed())?.map_err(Error::unsupported)?;
        Ok(WindowSurface { proxy: proxy.clone(), extensions })
    }
}

impl SurfaceProvider for WindowSurface {
    fn instance_extensions(&self) -> Vec<&'static CStr> {
        self.extensions.clone()
    }

    fn create_surface(&self, entry: &ash::Entry, instance: &ash::Instance) -> Result<vk::SurfaceKHR, Error> {
        let (reply, surface) = mpsc::channel();
        self.proxy
            .send_event(UserEvent::MakeVulkanSurface(Box::new((entry.clone(), instance.clone())), reply))
            .map_err(|_| closed())?;
        surface.recv().map_err(|_| closed())?.map_err(Error::native)
    }
}
