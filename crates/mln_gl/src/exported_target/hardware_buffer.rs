// The Android hardware buffers of an exported ring, allocated as
// crates/mln_vulkan/src/android_hardware_buffer.rs allocates them and
// imported as EGLImages (EGL_ANDROID_get_native_client_buffer and
// EGL_ANDROID_image_native_buffer), and the sync files that hand their
// frames over (EGL_ANDROID_native_fence_sync). Android 8.0 (API level 26)
// added AHardwareBuffer to libnativewindow and
// eglGetNativeClientBufferANDROID to EGL, so the functions are looked up at
// run time, and the library still loads on older versions.

use std::ffi::{c_int, c_void};
use std::os::fd::{AsRawFd as _, FromRawFd as _, IntoRawFd as _, OwnedFd};
use std::ptr::NonNull;
use std::sync::OnceLock;

use glow::HasContext;
use khronos_egl as egl;
use mln_gfx::{ExportKind, ExportedImage, ExportedRing};
use mln_util::error::Error;
use mln_util::size::Size;

use super::Images;
use crate::egl::Egl;
use crate::types::Gl;

/// The kind of image that the ring shares.
pub(super) const KIND: ExportKind = ExportKind::AndroidHardwareBuffer;

/// `AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM`.
const FORMAT_R8G8B8A8_UNORM: u32 = 1;
/// `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE`.
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
/// `AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT`.
const USAGE_GPU_COLOR_OUTPUT: u64 = 1 << 9;
/// `EGL_NATIVE_BUFFER_ANDROID`.
const NATIVE_BUFFER_ANDROID: egl::Enum = 0x3140;
/// `EGL_IMAGE_PRESERVED_KHR`.
const IMAGE_PRESERVED: egl::Int = 0x30D2;
/// `EGL_SYNC_NATIVE_FENCE_ANDROID`.
const SYNC_NATIVE_FENCE: egl::Enum = 0x3144;
/// `EGL_SYNC_NATIVE_FENCE_FD_ANDROID`.
const SYNC_NATIVE_FENCE_FD: egl::Int = 0x3145;

#[repr(C)]
struct Desc {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
    rfu0: u32,
    rfu1: u64,
}

type Allocate = unsafe extern "C" fn(*const Desc, *mut *mut c_void) -> c_int;
type Release = unsafe extern "C" fn(*mut c_void);
type GetNativeClientBuffer = unsafe extern "system" fn(*const c_void) -> *mut c_void;
type CreateSync = unsafe extern "system" fn(*mut c_void, egl::Enum, *const egl::Int) -> *mut c_void;
type DestroySync = unsafe extern "system" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type WaitSync = unsafe extern "system" fn(*mut c_void, *mut c_void, egl::Int) -> egl::Int;
type DupNativeFence = unsafe extern "system" fn(*mut c_void, *mut c_void) -> egl::Int;

struct Functions {
    _library: libloading::Library,
    allocate: Allocate,
    release: Release,
}

/// libnativewindow's functions, or `None` before API level 26. The library
/// stays loaded for the rest of the process.
fn functions() -> Option<&'static Functions> {
    static FUNCTIONS: OnceLock<Option<Functions>> = OnceLock::new();
    FUNCTIONS
        .get_or_init(|| {
            // SAFETY: libnativewindow is a system library, whose
            // initializers any process may run, and the symbols have these
            // signatures.
            unsafe {
                let library = libloading::Library::new("libnativewindow.so").ok()?;
                let allocate: Allocate = *library.get(b"AHardwareBuffer_allocate\0").ok()?;
                let release: Release = *library.get(b"AHardwareBuffer_release\0").ok()?;
                Some(Functions { _library: library, allocate, release })
            }
        })
        .as_ref()
}

/// A reference to a hardware buffer, which goes with it.
pub(super) struct Memory {
    buffer: NonNull<c_void>,
    release: Release,
}

