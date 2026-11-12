//! `render_target.h`: the targets that sessions render into, and what every
//! render target shares.

use core::ffi::c_void;
use core::ptr::NonNull;

use ::mln_runtime::{
    BorrowedRing, DmaBufFormat, ExportKind, ExportKinds, ExportedRing, RenderTarget, Surface, SurfaceType,
};
use mln_util::ErrorKind;
use mln_util::size::Size;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

use crate::completion::{self, Generations, PendingCompletion};
use crate::error::Error;
use crate::generated::types::{
    MLN_RENDER_TARGET_KIND_ANDROID_WINDOW, MLN_RENDER_TARGET_KIND_EXPORTED_RING, MLN_RENDER_TARGET_KIND_HTML_CANVAS,
    MLN_RENDER_TARGET_KIND_METAL_BORROWED_TEXTURES, MLN_RENDER_TARGET_KIND_METAL_LAYER,
    MLN_RENDER_TARGET_KIND_OFFSCREEN, MLN_RENDER_TARGET_KIND_OHOS_WINDOW,
    MLN_RENDER_TARGET_KIND_OPEN_GL_BORROWED_TEXTURES, MLN_RENDER_TARGET_KIND_OPEN_GL_SURFACE,
    MLN_RENDER_TARGET_KIND_VULKAN_BORROWED_TEXTURES, MLN_RENDER_TARGET_KIND_VULKAN_SURFACE,
    MLN_RENDER_TARGET_KIND_WAYLAND_SURFACE, MLN_RENDER_TARGET_KIND_WEB_GPU_BORROWED_TEXTURES,
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
    /// The host's surface, which only a device of the host's instance or
    /// share group presents to. Apple builds have neither kind.
    #[cfg_attr(any(target_vendor = "apple", target_family = "wasm"), allow(dead_code))]
    HostSurface {
        size: Size,
        surface: Surface,
    },
    /// A ring of the host's textures.
    Borrowed {
        size: Size,
        ring: BorrowedRing,
    },
    /// A ring of the library's images, which it exports.
    Exported {
        size: Size,
        ring: ExportedRing,
    },
    /// A target that this library doesn't render to.
    Unsupported(&'static str),
}

impl Target {
    fn size(&self) -> Option<Size> {
        match self {
            Target::Offscreen(size)
            | Target::Surface { size, .. }
            | Target::HostSurface { size, .. }
            | Target::Borrowed { size, .. }
            | Target::Exported { size, .. } => Some(*size),
            Target::Unsupported(_) => None,
        }
    }
}

/// A borrowed ring's textures: `count` of them at `textures`, from one to
/// `MLN_TEXTURE_RING_MAX_DEPTH`, distinct by `key`, of a positive size.
///
/// # Safety
///
/// `textures` is null or points to `count` readable entries.
unsafe fn ring_textures<'a, T, K: PartialEq>(
    textures: *const T,
    count: usize,
    width: u32,
    height: u32,
    name: &str,
    key: impl Fn(&T) -> K,
) -> Result<&'a [T], Error> {
    if textures.is_null() || count == 0 {
        return Err(Error::invalid_argument(format!("{name} needs at least one texture")));
    }
    if count > crate::generated::types::MLN_TEXTURE_RING_MAX_DEPTH as usize {
        return Err(Error::invalid_argument(format!("{name} has more textures than MLN_TEXTURE_RING_MAX_DEPTH")));
    }
    if width == 0 || height == 0 {
        return Err(Error::invalid_argument(format!("{name} physical texture dimensions must be positive")));
    }
    // SAFETY: the caller's contract.
    let textures = unsafe { core::slice::from_raw_parts(textures, count) };
    for (index, texture) in textures.iter().enumerate() {
        if textures[..index].iter().any(|earlier| key(earlier) == key(texture)) {
            return Err(Error::invalid_argument(format!("{name} must name distinct textures")));
        }
    }
    Ok(textures)
}

/// A borrowed ring's depth textures: none, or `count` at `textures`, one for
/// each of `colors`.
///
/// # Safety
///
/// `textures` is null or points to `count` readable entries.
unsafe fn depth_textures<'a, T, C>(
    textures: *const T,
    count: usize,
    colors: &[C],
    name: &str,
) -> Result<&'a [T], Error> {
    if count == 0 {
        return Ok(&[]);
    }
    if textures.is_null() || count != colors.len() {
        return Err(Error::invalid_argument(format!("{name} needs one depth texture for each texture, or none")));
    }
    // SAFETY: the caller's contract.
    Ok(unsafe { core::slice::from_raw_parts(textures, count) })
}

