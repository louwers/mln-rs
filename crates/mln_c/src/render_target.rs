//! `render_target.h`: the targets that sessions render into, and what every
//! render target shares.

use core::ffi::c_void;
use core::ptr::NonNull;

use ::mln_runtime::{RenderTarget, Surface, SurfaceType};
use mln_util::ErrorKind;
use mln_util::size::Size;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

use crate::completion::{self, Generations, PendingCompletion};
use crate::error::Error;
use crate::generated::types::{
    MLN_RENDER_TARGET_KIND_ANDROID_WINDOW, MLN_RENDER_TARGET_KIND_HTML_CANVAS, MLN_RENDER_TARGET_KIND_METAL_LAYER,
    MLN_RENDER_TARGET_KIND_OFFSCREEN, MLN_RENDER_TARGET_KIND_OHOS_WINDOW, MLN_RENDER_TARGET_KIND_WAYLAND_SURFACE,
    MLN_RENDER_TARGET_KIND_WEB_GPU_SURFACE, MLN_RENDER_TARGET_KIND_WIN32_WINDOW, MLN_RENDER_TARGET_KIND_XLIB_WINDOW,
    mln_completion, mln_diagnostic, mln_gpu_device, mln_logical_extent, mln_logical_size, mln_map,
    mln_open_gl_context_provider_flags, mln_render_session, mln_render_session_attach_options, mln_render_target,
    mln_status,
};
use crate::{abi, gpu_device, handles, map, render_session};

abi::versioned!(mln_render_target);

/// The pixels that a logical extent covers: each dimension times the scale
/// factor, rounded up so that a target never has a fraction of a pixel too
/// few.
pub(crate) fn physical_size(extent: mln_logical_extent) -> Result<(u32, u32), Error> {
    let mln_logical_extent { width, height, scale_factor } = extent;
    if width == 0 || height == 0 || !scale_factor.is_finite() || scale_factor <= 0.0 {
        return Err(Error::invalid_argument("extent dimensions and scale_factor must be positive"));
    }
    let physical = |logical: u32| {
        let pixels = (f64::from(logical) * scale_factor).ceil();
        // `as` saturates, so the range check comes first.
        (pixels <= f64::from(u32::MAX)).then_some(pixels as u32)
    };
    match (physical(width), physical(height)) {
        (Some(width), Some(height)) => Ok((width, height)),
        _ => Err(Error::invalid_argument("scaled extent dimensions are too large")),
    }
}

// MARK: - Targets

/// A caller's checked `mln_render_target`.
enum Target {
    Offscreen(Size),
    /// A window or layer, of a type that this library presents to.
    Surface {
        size: Size,
        kind: SurfaceType,
        surface: Surface,
    },
    /// A window or surface that this library doesn't present to.
    Unsupported(&'static str),
}

impl Target {
    fn size(&self) -> Option<Size> {
        match self {
            Target::Offscreen(size) | Target::Surface { size, .. } => Some(*size),
            Target::Unsupported(_) => None,
        }
    }
}

/// `pointer`, or an error that names it as `name` when it's null.
fn required<T>(pointer: *mut T, name: &str) -> Result<NonNull<T>, Error> {
    NonNull::new(pointer).ok_or_else(|| Error::invalid_argument(format!("{name} must not be null")))
}

/// A window by its handles, which the host keeps valid while a session uses
/// them, as the header's `mln_render_target` says.
struct HostWindow {
    window: RawWindowHandle,
    display: RawDisplayHandle,
}

// SAFETY: the handles name the host's window and display, which the backends
// use from the session's thread as the platforms allow; the host's contract
// keeps them alive.
unsafe impl Send for HostWindow {}
// SAFETY: as for `Send`; nothing here changes the handles.
unsafe impl Sync for HostWindow {}

impl HasWindowHandle for HostWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the host keeps the window alive while the session uses it.
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

impl HasDisplayHandle for HostWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: the host keeps the display alive while the session uses it.
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

/// A Win32 window's target, with the module that owns the window, which
/// Vulkan asks for.
#[cfg(all(windows, target_pointer_width = "64"))]
fn win32_window(size: Size, window: NonNull<c_void>) -> Target {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongPtrW(window: isize, index: i32) -> isize;
    }
    const GWLP_HINSTANCE: i32 = -6;
    let Some(hwnd) = core::num::NonZeroIsize::new(window.as_ptr() as isize) else {
        return Target::Unsupported("a null window");
    };
    let mut handle = raw_window_handle::Win32WindowHandle::new(hwnd);
    // SAFETY: a plain query of the host's window.
    handle.hinstance = core::num::NonZeroIsize::new(unsafe { GetWindowLongPtrW(hwnd.get(), GWLP_HINSTANCE) });
    let window = HostWindow {
        window: RawWindowHandle::Win32(handle),
        display: RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new()),
    };
    Target::Surface { size, kind: SurfaceType::Win32, surface: Surface::window(window) }
}