impl Memory {
    /// A buffer of `width` by `height` RGBA8888 pixels that the GPU renders
    /// into and samples, with the extra `usage` bits.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) before API
    /// level 26, and with [`ErrorKind::Native`](mln_util::ErrorKind) when
    /// Android allocates no such buffer.
    fn allocate(width: u32, height: u32, usage: u64) -> Result<Memory, Error> {
        let functions =
            functions().ok_or_else(|| Error::unsupported("Android hardware buffers need API level 26 or later"))?;
        let desc = Desc {
            width,
            height,
            layers: 1,
            format: FORMAT_R8G8B8A8_UNORM,
            usage: USAGE_GPU_COLOR_OUTPUT | USAGE_GPU_SAMPLED_IMAGE | usage,
            stride: 0,
            rfu0: 0,
            rfu1: 0,
        };
        let mut buffer = std::ptr::null_mut();
        // SAFETY: a valid description and output.
        let result = unsafe { (functions.allocate)(&raw const desc, &raw mut buffer) };
        match NonNull::new(buffer) {
            Some(buffer) if result == 0 => Ok(Memory { buffer, release: functions.release }),
            _ => Err(Error::native(format!(
                "Allocating a {width}x{height} Android hardware buffer of usage {:#x} failed: {result}",
                desc.usage
            ))),
        }
    }

    /// The buffer, as the host imports it.
    pub(super) fn exported(&self, _layout: &Layout) -> ExportedImage {
        ExportedImage::AndroidHardwareBuffer { buffer: self.buffer.as_ptr() as u64 }
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: the reference is this one's.
        unsafe { (self.release)(self.buffer.as_ptr()) };
    }
}

/// The `AHARDWAREBUFFER_USAGE_*` bits that the host's reads of a ring's
/// buffers need.
#[derive(Clone)]
pub(super) struct Layout {
    usage: u64,
}

/// What makes the display's images of hardware buffers.
pub(super) struct Source {
    client_buffer: GetNativeClientBuffer,
}

impl Source {
    /// The display's source, where it makes images of hardware buffers, or
    /// `None`.
    pub(super) fn load(egl: &'static Egl, _display: egl::Display, has: impl Fn(&str) -> bool) -> Option<Source> {
        if !has("EGL_ANDROID_image_native_buffer") || !has("EGL_ANDROID_get_native_client_buffer") {
            return None;
        }
        let function = egl.get_proc_address("eglGetNativeClientBufferANDROID")? as *const c_void;
        // SAFETY: the function has its extension's type.
        let client_buffer = unsafe { std::mem::transmute::<*const c_void, GetNativeClientBuffer>(function) };
        Some(Source { client_buffer })
    }

    /// A layout to probe the display with: no usage but the GPU's.
    #[expect(clippy::unused_self, clippy::unnecessary_wraps, reason = "as every platform's source probes")]
    pub(super) fn probe_layout(&self) -> Option<Layout> {
        Some(Layout { usage: 0 })
    }

    /// The layout of `ring`'s buffers, where Android allocates one with the
    /// host's usage bits, which it may refuse.
    #[expect(clippy::unused_self, reason = "as every platform's source lays a ring out")]
    pub(super) fn layout(&self, ring: &ExportedRing) -> Option<Layout> {
        let usage = ring.android_hardware_buffer_usage;
        Memory::allocate(1, 1, usage).ok().map(|_| Layout { usage })
    }

    /// A hardware buffer of `size` pixels with the usage bits of `layout`, and
    /// an `EGLImage` over it.
    pub(super) fn allocate(
        &self,
        images: &Images,
        size: Size,
        layout: &Layout,
    ) -> Result<(Memory, *mut c_void), Error> {
        let buffer = Memory::allocate(size.width, size.height, layout.usage)?;
        // SAFETY: the buffer is live, and its client buffer with it.
        let client = unsafe { (self.client_buffer)(buffer.buffer.as_ptr()) };
        if client.is_null() {
            return Err(Error::native("EGL has no client buffer of the hardware buffer"));
        }
        let attributes = [IMAGE_PRESERVED, egl::TRUE as egl::Int, egl::NONE];
        // SAFETY: the client buffer of the live hardware buffer, which the
        // image takes a reference to.
        let image = unsafe { images.create(NATIVE_BUFFER_ANDROID, client, &attributes) }
            .ok_or_else(|| Error::native("EGL can't make an image of the hardware buffer"))?;
        Ok((buffer, image))
    }
}