/// The ring of a caller's Vulkan images.
fn vulkan_ring(
    target: &crate::generated::types::mln_vulkan_borrowed_textures_target,
    name: &str,
) -> Result<Target, Error> {
    use crate::generated::types::{
        MLN_VULKAN_IMAGE_OWNER_EXTERNAL, MLN_VULKAN_IMAGE_OWNER_FOREIGN, MLN_VULKAN_IMAGE_OWNER_GRAPHICS_QUEUE,
    };
    // SAFETY: the header's contract for the textures array.
    let textures = unsafe {
        ring_textures(target.textures, target.texture_count, target.physical_width, target.physical_height, name, |t| {
            t.image
        })
    }?;
    if textures.iter().any(|texture| texture.image == 0 || texture.image_view == 0) {
        return Err(Error::invalid_argument(format!("{name} textures need an image and an image view")));
    }
    // VK_IMAGE_LAYOUT_UNDEFINED and VK_IMAGE_LAYOUT_PREINITIALIZED aren't
    // layouts that a render pass leaves.
    if target.format == 0 || target.final_layout == 0 || target.final_layout == 8 {
        return Err(Error::invalid_argument(format!("{name} needs a format and final_layout")));
    }
    let owner = match target.owner {
        MLN_VULKAN_IMAGE_OWNER_GRAPHICS_QUEUE | MLN_VULKAN_IMAGE_OWNER_EXTERNAL | MLN_VULKAN_IMAGE_OWNER_FOREIGN => {
            target.owner
        }
        _ => return Err(Error::invalid_argument(format!("{name}.owner is invalid"))),
    };
    // SAFETY: the header's contract for the depth textures array.
    let depth_textures = unsafe { depth_textures(target.depth_textures, target.depth_texture_count, textures, name) }?;
    if depth_textures.iter().any(|texture| texture.image == 0 || texture.image_view == 0) {
        return Err(Error::invalid_argument(format!("{name} depth textures need an image and an image view")));
    }
    if !depth_textures.is_empty()
        && (target.depth_format == 0 || target.depth_final_layout == 0 || target.depth_final_layout == 8)
    {
        return Err(Error::invalid_argument(format!("{name} needs a depth_format and depth_final_layout")));
    }
    let size = Size::new(target.logical_size.width, target.logical_size.height);
    #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
    {
        use mln_vulkan::ash::vk::{self, Handle as _};
        let borrowed = |textures: &[crate::generated::types::mln_vulkan_borrowed_texture]| {
            textures
                .iter()
                .map(|texture| mln_vulkan::BorrowedImage {
                    image: vk::Image::from_raw(texture.image),
                    view: vk::ImageView::from_raw(texture.image_view),
                })
                .collect::<Vec<_>>()
        };
        let images = mln_vulkan::BorrowedImages {
            images: borrowed(textures),
            size: Size::new(target.physical_width, target.physical_height),
            format: vk::Format::from_raw(target.format as i32),
            final_layout: vk::ImageLayout::from_raw(target.final_layout as i32),
            owner: match owner {
                MLN_VULKAN_IMAGE_OWNER_EXTERNAL => mln_vulkan::ImageOwner::External,
                MLN_VULKAN_IMAGE_OWNER_FOREIGN => mln_vulkan::ImageOwner::Foreign,
                _ => mln_vulkan::ImageOwner::GraphicsQueue,
            },
            depth: (!depth_textures.is_empty()).then(|| mln_vulkan::BorrowedDepth {
                images: borrowed(depth_textures),
                format: vk::Format::from_raw(target.depth_format as i32),
                final_layout: vk::ImageLayout::from_raw(target.depth_final_layout as i32),
            }),
        };
        Ok(Target::Borrowed { size, ring: BorrowedRing::vulkan(images) })
    }
    #[cfg(any(target_vendor = "apple", target_family = "wasm"))]
    {
        let _ = (owner, size);
        Ok(Target::Unsupported("Vulkan targets are not supported by this build"))
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

/// An OpenHarmony window that the session holds a reference to, which it
/// drops once it lets go of the window.
#[cfg(target_env = "ohos")]
struct OhosWindow(NonNull<c_void>);

#[cfg(target_env = "ohos")]
#[link(name = "native_window")]
unsafe extern "C" {
    fn OH_NativeWindow_NativeObjectReference(object: *mut c_void) -> i32;
    fn OH_NativeWindow_NativeObjectUnreference(object: *mut c_void) -> i32;
}

// SAFETY: an `OHNativeWindow` is reference counted, and the backends use it
// from the session's thread, as OpenHarmony allows.
#[cfg(target_env = "ohos")]
unsafe impl Send for OhosWindow {}
// SAFETY: as for `Send`; nothing here changes the window.
#[cfg(target_env = "ohos")]
unsafe impl Sync for OhosWindow {}

#[cfg(target_env = "ohos")]
impl HasWindowHandle for OhosWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = RawWindowHandle::OhosNdk(raw_window_handle::OhosNdkWindowHandle::new(self.0));
        // SAFETY: the reference keeps the window alive.
        Ok(unsafe { WindowHandle::borrow_raw(handle) })
    }
}

#[cfg(target_env = "ohos")]
impl HasDisplayHandle for OhosWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: OpenHarmony's display handle names nothing.
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Ohos(raw_window_handle::OhosDisplayHandle::new())) })
    }
}

#[cfg(target_env = "ohos")]
impl Drop for OhosWindow {
    fn drop(&mut self) {
        // SAFETY: the reference that `ohos_window` took.
        unsafe { OH_NativeWindow_NativeObjectUnreference(self.0.as_ptr()) };
    }
}

/// An OpenHarmony window's target, which takes its own reference to the
/// window, as the header says.
///
/// Fails with an invalid argument for a pointer that is no `OHNativeWindow`.
#[cfg(target_env = "ohos")]
fn ohos_window(size: Size, window: NonNull<c_void>) -> Result<Target, Error> {
    // SAFETY: the host's window, which OpenHarmony checks is a native object.
    if unsafe { OH_NativeWindow_NativeObjectReference(window.as_ptr()) } != 0 {
        return Err(Error::invalid_argument("mln_ohos_window_target.window is no OHNativeWindow"));
    }
    let window = OhosWindow(window);
    Ok(Target::Surface { size, kind: SurfaceType::OhosNdk, surface: Surface::window(window) })
}

