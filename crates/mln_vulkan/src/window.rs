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
        RawDisplayHandle::Ohos(_) => Ok(crate::device::OHOS_SURFACE),
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
                (RawDisplayHandle::Ohos(_), RawWindowHandle::OhosNdk(window)) => {
                    create_ohos_surface(entry, instance, window.native_window.as_ptr())
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

/// `VkSurfaceCreateInfoOHOS`, of `VK_OHOS_surface`, which ash lacks.
#[repr(C)]
struct SurfaceCreateInfoOhos {
    s_type: vk::StructureType,
    p_next: *const std::ffi::c_void,
    flags: vk::Flags,
    window: *mut std::ffi::c_void,
}

/// `VK_STRUCTURE_TYPE_SURFACE_CREATE_INFO_OHOS`.
const STRUCTURE_TYPE_SURFACE_CREATE_INFO_OHOS: vk::StructureType = vk::StructureType::from_raw(1_000_685_000);

/// `PFN_vkCreateSurfaceOHOS`.
type CreateSurfaceOhos = unsafe extern "system" fn(
    vk::Instance,
    *const SurfaceCreateInfoOhos,
    *const vk::AllocationCallbacks<'_>,
    *mut vk::SurfaceKHR,
) -> vk::Result;

/// A surface of `window`, an `OHNativeWindow*`, through `VK_OHOS_surface`.
///
/// # Safety
///
/// `window` is live, and `instance` was made with `VK_OHOS_surface`.
unsafe fn create_ohos_surface(
    entry: &ash::Entry,
    instance: &ash::Instance,
    window: *mut std::ffi::c_void,
) -> Result<vk::SurfaceKHR, vk::Result> {
    // SAFETY: the instance is live, and the name a function's name.
    let function = unsafe { entry.get_instance_proc_addr(instance.handle(), c"vkCreateSurfaceOHOS".as_ptr()) }
        .ok_or(vk::Result::ERROR_EXTENSION_NOT_PRESENT)?;
    // SAFETY: the extension's function has this signature.
    let create = unsafe { std::mem::transmute::<unsafe extern "system" fn(), CreateSurfaceOhos>(function) };
    let info = SurfaceCreateInfoOhos {
        s_type: STRUCTURE_TYPE_SURFACE_CREATE_INFO_OHOS,
        p_next: std::ptr::null(),
        flags: vk::Flags::default(),
        window,
    };
    let mut surface = vk::SurfaceKHR::null();
    // SAFETY: the create info names the caller's live window.
    match unsafe { create(instance.handle(), &raw const info, std::ptr::null(), &raw mut surface) } {
        vk::Result::SUCCESS => Ok(surface),
        error => Err(error),
    }
}
