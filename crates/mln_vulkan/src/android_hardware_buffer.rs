// The Android hardware buffers that exported rings render into. Android 8.0
// (API level 26) added AHardwareBuffer to libnativewindow, so its functions
// are looked up at run time, and the library still loads on older versions.

use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::sync::OnceLock;

use mln_util::error::Error;

/// `AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM`.
const FORMAT_R8G8B8A8_UNORM: u32 = 1;
/// `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE`.
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
/// `AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT`.
const USAGE_GPU_COLOR_OUTPUT: u64 = 1 << 9;

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

struct Functions {
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
                let library = libc::dlopen(c"libnativewindow.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
                if library.is_null() {
                    return None;
                }
                let allocate = libc::dlsym(library, c"AHardwareBuffer_allocate".as_ptr());
                let release = libc::dlsym(library, c"AHardwareBuffer_release".as_ptr());
                if allocate.is_null() || release.is_null() {
                    return None;
                }
                Some(Functions {
                    allocate: std::mem::transmute::<*mut c_void, Allocate>(allocate),
                    release: std::mem::transmute::<*mut c_void, Release>(release),
                })
            }
        })
        .as_ref()
}

/// A reference to a hardware buffer, which goes with it.
pub(crate) struct HardwareBuffer {
    buffer: NonNull<c_void>,
    release: Release,
}

// SAFETY: a hardware buffer's references may be released on any thread.
unsafe impl Send for HardwareBuffer {}

impl HardwareBuffer {
    /// A buffer of `width` by `height` RGBA8888 pixels that the GPU renders
    /// into and samples, with the extra `usage` bits.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) before API
    /// level 26, and with [`ErrorKind::Native`](mln_util::ErrorKind) when
    /// Android allocates no such buffer.
    pub(crate) fn allocate(width: u32, height: u32, usage: u64) -> Result<HardwareBuffer, Error> {
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
            Some(buffer) if result == 0 => Ok(HardwareBuffer { buffer, release: functions.release }),
            _ => Err(Error::native(format!(
                "Allocating a {width}x{height} Android hardware buffer of usage {:#x} failed: {result}",
                desc.usage
            ))),
        }
    }

    /// The `AHardwareBuffer*`.
    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.buffer.as_ptr()
    }
}

impl Drop for HardwareBuffer {
    fn drop(&mut self) {
        // SAFETY: the reference is this one's.
        unsafe { (self.release)(self.buffer.as_ptr()) };
    }
}