/// Checks a caller's target, whose size `scale_factor` scales, and makes the
/// runtime's surface of a window that the library presents to.
///
/// # Safety
///
/// `target` is null or points to a readable struct of the size it declares,
/// whose buffer views are readable.
unsafe fn read_target(target: *const mln_render_target, scale_factor: Option<f64>) -> Result<Target, Error> {
    // SAFETY: the caller's contract.
    let target = unsafe { abi::read_sized(target) }?;
    // SAFETY: each arm reads the variant that `kind` names.
    let (size, name): (mln_logical_size, &str) = unsafe {
        match target.kind {
            MLN_RENDER_TARGET_KIND_OFFSCREEN => (target.data.offscreen.logical_size, "mln_offscreen_target"),
            MLN_RENDER_TARGET_KIND_METAL_LAYER => (target.data.metal_layer.logical_size, "mln_metal_layer_target"),
            MLN_RENDER_TARGET_KIND_WIN32_WINDOW => (target.data.win32_window.logical_size, "mln_win32_window_target"),
            MLN_RENDER_TARGET_KIND_XLIB_WINDOW => (target.data.xlib_window.logical_size, "mln_xlib_window_target"),
            MLN_RENDER_TARGET_KIND_WAYLAND_SURFACE => {
                (target.data.wayland_surface.logical_size, "mln_wayland_surface_target")
            }
            MLN_RENDER_TARGET_KIND_ANDROID_WINDOW => {
                (target.data.android_window.logical_size, "mln_android_window_target")
            }
            MLN_RENDER_TARGET_KIND_OHOS_WINDOW => (target.data.ohos_window.logical_size, "mln_ohos_window_target"),
            MLN_RENDER_TARGET_KIND_WEB_GPU_SURFACE => {
                (target.data.web_gpu_surface.logical_size, "mln_web_gpu_surface_target")
            }
            MLN_RENDER_TARGET_KIND_HTML_CANVAS => (target.data.html_canvas.logical_size, "mln_html_canvas_target"),
            _ => return Err(Error::invalid_argument("mln_render_target.kind is invalid")),
        }
    };
    if size.width == 0 || size.height == 0 {
        return Err(Error::invalid_argument(format!("{name}.logical_size dimensions must be positive")));
    }
    if let Some(scale_factor) = scale_factor {
        physical_size(mln_logical_extent { width: size.width, height: size.height, scale_factor })?;
    }
    let size = Size::new(size.width, size.height);
    let field = |field: &str| format!("{name}.{field}");
    // SAFETY: each arm reads the variant that `kind` names.
    let target = unsafe {
        match target.kind {
            MLN_RENDER_TARGET_KIND_OFFSCREEN => Target::Offscreen(size),
            MLN_RENDER_TARGET_KIND_METAL_LAYER => {
                let layer = required(target.data.metal_layer.layer, &field("layer"))?;
                metal_layer(size, layer)
            }
            MLN_RENDER_TARGET_KIND_WIN32_WINDOW => {
                let window = required(target.data.win32_window.window, &field("window"))?;
                #[cfg(all(windows, target_pointer_width = "64"))]
                let target = win32_window(size, window);
                #[cfg(not(all(windows, target_pointer_width = "64")))]
                let target = {
                    let _ = window;
                    Target::Unsupported("this platform has no Win32 windows")
                };
                target
            }
            MLN_RENDER_TARGET_KIND_XLIB_WINDOW => {
                let xlib = target.data.xlib_window;
                let display = required(xlib.display, &field("display"))?;
                if xlib.window == 0 {
                    return Err(Error::invalid_argument(format!("{} must not be null", field("window"))));
                }
                let window = HostWindow {
                    window: RawWindowHandle::Xlib(raw_window_handle::XlibWindowHandle::new(
                        xlib.window as core::ffi::c_ulong,
                    )),
                    display: RawDisplayHandle::Xlib(raw_window_handle::XlibDisplayHandle::new(Some(display), 0)),
                };
                Target::Surface { size, kind: SurfaceType::Xlib, surface: Surface::window(window) }
            }
            MLN_RENDER_TARGET_KIND_WAYLAND_SURFACE => {
                let wayland = target.data.wayland_surface;
                let display = required(wayland.display, &field("display"))?;
                let surface = required(wayland.surface, &field("surface"))?;
                let window = HostWindow {
                    window: RawWindowHandle::Wayland(raw_window_handle::WaylandWindowHandle::new(surface)),
                    display: RawDisplayHandle::Wayland(raw_window_handle::WaylandDisplayHandle::new(display)),
                };
                Target::Surface { size, kind: SurfaceType::Wayland, surface: Surface::window(window) }
            }
            MLN_RENDER_TARGET_KIND_ANDROID_WINDOW => {
                required(target.data.android_window.window, &field("window"))?;
                Target::Unsupported("this library presents to no Android window")
            }
            MLN_RENDER_TARGET_KIND_OHOS_WINDOW => {
                required(target.data.ohos_window.window, &field("window"))?;
                Target::Unsupported("this library presents to no OpenHarmony window")
            }
            MLN_RENDER_TARGET_KIND_WEB_GPU_SURFACE => {
                let surface = target.data.web_gpu_surface;
                required(surface.surface, &field("surface"))?;
                if surface.format == 0 {
                    return Err(Error::invalid_argument(format!("{} must not be zero", field("format"))));
                }
                Target::Unsupported("this library wraps no WebGPU device to present to a WebGPU surface with")
            }
            _ => {
                if target.data.html_canvas.selector.size == 0 {
                    return Err(Error::invalid_argument(format!("{} must not be empty", field("selector"))));
                }
                Target::Unsupported("this library presents to no HTML canvas")
            }
        }
    };
    Ok(target)
}

