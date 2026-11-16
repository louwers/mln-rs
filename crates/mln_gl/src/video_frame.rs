// Video frames in platform buffers as `GL_TEXTURE_EXTERNAL_OES` textures,
// through EGLImages: Android hardware buffers (EGL_ANDROID_get_native_client_buffer
// and EGL_ANDROID_image_native_buffer) and OpenHarmony native buffers
// (EGL_OHOS_image_native_buffer). An external texture samples a YUV buffer
// as RGB, converted as the driver knows the buffer's format, and the video
// frame shader samples it with `samplerExternalOES`
// (GL_OES_EGL_image_external_essl3). The functions are looked up at run time,
// as exported_target/hardware_buffer.rs looks them up.

use std::ffi::c_void;
use std::sync::OnceLock;

use glow::HasContext;
use khronos_egl as egl;
use mln_gfx::gpu::GpuError;

use crate::types::{Gl, TextureID};

/// `GL_TEXTURE_EXTERNAL_OES`.
pub(crate) const TEXTURE_EXTERNAL: u32 = 0x8D65;
/// `EGL_IMAGE_PRESERVED_KHR`.
const IMAGE_PRESERVED: egl::Int = 0x30D2;

type CreateImage =
    unsafe extern "system" fn(*mut c_void, *mut c_void, egl::Enum, *mut c_void, *const egl::Int) -> *mut c_void;
type DestroyImage = unsafe extern "system" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type ImageTargetTexture = unsafe extern "system" fn(u32, *mut c_void);

/// What makes `EGLImage`s and textures over them.
struct ImageFunctions {
    create_image: CreateImage,
    destroy_image: DestroyImage,
    image_target_texture: ImageTargetTexture,
}