#[cfg(not(target_env = "ohos"))]
#[allow(clippy::unnecessary_wraps, reason = "OpenHarmony's checks the window")]
fn ohos_window(size: Size, window: NonNull<c_void>) -> Result<Target, Error> {
    let _ = (size, window);
    Ok(Target::Unsupported("this platform has no OpenHarmony windows"))
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
            MLN_RENDER_TARGET_KIND_VULKAN_SURFACE => {
                (target.data.vulkan_surface.logical_size, "mln_vulkan_surface_target")
            }
            MLN_RENDER_TARGET_KIND_OPEN_GL_SURFACE => {
                (target.data.open_gl_surface.logical_size, "mln_open_gl_surface_target")
            }
            MLN_RENDER_TARGET_KIND_METAL_BORROWED_TEXTURES => {
                (target.data.metal_borrowed_textures.logical_size, "mln_metal_borrowed_textures_target")
            }
            MLN_RENDER_TARGET_KIND_VULKAN_BORROWED_TEXTURES => {
                (target.data.vulkan_borrowed_textures.logical_size, "mln_vulkan_borrowed_textures_target")
            }
            MLN_RENDER_TARGET_KIND_OPEN_GL_BORROWED_TEXTURES => {
                (target.data.open_gl_borrowed_textures.logical_size, "mln_open_gl_borrowed_textures_target")
            }
            MLN_RENDER_TARGET_KIND_WEB_GPU_BORROWED_TEXTURES => {
                (target.data.web_gpu_borrowed_textures.logical_size, "mln_web_gpu_borrowed_textures_target")
            }
            MLN_RENDER_TARGET_KIND_EXPORTED_RING => {
                (target.data.exported_ring.logical_size, "mln_exported_ring_target")
            }
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
                let window = required(target.data.android_window.window, &field("window"))?;
                #[cfg(target_os = "android")]
                {
                    // SAFETY: the host's window, which the header's contract
                    // keeps alive through the call; the session keeps its own
                    // reference from here on.
                    let window = crate::android::Window::acquire(window);
                    Target::Surface { size, kind: SurfaceType::AndroidNdk, surface: Surface::window(window) }
                }
                #[cfg(not(target_os = "android"))]
                {
                    let _ = window;
                    Target::Unsupported("this platform has no Android windows")
                }
            }
            MLN_RENDER_TARGET_KIND_OHOS_WINDOW => {
                ohos_window(size, required(target.data.ohos_window.window, &field("window"))?)?
            }
            MLN_RENDER_TARGET_KIND_WEB_GPU_SURFACE => {
                let surface = target.data.web_gpu_surface;
                required(surface.surface, &field("surface"))?;
                if surface.format == 0 {
                    return Err(Error::invalid_argument(format!("{} must not be zero", field("format"))));
                }
                Target::Unsupported("this library wraps no WebGPU device to present to a WebGPU surface with")
            }
            MLN_RENDER_TARGET_KIND_VULKAN_SURFACE => {
                let surface = target.data.vulkan_surface.surface;
                if surface == 0 {
                    return Err(Error::invalid_argument(format!("{} must not be null", field("surface"))));
                }
                #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
                {
                    // SAFETY: the host's surface, which the header's contract
                    // keeps alive; the attach checks that it is of the
                    // device's instance as far as it can: the device must be
                    // a Vulkan device.
                    let surface = Surface::vulkan(mln_vulkan::HostSurface::from_raw(surface));
                    Target::HostSurface { size, surface }
                }
                #[cfg(any(target_vendor = "apple", target_family = "wasm"))]
                Target::Unsupported("Vulkan targets are not supported by this build")
            }
            MLN_RENDER_TARGET_KIND_OPEN_GL_SURFACE => {
                let surface = required(target.data.open_gl_surface.surface, &field("surface"))?;
                #[cfg(all(unix, not(target_vendor = "apple")))]
                {
                    // SAFETY: the host's surface, which the header's contract
                    // keeps alive; the attach checks that the device wraps a
                    // context of the host's.
                    let surface = Surface::gl(mln_gl::HostSurface::from_raw(surface));
                    Target::HostSurface { size, surface }
                }
                #[cfg(windows)]
                {
                    // SAFETY: the host's `HDC`, which the header's contract
                    // keeps alive; the attach checks that the device wraps a
                    // WGL context of the host's.
                    let surface = Surface::gl(mln_gl::HostSurface::from_hdc(surface));
                    Target::HostSurface { size, surface }
                }
                #[cfg(not(any(windows, all(unix, not(target_vendor = "apple")))))]
                {
                    let _ = surface;
                    Target::Unsupported("OpenGL surfaces of the host's are not supported by this build")
                }
            }
            MLN_RENDER_TARGET_KIND_METAL_BORROWED_TEXTURES => {
                let ring = target.data.metal_borrowed_textures;
                let textures = ring_textures(
                    ring.textures,
                    ring.texture_count,
                    ring.physical_width,
                    ring.physical_height,
                    name,
                    |t| t.texture,
                )?;
                let depth = depth_textures(ring.depth_textures, ring.depth_texture_count, textures, name)?;
                if textures.iter().chain(depth).any(|texture| texture.texture.is_null()) {
                    return Err(Error::invalid_argument(format!("{name} textures must not be null")));
                }
                #[cfg(target_vendor = "apple")]
                {
                    let pointers: Vec<*mut c_void> = textures.iter().map(|texture| texture.texture).collect();
                    let depth: Vec<*mut c_void> = depth.iter().map(|texture| texture.texture).collect();
                    let physical = Size::new(ring.physical_width, ring.physical_height);
                    // SAFETY: the host's textures, which the header's contract
                    // makes `id<MTLTexture>`s.
                    match mln_mtl::BorrowedTextures::from_raw(&pointers, &depth, physical) {
                        Some(textures) => Target::Borrowed { size, ring: BorrowedRing::metal(textures) },
                        None => return Err(Error::invalid_argument(format!("{name} textures must not be null"))),
                    }
                }
                #[cfg(not(target_vendor = "apple"))]
                Target::Unsupported("Metal textures are not supported by this build")
            }
            MLN_RENDER_TARGET_KIND_VULKAN_BORROWED_TEXTURES => {
                vulkan_ring(&target.data.vulkan_borrowed_textures, name)?
            }
            MLN_RENDER_TARGET_KIND_OPEN_GL_BORROWED_TEXTURES => {
                let ring = target.data.open_gl_borrowed_textures;
                let textures = ring_textures(
                    ring.textures,
                    ring.texture_count,
                    ring.physical_width,
                    ring.physical_height,
                    name,
                    |t| t.texture,
                )?;
                let depth = depth_textures(ring.depth_textures, ring.depth_texture_count, textures, name)?;
                if textures.iter().chain(depth).any(|texture| texture.texture == 0) {
                    return Err(Error::invalid_argument(format!("{name} textures must not be zero")));
                }
                // GL_TEXTURE_2D.
                if ring.target != 0x0DE1 {
                    return Err(Error::invalid_argument(format!("{} must be GL_TEXTURE_2D", field("target"))));
                }
                #[cfg(any(windows, all(unix, not(target_vendor = "apple"))))]
                {
                    let textures = mln_gl::BorrowedTextures {
                        textures: textures.iter().map(|texture| texture.texture).collect(),
                        size: Size::new(ring.physical_width, ring.physical_height),
                        depth_textures: depth.iter().map(|texture| texture.texture).collect(),
                    };
                    Target::Borrowed { size, ring: BorrowedRing::gl(textures) }
                }
                #[cfg(not(any(windows, all(unix, not(target_vendor = "apple")))))]
                Target::Unsupported("OpenGL textures of the host's are not supported by this build")
            }
            MLN_RENDER_TARGET_KIND_WEB_GPU_BORROWED_TEXTURES => {
                let ring = target.data.web_gpu_borrowed_textures;
                let textures = ring_textures(
                    ring.textures,
                    ring.texture_count,
                    ring.physical_width,
                    ring.physical_height,
                    name,
                    |t| t.texture,
                )?;
                if textures.iter().any(|texture| texture.texture.is_null() || texture.texture_view.is_null()) {
                    return Err(Error::invalid_argument(format!("{name} textures need a texture and a view")));
                }
                if ring.format == 0 {
                    return Err(Error::invalid_argument(format!("{} must not be zero", field("format"))));
                }
                Target::Unsupported("WebGPU textures are not supported by this build")
            }
            MLN_RENDER_TARGET_KIND_EXPORTED_RING => exported_ring(size, &target.data.exported_ring, name)?,
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