/// A `CAMetalLayer`'s target, on the platforms that have one.
fn metal_layer(size: Size, layer: NonNull<c_void>) -> Target {
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: the host's layer, which the header's contract keeps alive.
        match unsafe { mln_mtl::Layer::from_raw(layer.as_ptr()) } {
            Some(layer) => {
                Target::Surface { size, kind: SurfaceType::MetalLayer, surface: Surface::metal_layer(layer) }
            }
            None => Target::Unsupported("a null layer"),
        }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = (size, layer);
        Target::Unsupported("this platform has no Metal layers")
    }
}

fn unsupported(message: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::new(ErrorKind::Unsupported, message)
}

/// `mln_map_attach()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_map_attach(
    map: mln_map,
    target: *const mln_render_target,
    device: mln_gpu_device,
    options: *const mln_render_session_attach_options,
    out_session: *mut mln_render_session,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        let map = map::resolve(map)?;
        // SAFETY: the caller's contract.
        let target = unsafe { read_target(target, Some(map.map.scale_factor())) }?;
        let device = gpu_device::resolve(device)?;
        // SAFETY: the caller's contract.
        let options = unsafe { render_session::attach_options(options) }?;
        // SAFETY: the caller's contract.
        let out_session = unsafe { handles::out_handle(out_session, "out_session") }?;
        // SAFETY: the caller's contract.
        let pending = unsafe { PendingCompletion::new(completion) }?;
        let target = match target {
            Target::Offscreen(size) => {
                options.require_core_worker("an offscreen session renders on a core worker")?;
                options.refuse_queue_lock("an offscreen session takes no queue lock")?;
                RenderTarget::Offscreen { size, device }
            }
            Target::Surface { size, kind, surface } => {
                let presents =
                    device.as_ref().map_or(kind.presents_without_device(), |device| device.presents_to(kind));
                if !presents {
                    return Err(unsupported(format!("the device presents to no {kind:?} surface")));
                }
                options.require_core_worker("a window session renders on a core worker")?;
                options.refuse_queue_lock("a window session takes no queue lock")?;
                RenderTarget::Surface { size, surface, device }
            }
            Target::Unsupported(message) => return Err(unsupported(message)),
        };
        let (handle, attached) = render_session::attach(&map, target, options)?;
        // The completion may run before this call returns, and the caller may
        // read the handle there.
        *out_session = handle;
        completion::forward(attached, Some(pending.accept()), |result| {
            (result.map_err(Error::from), Generations::default())
        });
        Ok(())
    })
}