/// The display's sync files (`EGL_ANDROID_native_fence_sync`), which hand a
/// frame over to the host, and the host's reads back to the context.
pub(super) struct Fences {
    display: *mut c_void,
    create: CreateSync,
    destroy: DestroySync,
    wait: WaitSync,
    dup: DupNativeFence,
}

impl Fences {
    /// The display's sync files, or `None` where it has none or can't have
    /// a context wait for one.
    pub(super) fn load(egl: &'static Egl, display: egl::Display, has: impl Fn(&str) -> bool) -> Option<Fences> {
        if !has("EGL_ANDROID_native_fence_sync") || !has("EGL_KHR_wait_sync") {
            return None;
        }
        let lookup = |name: &str| egl.get_proc_address(name).map(|function| function as *const c_void);
        // SAFETY: each function has its extension's type.
        unsafe {
            Some(Fences {
                display: display.as_ptr(),
                create: std::mem::transmute::<*const c_void, CreateSync>(lookup("eglCreateSyncKHR")?),
                destroy: std::mem::transmute::<*const c_void, DestroySync>(lookup("eglDestroySyncKHR")?),
                wait: std::mem::transmute::<*const c_void, WaitSync>(lookup("eglWaitSyncKHR")?),
                dup: std::mem::transmute::<*const c_void, DupNativeFence>(lookup("eglDupNativeFenceFDANDROID")?),
            })
        }
    }

    /// A sync file that signals once the GPU finishes the current context's
    /// commands so far, which a flush sends it. Call it with a context of
    /// the display current.
    pub(super) fn signal(&self, gl: &Gl) -> Result<OwnedFd, Error> {
        let attributes = [egl::NONE];
        // SAFETY: a native fence of the display, in the current context.
        let sync = unsafe { (self.create)(self.display, SYNC_NATIVE_FENCE, attributes.as_ptr()) };
        if sync.is_null() {
            return Err(Error::native("EGL makes no native fence"));
        }
        // SAFETY: the context is current, and the fence is new; it has a
        // sync file once a flush sends it to the GPU.
        let fd = unsafe {
            gl.flush();
            let fd = (self.dup)(self.display, sync);
            (self.destroy)(self.display, sync);
            fd
        };
        if fd < 0 {
            return Err(Error::native("EGL has no sync file of the native fence"));
        }
        // SAFETY: the duplicate is new, and the caller's.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Has the current context's later commands wait for `file` to signal.
    /// Call it with a context of the display current.
    pub(super) fn wait(&self, file: OwnedFd) -> Result<(), Error> {
        let attributes = [SYNC_NATIVE_FENCE_FD, file.as_raw_fd(), egl::NONE];
        // SAFETY: a native fence of the display over the live sync file.
        let sync = unsafe { (self.create)(self.display, SYNC_NATIVE_FENCE, attributes.as_ptr()) };
        if sync.is_null() {
            return Err(Error::native("EGL makes no native fence of the host's sync file"));
        }
        // The fence owns the sync file now.
        let _ = file.into_raw_fd();
        // SAFETY: the fence is the display's, and the context is current.
        let waited = unsafe {
            let waited = (self.wait)(self.display, sync, 0);
            (self.destroy)(self.display, sync);
            waited
        };
        if waited == egl::FALSE as egl::Int {
            return Err(Error::native("the context can't wait for the host's sync file"));
        }
        Ok(())
    }
}