/// The export kinds of a set of `mln_gpu_export_kind_flag` bits.
pub(crate) fn export_kinds(flags: crate::generated::types::mln_gpu_export_kind_flags) -> ExportKinds {
    use crate::generated::types::{
        MLN_GPU_EXPORT_KIND_FLAG_ANDROID_HARDWARE_BUFFER, MLN_GPU_EXPORT_KIND_FLAG_DMA_BUF,
        MLN_GPU_EXPORT_KIND_FLAG_DXGI_SHARED_HANDLE, MLN_GPU_EXPORT_KIND_FLAG_IO_SURFACE,
        MLN_GPU_EXPORT_KIND_FLAG_OPAQUE_FD,
    };
    [
        (MLN_GPU_EXPORT_KIND_FLAG_IO_SURFACE, ExportKind::IoSurface),
        (MLN_GPU_EXPORT_KIND_FLAG_DMA_BUF, ExportKind::DmaBuf),
        (MLN_GPU_EXPORT_KIND_FLAG_ANDROID_HARDWARE_BUFFER, ExportKind::AndroidHardwareBuffer),
        (MLN_GPU_EXPORT_KIND_FLAG_DXGI_SHARED_HANDLE, ExportKind::DxgiSharedHandle),
        (MLN_GPU_EXPORT_KIND_FLAG_OPAQUE_FD, ExportKind::OpaqueFd),
    ]
    .into_iter()
    .filter(|(flag, _)| flags & flag != 0)
    .fold(ExportKinds::NONE, |kinds, (_, kind)| kinds.with(kind))
}

/// The `mln_gpu_export_kind_flag` bits of `kinds`.
pub(crate) fn export_flags(kinds: ExportKinds) -> crate::generated::types::mln_gpu_export_kind_flags {
    kinds.iter().map(|kind| 1 << (export_kind_value(kind) - 1)).fold(0, |flags, bit| flags | bit)
}