/// `mln_render_session_set_target()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_render_session_set_target(
    session: mln_render_session,
    target: *const mln_render_target,
    completion: *const mln_completion,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // A stale session has no scale factor to check the target's size with:
        // the session checks it once it's found.
        // SAFETY: the caller's contract.
        let target = unsafe { read_target(target, None) }?;
        if let Target::Offscreen(_) = target {
            return Err(unsupported("an offscreen target replaces no window"));
        }
        // SAFETY: the caller's contract.
        let pending = unsafe { PendingCompletion::optional(completion) }?;
        let object = render_session::resolve(session)?;
        let scale_factor = object.session.snapshot()?.extent.scale_factor;
        if let Some(size) = target.size() {
            physical_size(mln_logical_extent { width: size.width, height: size.height, scale_factor })?;
        }
        let (size, surface) = match target {
            Target::Surface { size, kind, surface } => {
                if !kind.presents_with_any() {
                    return Err(unsupported(format!("this build presents to no {kind:?} surface")));
                }
                (size, surface)
            }
            Target::Unsupported(message) => return Err(unsupported(message)),
            Target::Offscreen(_) => return Err(unsupported("an offscreen target replaces no window")),
        };
        let replaced = object.session.set_surface(size, surface)?;
        let map = object.map.clone();
        completion::forward(replaced, pending.map(PendingCompletion::accept), move |result| {
            let generations = map::generations(&map, &result);
            (result.map(drop).map_err(Error::from), generations)
        });
        Ok(())
    })
}

/// `mln_logical_extent_physical_size()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_logical_extent_physical_size(
    extent: mln_logical_extent,
    out_width: *mut u32,
    out_height: *mut u32,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_width, out_height), {
        let (width, height) = physical_size(extent)?;
        // SAFETY: both are non-null, and the caller's contract.
        unsafe {
            out_width.write(width);
            out_height.write(height);
        }
        Ok(())
    })
}