/// The `EGLImage` functions, or why EGL has none.
fn image_functions() -> Result<&'static ImageFunctions, String> {
    static FUNCTIONS: OnceLock<Result<ImageFunctions, String>> = OnceLock::new();
    FUNCTIONS
        .get_or_init(|| {
            let egl = crate::egl::library()?;
            let lookup = |name: &str| {
                egl.get_proc_address(name).map(|function| function as *const c_void).ok_or(format!("EGL has no {name}"))
            };
            // SAFETY: each function has its extension's type.
            unsafe {
                Ok(ImageFunctions {
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

/// An `EGLImage`, destroyed when dropped, and then what it was made of: the
/// textures over it keep its storage.
pub(crate) struct Image {
    display: *mut c_void,
    image: *mut c_void,
    destroy: DestroyImage,
    /// What the image reads, which goes after it.
    #[cfg(target_env = "ohos")]
    _source: Option<ohos::WindowBuffer>,
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: the image is the display's, and this one's.
        unsafe { (self.destroy)(self.display, self.image) };
    }
}

/// What an import checks first: the external sampler, the current
/// display's `extensions`, and the functions. Returns the display and the
/// functions.
fn prepare(
    gl: &Gl,
    extensions: &[&str],
    unsupported: impl Fn(String) -> GpuError,
) -> Result<(egl::Display, &'static ImageFunctions), GpuError> {
    if !gl.supported_extensions().contains("GL_OES_EGL_image_external_essl3") {
        return Err(unsupported("OpenGL ES has no GL_OES_EGL_image_external_essl3".to_owned()));
    }
    let functions = image_functions().map_err(&unsupported)?;
    let egl = crate::egl::library().map_err(&unsupported)?;
    let display = egl.get_current_display().ok_or_else(|| unsupported("no EGL display is current".to_owned()))?;
    let available = egl.query_string(Some(display), egl::EXTENSIONS).map_err(|error| unsupported(error.to_string()))?;
    let available = available.to_string_lossy();
    for extension in extensions {
        if !available.split(' ').any(|name| name == *extension) {
            return Err(unsupported(format!("EGL has no {extension}")));
        }
    }
    Ok((display, functions))
}

/// An external texture over `image`, bound to the active unit's external
/// target. Call it with the context current.
fn external_texture(
    gl: &Gl,
    image: &Image,
    functions: &ImageFunctions,
    unsupported: impl Fn(String) -> GpuError,
) -> Result<TextureID, GpuError> {
    // SAFETY: the context is current, and the texture is new; the image is
    // alive.
    unsafe {
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
            return Err(unsupported(format!("OpenGL ES samples no such image: {error:#x}")));
        }
        Ok(texture)
    }
}

#[cfg(target_os = "android")]
pub(crate) use android::import;
#[cfg(target_env = "ohos")]
pub(crate) use ohos::import;

#[cfg(target_os = "android")]
mod android {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use khronos_egl as egl;
    use mln_gfx::gpu::GpuError;
    use mln_util::size::Size;

    use super::{IMAGE_PRESERVED, Image, external_texture, prepare};
    use crate::types::{Gl, TextureID};

    /// `EGL_NATIVE_BUFFER_ANDROID`.
    const NATIVE_BUFFER_ANDROID: egl::Enum = 0x3140;

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

    /// What describes hardware buffers and makes EGL's client buffers of
    /// them.
    struct Functions {
        describe: Describe,
        client_buffer: GetNativeClientBuffer,
    }

    /// The functions, or why there are none: before API level 26, or an EGL
    /// without the extension. libnativewindow stays loaded for the rest of
    /// the process.
    fn functions() -> Result<&'static Functions, String> {
        static FUNCTIONS: OnceLock<Result<Functions, String>> = OnceLock::new();
        FUNCTIONS
            .get_or_init(|| {
                let egl = crate::egl::library()?;
                let client_buffer = egl
                    .get_proc_address("eglGetNativeClientBufferANDROID")
                    .ok_or("EGL has no eglGetNativeClientBufferANDROID")?;
                // SAFETY: libnativewindow is a system library, whose
                // initializers any process may run, and each function has
                // its extension's or the NDK's type.
                unsafe {
                    let library = libloading::Library::new("libnativewindow.so").map_err(|error| error.to_string())?;
                    let describe: Describe =
                        *library.get(b"AHardwareBuffer_describe\0").map_err(|error| error.to_string())?;
                    std::mem::forget(library);
                    Ok(Functions {
                        describe,
                        client_buffer: std::mem::transmute::<extern "system" fn(), GetNativeClientBuffer>(
                            client_buffer,
                        ),
                    })
                }
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    fn unsupported(why: impl std::fmt::Display) -> GpuError {
        GpuError::Unsupported(format!("importing Android hardware buffer video frames: {why}"))
    }

    /// An external texture over `buffer`, an `AHardwareBuffer*`, and the
    /// image it samples, which goes after the texture; and the buffer's
    /// size. Call it with the context current; the texture stays bound to
    /// the active unit's external target.
    pub(crate) fn import(gl: &Gl, buffer: *mut c_void) -> Result<(TextureID, Image, Size), GpuError> {
        let extensions = ["EGL_ANDROID_image_native_buffer", "EGL_ANDROID_get_native_client_buffer"];
        let (display, images) = prepare(gl, &extensions, unsupported)?;
        let functions = functions().map_err(unsupported)?;
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
        // SAFETY: the client buffer of the live hardware buffer, which the
        // image takes a reference to.
        let image = unsafe {
            (images.create_image)(
                display.as_ptr(),
                std::ptr::null_mut(),
                NATIVE_BUFFER_ANDROID,
                client,
                attributes.as_ptr(),
            )
        };
        if image.is_null() {
            let error = crate::egl::library().map_err(unsupported)?.get_error();
            return Err(unsupported(format!(
                "EGL makes no image of a hardware buffer of format {:#x}: {error:?}",
                desc.format
            )));
        }
        let image = Image { display: display.as_ptr(), image, destroy: images.destroy_image };
        let what = |why: String| unsupported(format!("a hardware buffer of format {:#x}: {why}", desc.format));
        let texture = external_texture(gl, &image, images, what)?;
        Ok((texture, image, Size::new(desc.width, desc.height)))
    }
}

#[cfg(target_env = "ohos")]
mod ohos {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use khronos_egl as egl;
    use mln_gfx::gpu::GpuError;
    use mln_util::size::Size;

    use super::{IMAGE_PRESERVED, Image, external_texture, prepare};
    use crate::types::{Gl, TextureID};

    /// `EGL_NATIVE_BUFFER_OHOS`.
    const NATIVE_BUFFER_OHOS: egl::Enum = 0x34E1;

    /// `OH_NativeBuffer_Config`.
    #[repr(C)]
    #[derive(Default)]
    struct Config {
        width: i32,
        height: i32,
        format: i32,
        usage: i32,
        stride: i32,
    }

    type GetConfig = unsafe extern "C" fn(*mut c_void, *mut Config);
    type WindowBufferOf = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type DestroyWindowBuffer = unsafe extern "C" fn(*mut c_void);

    /// What describes native buffers and makes the window buffers that
    /// EGL takes of them.
    struct Functions {
        config: GetConfig,
        window_buffer: WindowBufferOf,
        destroy_window_buffer: DestroyWindowBuffer,
    }

    /// The functions, or why there are none. `libnative_buffer` and
    /// `libnative_window` stay loaded for the rest of the process.
    fn functions() -> Result<&'static Functions, String> {
        static FUNCTIONS: OnceLock<Result<Functions, String>> = OnceLock::new();
        FUNCTIONS
            .get_or_init(|| {
                // SAFETY: system libraries, whose initializers any process
                // may run, and each function has the SDK's type.
                unsafe {
                    let buffer = libloading::Library::new("libnative_buffer.so").map_err(|error| error.to_string())?;
                    let window = libloading::Library::new("libnative_window.so").map_err(|error| error.to_string())?;
                    let functions = Functions {
                        config: *buffer.get(b"OH_NativeBuffer_GetConfig\0").map_err(|error| error.to_string())?,
                        window_buffer: *window
                            .get(b"OH_NativeWindow_CreateNativeWindowBufferFromNativeBuffer\0")
                            .map_err(|error| error.to_string())?,
                        destroy_window_buffer: *window
                            .get(b"OH_NativeWindow_DestroyNativeWindowBuffer\0")
                            .map_err(|error| error.to_string())?,
                    };
                    std::mem::forget(buffer);
                    std::mem::forget(window);
                    Ok(functions)
                }
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// An `OHNativeWindowBuffer` of a native buffer, destroyed when dropped.
    pub(crate) struct WindowBuffer {
        raw: NonNull<c_void>,
        destroy: DestroyWindowBuffer,
    }

    impl Drop for WindowBuffer {
        fn drop(&mut self) {
            // SAFETY: the window buffer is this one's, and no image reads it
            // any more.
            unsafe { (self.destroy)(self.raw.as_ptr()) };
        }
    }

    fn unsupported(why: impl std::fmt::Display) -> GpuError {
        GpuError::Unsupported(format!("importing OpenHarmony native buffer video frames: {why}"))
    }

    /// An external texture over `buffer`, an `OH_NativeBuffer*`, and the
    /// image it samples, which goes after the texture; and the buffer's
    /// size. Call it with the context current; the texture stays bound to
    /// the active unit's external target.
    pub(crate) fn import(gl: &Gl, buffer: *mut c_void) -> Result<(TextureID, Image, Size), GpuError> {
        let (display, images) = prepare(gl, &["EGL_OHOS_image_native_buffer"], unsupported)?;
        let functions = functions().map_err(unsupported)?;
        let mut config = Config::default();
        // SAFETY: the frame's buffer, which it holds a reference on; the
        // window buffer is this function's.
        let window_buffer = unsafe {
            (functions.config)(buffer, &raw mut config);
            NonNull::new((functions.window_buffer)(buffer))
        };
        let window_buffer = window_buffer.ok_or_else(|| unsupported("the native buffer has no window buffer"))?;
        let window_buffer = WindowBuffer { raw: window_buffer, destroy: functions.destroy_window_buffer };
        let attributes = [IMAGE_PRESERVED, egl::TRUE as egl::Int, egl::NONE];
        // SAFETY: the window buffer of the live native buffer, which
        // outlives the image.
        let image = unsafe {
            (images.create_image)(
                display.as_ptr(),
                std::ptr::null_mut(),
                NATIVE_BUFFER_OHOS,
                window_buffer.raw.as_ptr(),
                attributes.as_ptr(),
            )
        };
        if image.is_null() {
            let error = crate::egl::library().map_err(unsupported)?.get_error();
            return Err(unsupported(format!(
                "EGL makes no image of a native buffer of format {:#x}: {error:?}",
                config.format
            )));
        }
        let image =
            Image { display: display.as_ptr(), image, destroy: images.destroy_image, _source: Some(window_buffer) };
        let what = |why: String| unsupported(format!("a native buffer of format {:#x}: {why}", config.format));
        let texture = external_texture(gl, &image, images, what)?;
        let size = Size::new(config.width.max(0) as u32, config.height.max(0) as u32);
        Ok((texture, image, size))
    }
}