/// The `mln_gpu_export_kind` of `kind`.
pub(crate) fn export_kind_value(kind: ExportKind) -> crate::generated::types::mln_gpu_export_kind {
    use crate::generated::types::{
        MLN_GPU_EXPORT_KIND_ANDROID_HARDWARE_BUFFER, MLN_GPU_EXPORT_KIND_DMA_BUF,
        MLN_GPU_EXPORT_KIND_DXGI_SHARED_HANDLE, MLN_GPU_EXPORT_KIND_IO_SURFACE, MLN_GPU_EXPORT_KIND_OPAQUE_FD,
    };
    match kind {
        ExportKind::IoSurface => MLN_GPU_EXPORT_KIND_IO_SURFACE,
        ExportKind::DmaBuf => MLN_GPU_EXPORT_KIND_DMA_BUF,
        ExportKind::AndroidHardwareBuffer => MLN_GPU_EXPORT_KIND_ANDROID_HARDWARE_BUFFER,
        ExportKind::DxgiSharedHandle => MLN_GPU_EXPORT_KIND_DXGI_SHARED_HANDLE,
        ExportKind::OpaqueFd => MLN_GPU_EXPORT_KIND_OPAQUE_FD,
    }
}

/// A caller's exported ring.
///
/// # Safety
///
/// `ring.dma_buf_formats` is null or points to `dma_buf_format_count`
/// readable entries.
unsafe fn exported_ring(
    size: Size,
    ring: &crate::generated::types::mln_exported_ring_target,
    name: &str,
) -> Result<Target, Error> {
    use crate::generated::types::{MLN_GPU_EXPORT_KIND_FLAG_DMA_BUF, MLN_TEXTURE_RING_MAX_DEPTH};
    if ring.texture_count == 0 || ring.texture_count > MLN_TEXTURE_RING_MAX_DEPTH {
        return Err(Error::invalid_argument(format!(
            "{name}.texture_count must be from 1 to MLN_TEXTURE_RING_MAX_DEPTH"
        )));
    }
    let kinds = export_kinds(ring.export_kinds);
    if ring.export_kinds == 0 || export_flags(kinds) != ring.export_kinds {
        return Err(Error::invalid_argument(format!("{name}.export_kinds must name known kinds of image")));
    }
    let formats = if ring.dma_buf_format_count == 0 {
        &[][..]
    } else if ring.dma_buf_formats.is_null() {
        return Err(Error::invalid_argument(format!("{name}.dma_buf_formats must not be null")));
    } else {
        // SAFETY: the caller's contract.
        unsafe { core::slice::from_raw_parts(ring.dma_buf_formats, ring.dma_buf_format_count) }
    };
    if ring.export_kinds & MLN_GPU_EXPORT_KIND_FLAG_DMA_BUF != 0 && formats.is_empty() {
        return Err(Error::invalid_argument(format!("{name} needs dma_buf_formats to export dmabufs")));
    }
    let ring = ExportedRing {
        depth: ring.texture_count as usize,
        kinds,
        dma_buf_formats: formats
            .iter()
            .map(|format| DmaBufFormat { fourcc: format.fourcc, modifier: format.modifier })
            .collect(),
        android_hardware_buffer_usage: ring.android_hardware_buffer_usage,
        keep_depth: false,
    };
    Ok(Target::Exported { size, ring })
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
        if options.keep_depth && !matches!(target, Target::Exported { .. }) {
            return Err(unsupported("only an exported ring keeps depth for the host"));
        }
        let target = match target {
            Target::Offscreen(size) => {
                options.require_core_worker("an offscreen session renders on a core worker")?;
                RenderTarget::Offscreen { size, device }
            }
            Target::Surface { size, kind, surface } => {
                let presents =
                    device.as_ref().map_or(kind.presents_without_device(), |device| device.presents_to(kind));
                if !presents {
                    return Err(unsupported(format!("the device presents to no {kind:?} surface")));
                }
                // A layer is the host's to present to from its own thread,
                // and a window the library's.
                if kind != SurfaceType::MetalLayer {
                    options.require_core_worker("a window session renders on a core worker")?;
                }
                RenderTarget::Surface { size, surface, device }
            }
            Target::HostSurface { size, surface } => {
                let Some(device) = device.filter(|device| {
                    device.presents_to_host_surfaces() && surface.host_backend() == Some(device.backend())
                }) else {
                    return Err(unsupported("only a device of the surface's instance or share group presents to it"));
                };
                RenderTarget::Surface { size, surface, device: Some(device) }
            }
            Target::Borrowed { size, ring } => {
                let Some(device) = device else {
                    return Err(unsupported("a borrowed ring renders on the device of its textures"));
                };
                if !ring.takes_depth_on(&device) {
                    return Err(unsupported("the device draws no depth into textures of the ring's depth format"));
                }
                RenderTarget::Borrowed { size, ring, device }
            }
            Target::Exported { size, mut ring } => {
                options.require_core_worker("an exported ring renders on a library thread, which owns its images")?;
                ring.keep_depth = options.keep_depth;
                RenderTarget::Exported { size, ring, device }
            }
            Target::Unsupported(message) => return Err(unsupported(message)),
        };
        let attached = render_session::attach(&map, target, &options);
        // The request's references to the wakes go before the completion can
        // run, which leaves the session's as the last ones.
        drop(options);
        let (handle, attached) = attached?;
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
        if let Target::Exported { .. } = target {
            return Err(unsupported("an exported ring is no replacement target: resize the session instead"));
        }
        // SAFETY: the caller's contract.
        let pending = unsafe { PendingCompletion::optional(completion) }?;
        let object = render_session::resolve(session)?;
        let scale_factor = object.session.snapshot()?.extent.scale_factor;
        if let Some(size) = target.size() {
            physical_size(mln_logical_extent { width: size.width, height: size.height, scale_factor })?;
        }
        let replaced = match target {
            Target::Surface { size, kind, surface } => {
                if !kind.presents_with_any() {
                    return Err(unsupported(format!("this build presents to no {kind:?} surface")));
                }
                object.session.set_surface(size, surface)?
            }
            Target::HostSurface { size, surface } => object.session.set_surface(size, surface)?,
            Target::Borrowed { size, ring } => object.session.set_ring(size, ring)?,
            Target::Exported { .. } => {
                return Err(unsupported("an exported ring is no replacement target: resize the session instead"));
            }
            Target::Unsupported(message) => return Err(unsupported(message)),
            Target::Offscreen(_) => return Err(unsupported("an offscreen target replaces no window")),
        };
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

/// `mln_open_gl_supported_context_provider_mask()`: EGL on Linux, FreeBSD,
/// Android and `OpenHarmony`, and WGL on Windows.
#[unsafe(no_mangle)]
pub extern "C" fn mln_open_gl_supported_context_provider_mask() -> mln_open_gl_context_provider_flags {
    if cfg!(windows) {
        crate::generated::types::MLN_OPEN_GL_CONTEXT_PROVIDER_FLAG_WGL
    } else if cfg!(all(unix, not(target_vendor = "apple"))) {
        crate::generated::types::MLN_OPEN_GL_CONTEXT_PROVIDER_FLAG_EGL
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::types::{MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_OK};

    fn call(width: u32, height: u32, scale_factor: f64) -> (mln_status, u32, u32) {
        let (mut out_width, mut out_height) = (7, 7);
        let extent = mln_logical_extent { width, height, scale_factor };
        // SAFETY: local outputs and a null diagnostic.
        let status = unsafe {
            mln_logical_extent_physical_size(extent, &raw mut out_width, &raw mut out_height, core::ptr::null_mut())
        };
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
            mln_logical_extent_physical_size(extent, core::ptr::null_mut(), &raw mut height, core::ptr::null_mut())
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
        #[cfg(not(target_os = "android"))]
        assert!(matches!(read(&android(fake), None), Ok(Target::Unsupported(_))));
        assert_eq!(read(&android(core::ptr::null_mut()), None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));

        // OpenHarmony takes only its own windows: see the OpenHarmony test.
        #[cfg(not(target_env = "ohos"))]
        assert!(matches!(read(&ohos(8, 8, fake), None), Ok(Target::Unsupported(_))));
        assert_eq!(read(&ohos(8, 8, core::ptr::null_mut()), None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));
    }

    /// An `OpenHarmony` target of `window`, `width` by `height` logical pixels.
    fn ohos(width: u32, height: u32, window: *mut c_void) -> mln_render_target {
        use crate::generated::types::{mln_ohos_window_target, mln_render_target_data};
        let logical_size = mln_logical_size { width, height };
        let data = mln_render_target_data { ohos_window: mln_ohos_window_target { logical_size, window } };
        target(MLN_RENDER_TARGET_KIND_OHOS_WINDOW, data)
    }

    // An OpenHarmony window, of a consumer surface that nothing shows: the
    // target takes a reference of its own, which outlives the host's, and
    // presents on each backend that a device of the library's has, through
    // the C API. The window has no buffer size of its own, so the session
    // gives it the frames'. Once the consumer goes away, the session's
    // frames find no window.
    #[cfg(target_env = "ohos")]
    #[test]
    fn an_ohos_window_target_holds_its_window_and_presents_on_each_backend() {
        use std::os::fd::FromRawFd as _;
        use std::time::{Duration, Instant};

        use crate::generated::types::{
            MLN_FRAME_DEMAND_FORCE, MLN_GPU_DEVICE_OPTION_BACKEND, MLN_RENDER_RESULT_RENDERED,
            MLN_RENDER_RESULT_TARGET_NOT_READY, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION,
            MLN_RENDER_TARGET_KIND_FLAG_OHOS_WINDOW, mln_gpu_device_capabilities, mln_gpu_device_options,
            mln_render_session_capabilities,
        };
        use crate::gpu_device::{mln_gpu_device_create, mln_gpu_device_get_capabilities};
        use crate::map::mln_map_set_style_json;
        use crate::map::tests::{create_map, options, release_map, view};
        use crate::render_session::mln_render_session_get_capabilities;
        use crate::render_session::tests::{attach_options, demand, release_session, sized, wait_for_result};
        use crate::runtime::tests::{Waiter, create, release};

        /// `OH_NativeBuffer_Config`.
        #[repr(C)]
        #[derive(Default)]
        struct BufferConfig {
            width: i32,
            height: i32,
            format: i32,
            usage: i32,
            stride: i32,
        }
        #[repr(C)]
        struct PollFd {
            fd: i32,
            events: i16,
            revents: i16,
        }
        #[link(name = "native_image")]
        unsafe extern "C" {
            fn OH_ConsumerSurface_Create() -> *mut c_void;
            fn OH_ConsumerSurface_SetDefaultUsage(image: *mut c_void, usage: u64) -> i32;
            fn OH_NativeImage_AcquireNativeWindow(image: *mut c_void) -> *mut c_void;
            fn OH_NativeImage_AcquireNativeWindowBuffer(
                image: *mut c_void,
                buffer: *mut *mut c_void,
                fence: *mut i32,
            ) -> i32;
            fn OH_NativeImage_ReleaseNativeWindowBuffer(image: *mut c_void, buffer: *mut c_void, fence: i32) -> i32;
            fn OH_NativeImage_Destroy(image: *mut *mut c_void);
        }
        #[link(name = "native_buffer")]
        unsafe extern "C" {
            fn OH_NativeBuffer_FromNativeWindowBuffer(buffer: *mut c_void, out: *mut *mut c_void) -> i32;
            fn OH_NativeBuffer_GetConfig(buffer: *mut c_void, config: *mut BufferConfig);
            fn OH_NativeBuffer_Map(buffer: *mut c_void, data: *mut *mut c_void) -> i32;
            fn OH_NativeBuffer_Unmap(buffer: *mut c_void) -> i32;
        }
        unsafe extern "C" {
            fn poll(fds: *mut PollFd, count: core::ffi::c_ulong, timeout: i32) -> i32;
        }
        const USAGE_CPU_READ: u64 = 1;
        const USAGE_HW_RENDER: u64 = 1 << 8;
        const POLLIN: i16 = 1;
        // `OH_NativeBuffer_Format`'s 8-bit RGB orders, with or without alpha.
        const RGBX_8888: i32 = 11;
        const RGBA_8888: i32 = 12;
        const BGRX_8888: i32 = 19;
        const BGRA_8888: i32 = 20;

        /// The size and the center pixel, as RGB, of the newest frame that
        /// `image` has, which comes within a few seconds of its present.
        ///
        /// # Safety
        ///
        /// `image` is a live consumer surface.
        unsafe fn newest_frame(image: *mut c_void) -> ((i32, i32), [u8; 3]) {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut newest = None;
            while newest.is_none() || Instant::now() < deadline {
                let (mut buffer, mut fence) = (core::ptr::null_mut(), -1);
                // SAFETY: the live image, and local outputs.
                if unsafe { OH_NativeImage_AcquireNativeWindowBuffer(image, &mut buffer, &mut fence) } != 0 {
                    if newest.is_some() {
                        break;
                    }
                    assert!(Instant::now() < deadline, "the consumer got no frame");
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                if fence >= 0 {
                    let mut wait = PollFd { fd: fence, events: POLLIN, revents: 0 };
                    // SAFETY: one local pollfd, then the fence that is the
                    // test's to close.
                    unsafe {
                        poll(&mut wait, 1, -1);
                        drop(std::os::fd::OwnedFd::from_raw_fd(fence));
                    }
                }
                let (mut native, mut data, mut config) =
                    (core::ptr::null_mut(), core::ptr::null_mut(), BufferConfig::default());
                // SAFETY: the acquired buffer, until its release.
                unsafe {
                    assert_eq!(OH_NativeBuffer_FromNativeWindowBuffer(buffer, &mut native), 0);
                    OH_NativeBuffer_GetConfig(native, &mut config);
                    assert_eq!(OH_NativeBuffer_Map(native, &mut data), 0, "the frame maps for the CPU");
                }
                let center = (config.height / 2) as usize * config.stride as usize + (config.width / 2) as usize * 4;
                // SAFETY: the mapped buffer holds `height` rows of `stride`
                // bytes.
                let [first, green, third, _] = unsafe { data.cast::<[u8; 4]>().byte_add(center).read() };
                let rgb = match config.format {
                    RGBA_8888 | RGBX_8888 => [first, green, third],
                    BGRA_8888 | BGRX_8888 => [third, green, first],
                    format => panic!("the frame has 8-bit RGB pixels, not format {format}"),
                };
                // SAFETY: the mapped, acquired buffer, which the consumer
                // takes back.
                unsafe {
                    OH_NativeBuffer_Unmap(native);
                    OH_NativeImage_ReleaseNativeWindowBuffer(image, buffer, -1);
                }
                newest = Some(((config.width, config.height), rgb));
            }
            newest.expect("a frame")
        }

        // Memory that is no native object, which the target turns down.
        let mut not_a_window = [0u64; 8];
        let not_a_window = not_a_window.as_mut_ptr().cast::<c_void>();
        assert_eq!(read(&ohos(8, 8, not_a_window), None).err(), Some(MLN_STATUS_INVALID_ARGUMENT));

        let runtime = create();
        let map = create_map(runtime, &options(1.0));
        let styled = Waiter::new();
        let style = r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"red"}}]}"#;
        // SAFETY: local arguments.
        unsafe { mln_map_set_style_json(map, view(style), &styled.descriptor, core::ptr::null_mut()) };
        assert_eq!(styled.status(), MLN_STATUS_OK);

        let mut presented = 0;
        for (index, (backend, value, _)) in gpu_device::BACKENDS.iter().enumerate() {
            let mut device_options = sized::<mln_gpu_device_options>();
            device_options.fields = MLN_GPU_DEVICE_OPTION_BACKEND;
            device_options.backend = *value;
            let mut device = 0;
            // SAFETY: local arguments.
            if unsafe { mln_gpu_device_create(&device_options, &mut device, core::ptr::null_mut()) } != MLN_STATUS_OK {
                continue;
            }
            let mut capabilities = sized::<mln_gpu_device_capabilities>();
            // SAFETY: local arguments.
            unsafe { mln_gpu_device_get_capabilities(device, &mut capabilities, core::ptr::null_mut()) };
            if capabilities.target_kinds & MLN_RENDER_TARGET_KIND_FLAG_OHOS_WINDOW == 0 {
                gpu_device::mln_gpu_device_release(device);
                continue;
            }
            // SAFETY: OpenHarmony's calls, whose image the test destroys.
            let mut image = unsafe { OH_ConsumerSurface_Create() };
            assert!(!image.is_null(), "a consumer surface");
            // SAFETY: as above, of the live image.
            let window = unsafe {
                assert_eq!(OH_ConsumerSurface_SetDefaultUsage(image, USAGE_CPU_READ | USAGE_HW_RENDER), 0);
                OH_NativeImage_AcquireNativeWindow(image)
            };
            assert!(!window.is_null(), "the consumer surface's window");

            let (target, attach) = (ohos(48, 40, window), attach_options());
            let (mut session, attached) = (0, Waiter::new());
            // SAFETY: local arguments.
            let status = unsafe {
                mln_map_attach(map, &target, device, &attach, &mut session, &attached.descriptor, core::ptr::null_mut())
            };
            gpu_device::mln_gpu_device_release(device);
            assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK), "{backend:?}");
            let mut session_capabilities = sized::<mln_render_session_capabilities>();
            // SAFETY: local capabilities.
            unsafe { mln_render_session_get_capabilities(session, &mut session_capabilities, core::ptr::null_mut()) };
            assert_eq!(session_capabilities.flags, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION);
            let token = 10 * index as u64;
            assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token + 1), MLN_STATUS_OK);
            assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED, "{backend:?}");
            // SAFETY: the live image.
            assert_eq!(unsafe { newest_frame(image) }, ((48, 40), [0xff, 0, 0]), "{backend:?}");

            // A frame may still present to the window as its consumer goes,
            // and the next finds no window.
            // SAFETY: the image is the test's. The session holds the window.
            unsafe { OH_NativeImage_Destroy(&mut image) };
            assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token + 2), MLN_STATUS_OK);
            let first = wait_for_result(session).disposition;
            assert!([MLN_RENDER_RESULT_RENDERED, MLN_RENDER_RESULT_TARGET_NOT_READY].contains(&first), "{backend:?}");
            assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token + 3), MLN_STATUS_OK);
            assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_TARGET_NOT_READY, "{backend:?}");
            release_session(session);
            presented += 1;
        }
        assert!(presented > 0, "no backend presents to an OpenHarmony window");

        release_map(map);
        release(runtime);
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
        unsafe { mln_map_set_style_json(map, view(style), &raw const styled.descriptor, core::ptr::null_mut()) };
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
                let status =
                    unsafe { mln_gpu_device_create(&raw const options, &raw mut device, core::ptr::null_mut()) };
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
                    &raw const target,
                    device,
                    &raw const options,
                    &raw mut session,
                    &raw const attached.descriptor,
                    core::ptr::null_mut(),
                )
            };
            gpu_device::mln_gpu_device_release(device);
            assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK), "device {index}");
            let mut capabilities = sized::<mln_render_session_capabilities>();
            // SAFETY: local capabilities.
            unsafe { mln_render_session_get_capabilities(session, &raw mut capabilities, core::ptr::null_mut()) };
            assert_eq!(capabilities.flags, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION);
            rendered(session, token + 1);

            let (target, moved) = (xlib(&second, 80, 40), Waiter::new());
            // SAFETY: local arguments.
            let status = unsafe {
                mln_render_session_set_target(
                    session,
                    &raw const target,
                    &raw const moved.descriptor,
                    core::ptr::null_mut(),
                )
            };
            assert_eq!((status, moved.status()), (MLN_STATUS_OK, MLN_STATUS_OK), "device {index}");
            assert_eq!(rendered(session, token + 2).view.extent.width, 80);

            let extent = mln_logical_extent { width: 40, height: 20, scale_factor: 2.0 };
            let resized = Waiter::new();
            // SAFETY: local arguments.
            let status = unsafe {
                mln_render_session_resize(session, extent, &raw const resized.descriptor, core::ptr::null_mut())
            };
            assert_eq!((status, resized.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
            assert_eq!(rendered(session, token + 3).view.extent.scale_factor, 2.0);
            assert_eq!(snapshot(session).map(|snapshot| snapshot.extent.scale_factor), Ok(2.0));
            release_session(session);
        }

        release_map(map);
        release(runtime);
    }

    #[test]
    fn each_platform_has_one_open_gl_provider() {
        let egl = crate::generated::types::MLN_OPEN_GL_CONTEXT_PROVIDER_FLAG_EGL;
        let wgl = crate::generated::types::MLN_OPEN_GL_CONTEXT_PROVIDER_FLAG_WGL;
        let expected = if cfg!(windows) {
            wgl
        } else if cfg!(all(unix, not(target_vendor = "apple"))) {
            egl
        } else {
            0
        };
        assert_eq!(mln_open_gl_supported_context_provider_mask(), expected);
    }
}
