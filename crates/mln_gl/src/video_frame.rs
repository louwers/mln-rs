// Video frames in Android hardware buffers as `GL_TEXTURE_EXTERNAL_OES`
// textures, through EGLImages (EGL_ANDROID_get_native_client_buffer and
// EGL_ANDROID_image_native_buffer). An external texture samples a YUV buffer
// as RGB, converted as the driver knows the buffer's format, and the video
// frame shader samples it with `samplerExternalOES`
// (GL_OES_EGL_image_external_essl3). The functions are looked up at run time,
// as exported_target/hardware_buffer.rs looks them up.

use std::ffi::c_void;
use std::sync::OnceLock;

use glow::HasContext;
use khronos_egl as egl;
use mln_gfx::gpu::GpuError;
use mln_util::size::Size;

use crate::types::{Gl, TextureID};

/// `GL_TEXTURE_EXTERNAL_OES`.
pub(crate) const TEXTURE_EXTERNAL: u32 = 0x8D65;
/// `EGL_NATIVE_BUFFER_ANDROID`.
const NATIVE_BUFFER_ANDROID: egl::Enum = 0x3140;
/// `EGL_IMAGE_PRESERVED_KHR`.
const IMAGE_PRESERVED: egl::Int = 0x30D2;

/// `AHardwareBuffer_Desc`.
#[repr(C)]
#[derive(Default)]
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

type Describe = unsafe extern "C" fn(*const c_void, *mut Desc);
type GetNativeClientBuffer = unsafe extern "system" fn(*const c_void) -> *mut c_void;
type CreateImage =
    unsafe extern "system" fn(*mut c_void, *mut c_void, egl::Enum, *mut c_void, *const egl::Int) -> *mut c_void;
type DestroyImage = unsafe extern "system" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type ImageTargetTexture = unsafe extern "system" fn(u32, *mut c_void);

/// What imports hardware buffers.
struct Functions {
    describe: Describe,
    client_buffer: GetNativeClientBuffer,
    create_image: CreateImage,
    destroy_image: DestroyImage,
    image_target_texture: ImageTargetTexture,
}

/// The functions, or why there are none: before API level 26, or an EGL
/// without the extensions. libnativewindow stays loaded for the rest of
/// the process.
fn functions() -> Result<&'static Functions, String> {
    static FUNCTIONS: OnceLock<Result<Functions, String>> = OnceLock::new();
    FUNCTIONS
        .get_or_init(|| {
            let egl = crate::egl::library()?;
            let lookup = |name: &str| {
                egl.get_proc_address(name).map(|function| function as *const c_void).ok_or(format!("EGL has no {name}"))
            };
            // SAFETY: libnativewindow is a system library, whose
            // initializers any process may run, and each function has its
            // extension's or the NDK's type.
            unsafe {
                let library = libloading::Library::new("libnativewindow.so").map_err(|error| error.to_string())?;
                let describe: Describe =
                    *library.get(b"AHardwareBuffer_describe\0").map_err(|error| error.to_string())?;
                std::mem::forget(library);
                Ok(Functions {
                    describe,
                    client_buffer: std::mem::transmute::<*const c_void, GetNativeClientBuffer>(lookup(
                        "eglGetNativeClientBufferANDROID",
                    )?),
                    create_image: std::mem::transmute::<*const c_void, CreateImage>(lookup("eglCreateImageKHR")?),
                    destroy_image: std::mem::transmute::<*const c_void, DestroyImage>(lookup("eglDestroyImageKHR")?),
                    image_target_texture: std::mem::transmute::<*const c_void, ImageTargetTexture>(lookup(
                        "glEGLImageTargetTexture2DOES",
                    )?),
                })
            }
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// An `EGLImage`, destroyed when dropped: the textures over it keep its
/// storage.
pub(crate) struct Image {
    display: *mut c_void,
    image: *mut c_void,
    destroy: DestroyImage,
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: the image is the display's, and this one's.
        unsafe { (self.destroy)(self.display, self.image) };
    }
}

fn unsupported(why: impl std::fmt::Display) -> GpuError {
    GpuError::Unsupported(format!("importing Android hardware buffer video frames: {why}"))
}

/// An external texture over `buffer`, an `AHardwareBuffer*`, and the image
/// it samples, which goes after the texture; and the buffer's size. Call it
/// with the context current; the texture stays bound to the active unit's
/// external target.
pub(crate) fn import(gl: &Gl, buffer: *mut c_void) -> Result<(TextureID, Image, Size), GpuError> {
    if !gl.supported_extensions().contains("GL_OES_EGL_image_external_essl3") {
        return Err(unsupported("OpenGL ES has no GL_OES_EGL_image_external_essl3"));
    }
    let functions = functions().map_err(unsupported)?;
    let egl = crate::egl::library().map_err(unsupported)?;
    let display = egl.get_current_display().ok_or_else(|| unsupported("no EGL display is current"))?;
    let extensions = egl.query_string(Some(display), egl::EXTENSIONS).map_err(unsupported)?.to_string_lossy();
    let has = |name: &str| extensions.split(' ').any(|extension| extension == name);
    if !has("EGL_ANDROID_image_native_buffer") || !has("EGL_ANDROID_get_native_client_buffer") {
        return Err(unsupported("EGL imports no hardware buffers"));
    }
    let mut desc = Desc::default();
    // SAFETY: the frame's buffer, which it holds a reference on.
    let client = unsafe {
        (functions.describe)(buffer, &raw mut desc);
        (functions.client_buffer)(buffer)
    };
    if client.is_null() {
        return Err(unsupported("EGL has no client buffer of the hardware buffer"));
    }
    let attributes = [IMAGE_PRESERVED, egl::TRUE as egl::Int, egl::NONE];
    // SAFETY: the client buffer of the live hardware buffer, which the image
    // takes a reference to.
    let image = unsafe {
        (functions.create_image)(
            display.as_ptr(),
            std::ptr::null_mut(),
            NATIVE_BUFFER_ANDROID,
            client,
            attributes.as_ptr(),
        )
    };
    if image.is_null() {
        let error = egl.get_error();
        return Err(unsupported(format!(
            "EGL makes no image of a hardware buffer of format {:#x}: {error:?}",
            desc.format
        )));
    }
    let image = Image { display: display.as_ptr(), image, destroy: functions.destroy_image };
    // SAFETY: the context is current, and the texture is new; the image is
    // alive.
    let texture = unsafe {
        let texture = gl.create_texture().map_err(GpuError::Invalid)?;
        gl.bind_texture(TEXTURE_EXTERNAL, Some(texture));
        // Errors of earlier calls aren't the import's.
        for _ in 0..8 {
            if gl.get_error() == glow::NO_ERROR {
                break;
            }
        }
        (functions.image_target_texture)(TEXTURE_EXTERNAL, image.image);
        let error = gl.get_error();
        if error != glow::NO_ERROR {
            gl.delete_texture(texture);
            return Err(unsupported(format!("OpenGL ES samples no image of format {:#x}: {error:#x}", desc.format)));
        }
        texture
    };
    Ok((texture, image, Size::new(desc.width, desc.height)))
}