/// `mln_open_gl_supported_context_provider_mask()`: none, since the library
/// doesn't render with OpenGL.
#[unsafe(no_mangle)]
pub extern "C" fn mln_open_gl_supported_context_provider_mask() -> mln_open_gl_context_provider_flags {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::types::{MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_OK};

    fn call(width: u32, height: u32, scale_factor: f64) -> (mln_status, u32, u32) {
        let (mut out_width, mut out_height) = (7, 7);
        let extent = mln_logical_extent { width, height, scale_factor };
        // SAFETY: local outputs and a null diagnostic.
        let status =
            unsafe { mln_logical_extent_physical_size(extent, &mut out_width, &mut out_height, core::ptr::null_mut()) };
        (status, out_width, out_height)
    }

    #[test]
    fn physical_sizes_round_up_and_reject_overflow() {
        assert_eq!(call(3, 5, 1.5), (MLN_STATUS_OK, 5, 8));
        assert_eq!(call(1, 3, 0.25), (MLN_STATUS_OK, 1, 1));
        assert_eq!(call(u32::MAX, 1, 1.0), (MLN_STATUS_OK, u32::MAX, 1));
        // A failed call leaves the outputs alone.
        assert_eq!(call(0x8000_0000, 1, 2.0), (MLN_STATUS_INVALID_ARGUMENT, 7, 7));
        assert_eq!(call(1, 1, f64::NAN), (MLN_STATUS_INVALID_ARGUMENT, 7, 7));
        assert_eq!(call(0, 1, 1.0), (MLN_STATUS_INVALID_ARGUMENT, 7, 7));
        let mut height = 0;
        let extent = mln_logical_extent { width: 1, height: 1, scale_factor: 1.0 };
        // SAFETY: a null output, a local one, and a null diagnostic.
        let status = unsafe {
            mln_logical_extent_physical_size(extent, core::ptr::null_mut(), &mut height, core::ptr::null_mut())
        };
        assert_eq!(status, MLN_STATUS_INVALID_ARGUMENT);
    }

    fn target(kind: u32, data: crate::generated::types::mln_render_target_data) -> mln_render_target {
        mln_render_target { size: size_of::<mln_render_target>() as u32, kind, data }
    }

    fn read(target: &mln_render_target, scale_factor: Option<f64>) -> Result<Target, mln_status> {
        // SAFETY: a local target.
        unsafe { read_target(target, scale_factor) }.map_err(|error| error.status())
    }

    fn wayland(display: *mut c_void, surface: *mut c_void) -> mln_render_target {
        use crate::generated::types::{mln_render_target_data, mln_wayland_surface_target};
        let logical_size = mln_logical_size { width: 64, height: 32 };
        let data = mln_wayland_surface_target { logical_size, display, surface };
        target(MLN_RENDER_TARGET_KIND_WAYLAND_SURFACE, mln_render_target_data { wayland_surface: data })
    }

    #[test]
    fn targets_check_their_kind_size_and_objects() {
        use crate::generated::types::{mln_android_window_target, mln_offscreen_target, mln_render_target_data};
        let fake = NonNull::<c_void>::dangling().as_ptr();
        let Ok(Target::Surface { size, kind, .. }) = read(&wayland(fake, fake), Some(2.0)) else {
            panic!("a Wayland surface is a surface target")
        };
        assert_eq!((size, kind), (Size::new(64, 32), SurfaceType::Wayland));
        assert_eq!(read(&wayland(core::ptr::null_mut(), fake), None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));
        assert_eq!(read(&wayland(fake, core::ptr::null_mut()), None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));

        let offscreen = |width, height| {
            let logical_size = mln_logical_size { width, height };
            let data = mln_render_target_data { offscreen: mln_offscreen_target { logical_size } };
            target(MLN_RENDER_TARGET_KIND_OFFSCREEN, data)
        };
        assert!(matches!(read(&offscreen(8, 8), Some(1.0)), Ok(Target::Offscreen(size)) if size == Size::new(8, 8)));
        assert_eq!(read(&offscreen(0, 8), None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));
        // Only a scale factor makes the size too large.
        assert!(read(&offscreen(u32::MAX, 8), None).is_ok());
        assert_eq!(read(&offscreen(u32::MAX, 8), Some(2.0)).err(), Some(MLN_STATUS_INVALID_ARGUMENT));
        let mut unknown = offscreen(8, 8);
        unknown.kind = 99;
        assert_eq!(read(&unknown, None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));
        let mut undersized = offscreen(8, 8);
        undersized.size -= 1;
        assert_eq!(read(&undersized, None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));

        let logical_size = mln_logical_size { width: 8, height: 8 };
        let android = |window| {
            let data = mln_render_target_data { android_window: mln_android_window_target { logical_size, window } };
            target(MLN_RENDER_TARGET_KIND_ANDROID_WINDOW, data)
        };
        assert!(matches!(read(&android(fake), None), Ok(Target::Unsupported(_))));
        assert_eq!(read(&android(core::ptr::null_mut()), None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));
    }

    /// An Xlib target of `window`, a winit window on X11, at `width` by
    /// `height` logical pixels.
    #[cfg(target_os = "linux")]
    fn xlib(window: &winit::window::Window, width: u32, height: u32) -> mln_render_target {
        use crate::generated::types::{mln_render_target_data, mln_xlib_window_target};
        let (Ok(handle), Ok(display)) = (window.window_handle(), window.display_handle()) else {
            panic!("the window has handles")
        };
        let (RawWindowHandle::Xlib(handle), RawDisplayHandle::Xlib(display)) = (handle.as_raw(), display.as_raw())
        else {
            panic!("the window is an Xlib window")
        };
        let display = display.display.map_or(core::ptr::null_mut(), NonNull::as_ptr);
        let logical_size = mln_logical_size { width, height };
        #[allow(clippy::useless_conversion, reason = "`c_ulong` is narrower on 32-bit targets")]
        let window = u64::from(handle.window);
        let data = mln_xlib_window_target { logical_size, display, window };
        target(MLN_RENDER_TARGET_KIND_XLIB_WINDOW, mln_render_target_data { xlib_window: data })
    }

    // A window target, through the C API, on each backend: it presents,
    // moves to another window, and takes a new scale factor. It needs an X server that the
    // GPU can present to, as mln_runtime's surfaces_window test does.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "needs an X server"]
    fn an_xlib_window_target_presents_moves_and_rescales_on_each_backend() {
        use winit::dpi::PhysicalSize;
        use winit::event_loop::EventLoop;
        use winit::platform::x11::EventLoopBuilderExtX11;
        use winit::window::Window;

        use crate::generated::types::{
            MLN_FRAME_DEMAND_FORCE, MLN_GPU_DEVICE_OPTION_BACKEND, MLN_RENDER_RESULT_RENDERED,
            MLN_RENDER_SESSION_CAPABILITY_PRESENTATION, mln_gpu_device_options, mln_render_session_capabilities,
        };
        use crate::gpu_device::mln_gpu_device_create;
        use crate::map::mln_map_set_style_json;
        use crate::map::tests::{create_map, options, release_map, view};
        use crate::render_session::tests::{attach_options, demand, release_session, sized, snapshot, wait_for_result};
        use crate::render_session::{mln_render_session_get_capabilities, mln_render_session_resize};
        use crate::runtime::tests::{Waiter, create, release};

        let Ok(event_loop) = EventLoop::builder().with_x11().with_any_thread(true).build() else {
            panic!("an X11 event loop")
        };
        let window = |width, height| {
            #[expect(deprecated, reason = "the test runs no event loop to create windows from")]
            let window = event_loop
                .create_window(Window::default_attributes().with_inner_size(PhysicalSize::new(width, height)));
            window.unwrap_or_else(|error| panic!("an X11 window: {error}"))
        };
        let (first, second) = (window(64, 48), window(80, 40));
        let runtime = create();
        let map = create_map(runtime, &options(1.0));
        let styled = Waiter::new();
        let style = r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"red"}}]}"#;
        // SAFETY: local arguments.
        unsafe { mln_map_set_style_json(map, view(style), &styled.descriptor, core::ptr::null_mut()) };
        assert_eq!(styled.status(), MLN_STATUS_OK);
        let rendered = |session, token| {
            assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token), MLN_STATUS_OK);
            let result = wait_for_result(session);
            assert_eq!(result.disposition, MLN_RENDER_RESULT_RENDERED);
            result
        };

        // A session on a library device of each backend, then one that makes
        // its own.
        let mut devices: Vec<_> = gpu_device::BACKENDS
            .iter()
            .map(|(backend, value, _)| {
                let mut options = sized::<mln_gpu_device_options>();
                options.fields = MLN_GPU_DEVICE_OPTION_BACKEND;
                options.backend = *value;
                let mut device = 0;
                // SAFETY: local arguments.
                let status = unsafe { mln_gpu_device_create(&options, &mut device, core::ptr::null_mut()) };
                assert_eq!(status, MLN_STATUS_OK, "a {backend:?} device");
                device
            })
            .collect();
        devices.push(0);
        for (index, device) in devices.into_iter().enumerate() {
            let token = 10 * index as u64;
            let (target, options) = (xlib(&first, 64, 48), attach_options());
            let (mut session, attached) = (0, Waiter::new());
            // SAFETY: local arguments.
            let status = unsafe {
                mln_map_attach(
                    map,
                    &target,
                    device,
                    &options,
                    &mut session,
                    &attached.descriptor,
                    core::ptr::null_mut(),
                )
            };
            gpu_device::mln_gpu_device_release(device);
            assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK), "device {index}");
            let mut capabilities = sized::<mln_render_session_capabilities>();
            // SAFETY: local capabilities.
            unsafe { mln_render_session_get_capabilities(session, &mut capabilities, core::ptr::null_mut()) };
            assert_eq!(capabilities.flags, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION);
            rendered(session, token + 1);

            let (target, moved) = (xlib(&second, 80, 40), Waiter::new());
            // SAFETY: local arguments.
            let status =
                unsafe { mln_render_session_set_target(session, &target, &moved.descriptor, core::ptr::null_mut()) };
            assert_eq!((status, moved.status()), (MLN_STATUS_OK, MLN_STATUS_OK), "device {index}");
            assert_eq!(rendered(session, token + 2).view.extent.width, 80);

            let extent = mln_logical_extent { width: 40, height: 20, scale_factor: 2.0 };
            let resized = Waiter::new();
            // SAFETY: local arguments.
            let status =
                unsafe { mln_render_session_resize(session, extent, &resized.descriptor, core::ptr::null_mut()) };
            assert_eq!((status, resized.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
            assert_eq!(rendered(session, token + 3).view.extent.scale_factor, 2.0);
            assert_eq!(snapshot(session).map(|snapshot| snapshot.extent.scale_factor), Ok(2.0));
            release_session(session);
        }

        release_map(map);
        release(runtime);
    }

    #[test]
    fn no_open_gl_provider_is_built() {
        assert_eq!(mln_open_gl_supported_context_provider_mask(), 0);
    }
}
