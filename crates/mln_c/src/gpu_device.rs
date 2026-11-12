//! `gpu_device.h`: the GPU devices that render sessions render on.

use ::mln_runtime::{Adapter, BackendType, Device, DeviceOptions, SurfaceType};
use mln_util::ErrorKind;

use crate::abi::{self, Fields};
use crate::buffer;
use crate::error::Error;
use crate::generated::types;
use crate::generated::types::{
    MLN_GPU_DEVICE_OPTION_ADAPTER_INDEX, MLN_GPU_DEVICE_OPTION_ADAPTER_LUID, MLN_GPU_DEVICE_OPTION_ADAPTER_UUID,
    MLN_GPU_DEVICE_OPTION_BACKEND, MLN_GPU_DEVICE_OPTION_OPEN_GL_CLIENT_API,
    MLN_GPU_DEVICE_OPTION_OPEN_GL_SINGLE_CONTEXT, MLN_OPEN_GL_CLIENT_API_GL, MLN_OPEN_GL_CLIENT_API_GLES,
    MLN_RENDER_BACKEND_OPEN_GL, MLN_RENDER_BACKEND_VULKAN, MLN_RENDER_BACKEND_WEB_GPU,
    MLN_RENDER_TARGET_KIND_FLAG_METAL_LAYER, MLN_RENDER_TARGET_KIND_FLAG_OFFSCREEN,
    MLN_RENDER_TARGET_KIND_FLAG_WAYLAND_SURFACE, MLN_RENDER_TARGET_KIND_FLAG_WIN32_WINDOW,
    MLN_RENDER_TARGET_KIND_FLAG_XLIB_WINDOW, mln_diagnostic, mln_gpu_device, mln_gpu_device_capabilities,
    mln_gpu_device_options, mln_gpu_device_vulkan_context_result, mln_render_backend, mln_render_backend_flags,
    mln_render_target_kind_flags, mln_status, mln_web_gpu_context_descriptor,
};
use crate::handles::{self, HandleKind, HandleTable};

abi::versioned!(mln_gpu_device_options, mln_gpu_device_capabilities);

static DEVICES: HandleTable<Device> = HandleTable::new(HandleKind::GpuDevice);

/// The backends that this build draws with on this platform, each with its
/// `mln_render_backend` and its flag.
pub(crate) const BACKENDS: &[(BackendType, mln_render_backend, mln_render_backend_flags)] = &[
    #[cfg(target_vendor = "apple")]
    (BackendType::Metal, types::MLN_RENDER_BACKEND_METAL, types::MLN_RENDER_BACKEND_FLAG_METAL),
    #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
    (BackendType::Vulkan, MLN_RENDER_BACKEND_VULKAN, types::MLN_RENDER_BACKEND_FLAG_VULKAN),
    // OpenGL ES comes through EGL.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    (BackendType::OpenGL, MLN_RENDER_BACKEND_OPEN_GL, types::MLN_RENDER_BACKEND_FLAG_OPEN_GL),
    #[cfg(not(target_family = "wasm"))]
    (BackendType::WebGPU, MLN_RENDER_BACKEND_WEB_GPU, types::MLN_RENDER_BACKEND_FLAG_WEB_GPU),
];

/// The window and layer kinds that this library takes, each with its
/// target kind flag.
pub(crate) const SURFACE_KINDS: &[(SurfaceType, mln_render_target_kind_flags)] = &[
    (SurfaceType::MetalLayer, MLN_RENDER_TARGET_KIND_FLAG_METAL_LAYER),
    (SurfaceType::Win32, MLN_RENDER_TARGET_KIND_FLAG_WIN32_WINDOW),
    (SurfaceType::Xlib, MLN_RENDER_TARGET_KIND_FLAG_XLIB_WINDOW),
    (SurfaceType::Wayland, MLN_RENDER_TARGET_KIND_FLAG_WAYLAND_SURFACE),
];

/// `mln_supported_render_backend_mask()`'s value.
pub(crate) fn supported_backend_mask() -> mln_render_backend_flags {
    BACKENDS.iter().fold(0, |mask, (_, _, flag)| mask | flag)
}

/// The device that `device` names, or `None` for the null handle.
pub(crate) fn resolve(device: mln_gpu_device) -> Result<Option<Device>, Error> {
    if device == handles::NULL { Ok(None) } else { Ok(Some(Device::clone(&*DEVICES.get(device)?))) }
}

/// What a caller's `mln_gpu_device_options` ask for.
#[derive(Debug, Default)]
struct Request {
    backend: Option<BackendType>,
    options: DeviceOptions,
    client_api: Option<u32>,
    single_context: Option<bool>,
}

/// A caller's checked `mln_gpu_device_options`.
///
/// # Safety
///
/// `options` is null or points to a readable struct of the size it declares,
/// whose buffer views are readable.
unsafe fn request(options: *const mln_gpu_device_options) -> Result<Request, Error> {
    if options.is_null() {
        return Ok(Request::default());
    }
    // SAFETY: non-null, and the caller's contract.
    let options = unsafe { abi::read_sized(options) }?;
    let fields = Fields::new(
        options.fields,
        MLN_GPU_DEVICE_OPTION_BACKEND
            | MLN_GPU_DEVICE_OPTION_ADAPTER_INDEX
            | MLN_GPU_DEVICE_OPTION_ADAPTER_UUID
            | MLN_GPU_DEVICE_OPTION_ADAPTER_LUID
            | MLN_GPU_DEVICE_OPTION_OPEN_GL_CLIENT_API
            | MLN_GPU_DEVICE_OPTION_OPEN_GL_SINGLE_CONTEXT,
        "mln_gpu_device_options.fields",
    )?;
    let backend = fields
        .get(MLN_GPU_DEVICE_OPTION_BACKEND, options.backend)
        .map(|backend| match backend {
            crate::generated::types::MLN_RENDER_BACKEND_METAL => Ok(BackendType::Metal),
            MLN_RENDER_BACKEND_VULKAN => Ok(BackendType::Vulkan),
            MLN_RENDER_BACKEND_OPEN_GL => Ok(BackendType::OpenGL),
            MLN_RENDER_BACKEND_WEB_GPU => Ok(BackendType::WebGPU),
            _ => Err(Error::invalid_argument("mln_gpu_device_options.backend is invalid")),
        })
        .transpose()?;
    let id = |view, name: &str, size: usize| -> Result<Vec<u8>, Error> {
        // SAFETY: the caller's contract.
        let bytes = unsafe { buffer::bytes(view, name) }?;
        if bytes.len() != size {
            return Err(Error::invalid_argument(format!("{name} must be {size} bytes")));
        }
        Ok(bytes.to_vec())
    };
    let mut adapters = Vec::new();
    if let Some(index) = fields.get(MLN_GPU_DEVICE_OPTION_ADAPTER_INDEX, options.adapter_index) {
        adapters.push(Adapter::Index(index as usize));
    }
    if let Some(view) = fields.get(MLN_GPU_DEVICE_OPTION_ADAPTER_UUID, options.adapter_uuid) {
        let uuid = id(view, "mln_gpu_device_options.adapter_uuid", 16)?;
        adapters.push(Adapter::Uuid(uuid.try_into().unwrap_or_default()));
    }
    if let Some(view) = fields.get(MLN_GPU_DEVICE_OPTION_ADAPTER_LUID, options.adapter_luid) {
        let luid = id(view, "mln_gpu_device_options.adapter_luid", 8)?;
        adapters.push(Adapter::Luid(luid.try_into().unwrap_or_default()));
    }
    if adapters.len() > 1 {
        return Err(Error::invalid_argument("mln_gpu_device_options names the GPU more than once"));
    }
    let client_api = fields.get(MLN_GPU_DEVICE_OPTION_OPEN_GL_CLIENT_API, options.open_gl_client_api);
    if client_api.is_some_and(|api| api != MLN_OPEN_GL_CLIENT_API_GL && api != MLN_OPEN_GL_CLIENT_API_GLES) {
        return Err(Error::invalid_argument("mln_gpu_device_options.open_gl_client_api must name an OpenGL API"));
    }
    Ok(Request {
        backend,
        options: DeviceOptions { adapter: adapters.pop() },
        client_api,
        single_context: fields.get(MLN_GPU_DEVICE_OPTION_OPEN_GL_SINGLE_CONTEXT, options.open_gl_single_context),
    })
}

/// Fails with `MLN_STATUS_UNSUPPORTED` for OpenGL options that a device of
/// `backend` can't take: only an OpenGL device reads them, and this
/// library's renders with OpenGL ES, with a context for each session.
fn check_open_gl_options(request: &Request, backend: BackendType) -> Result<(), Error> {
    let unsupported = |message: &'static str| Err(Error::new(ErrorKind::Unsupported, message));
    if (request.client_api.is_some() || request.single_context.is_some()) && backend != BackendType::OpenGL {
        return unsupported("only an OpenGL device takes OpenGL options");
    }
    if request.client_api == Some(MLN_OPEN_GL_CLIENT_API_GL) {
        return unsupported("this library's OpenGL devices render with OpenGL ES");
    }
    if request.single_context == Some(true) {
        return unsupported("each session on this library's OpenGL devices has a context of its own");
    }
    Ok(())
}

/// The device that `request` asks for.
fn create(request: &Request) -> Result<Device, Error> {
    if let Some(backend) = request.backend {
        if !BACKENDS.iter().any(|(built, _, _)| *built == backend) {
            return Err(Error::new(ErrorKind::Unsupported, format!("this build draws with no {backend:?}")));
        }
        check_open_gl_options(request, backend)?;
    }
    let device = match request.backend {
        Some(backend) => Device::with_backend(backend, request.options)?,
        None => Device::new(request.options)?,
    };
    check_open_gl_options(request, device.backend())?;
    Ok(device)
}

/// `mln_gpu_device_create()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_gpu_device_create(
    options: *const mln_gpu_device_options,
    out_device: *mut mln_gpu_device,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        let request = unsafe { request(options) }?;
        // SAFETY: the caller's contract.
        let out_device = unsafe { handles::out_handle(out_device, "out_device") }?;
        let device = create(&request)?;
        *out_device = DEVICES.insert(device.into())?;
        Ok(())
    })
}

/// The capabilities of `device`. It shares no textures yet, so it reports no
/// export or sync kinds.
fn capabilities(device: &Device) -> mln_gpu_device_capabilities {
    let backend = device.backend();
    let target_kinds = SURFACE_KINDS
        .iter()
        .filter(|(surface, _)| device.presents_to(*surface))
        .fold(MLN_RENDER_TARGET_KIND_FLAG_OFFSCREEN, |kinds, (_, flag)| kinds | flag);
    mln_gpu_device_capabilities {
        size: size_of::<mln_gpu_device_capabilities>() as u32,
        backend: BACKENDS.iter().find(|(built, _, _)| *built == backend).map_or(0, |(_, value, _)| *value),
        target_kinds,
        export_kinds: 0,
        sync_kinds: 0,
    }
}

/// `mln_gpu_device_get_capabilities()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_gpu_device_get_capabilities(
    device: mln_gpu_device,
    out_capabilities: *mut mln_gpu_device_capabilities,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_capabilities), {
        let device = DEVICES.get(device)?;
        // SAFETY: the caller's contract.
        unsafe { abi::write_sized(out_capabilities, &capabilities(&device)) }
    })
}

/// `mln_gpu_device_wrap_web_gpu()`: this library's WebGPU is wgpu's, which
/// takes no `webgpu.h` device.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_gpu_device_wrap_web_gpu(
    context: *const mln_web_gpu_context_descriptor,
    out_device: *mut mln_gpu_device,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        let context = unsafe { context.as_ref() }
            .ok_or_else(|| Error::invalid_argument("mln_web_gpu_context_descriptor must not be null"))?;
        if context.device.is_null() {
            return Err(Error::invalid_argument("mln_web_gpu_context_descriptor.device must not be null"));
        }
        // SAFETY: the caller's contract.
        unsafe { handles::out_handle(out_device, "out_device") }?;
        Err(Error::new(ErrorKind::Unsupported, "this library wraps no WebGPU device outside a browser"))
    })
}

/// `mln_gpu_device_get_vulkan_context()`: a session on this library's
/// Vulkan devices makes its own instance and device on the device's GPU, so
/// a device has none to share.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_gpu_device_get_vulkan_context(
    device: mln_gpu_device,
    out_result: *mut mln_gpu_device_vulkan_context_result,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, non_null(out_result), {
        let device = DEVICES.get(device)?;
        let message = if device.backend() == BackendType::Vulkan {
            "this library's Vulkan devices share no instance or device yet"
        } else {
            "the device doesn't render with Vulkan"
        };
        Err(Error::new(ErrorKind::Unsupported, message))
    })
}

/// `mln_gpu_device_release()`. Sessions on the device hold it until they
/// end.
#[unsafe(no_mangle)]
pub extern "C" fn mln_gpu_device_release(device: mln_gpu_device) {
    if device != handles::NULL {
        let _ = release(device);
    }
}

/// Releases `device`, as `mln_gpu_device_release()` does.
pub(crate) fn release(device: mln_gpu_device) -> Result<(), Error> {
    DEVICES.remove(device).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::types::{MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_UNSUPPORTED, mln_buffer_view};

    fn options(fields: u32) -> mln_gpu_device_options {
        // SAFETY: zeros are a valid `mln_gpu_device_options`.
        let mut options: mln_gpu_device_options = unsafe { core::mem::zeroed() };
        options.size = size_of::<mln_gpu_device_options>() as u32;
        options.fields = fields;
        options
    }

    fn view(bytes: &[u8]) -> mln_buffer_view {
        mln_buffer_view { data: bytes.as_ptr().cast(), size: bytes.len() }
    }

    fn checked(options: &mln_gpu_device_options) -> Result<Request, Error> {
        // SAFETY: local options and buffers.
        unsafe { request(options) }
    }

    #[test]
    fn options_name_a_backend_and_at_most_one_gpu() {
        let uuid = [7u8; 16];
        let mut named = options(MLN_GPU_DEVICE_OPTION_BACKEND | MLN_GPU_DEVICE_OPTION_ADAPTER_UUID);
        named.backend = MLN_RENDER_BACKEND_VULKAN;
        named.adapter_uuid = view(&uuid);
        let request = checked(&named).unwrap();
        assert_eq!(request.backend, Some(BackendType::Vulkan));
        assert_eq!(request.options.adapter, Some(Adapter::Uuid(uuid)));
        // SAFETY: null options.
        assert!(unsafe { super::request(core::ptr::null()) }.unwrap().options.adapter.is_none());

        let invalid = |options: mln_gpu_device_options| checked(&options).unwrap_err().status();
        let mut short = options(MLN_GPU_DEVICE_OPTION_ADAPTER_LUID);
        short.adapter_luid = view(&uuid[..7]);
        assert_eq!(invalid(short), MLN_STATUS_INVALID_ARGUMENT);
        let mut twice = options(MLN_GPU_DEVICE_OPTION_ADAPTER_INDEX | MLN_GPU_DEVICE_OPTION_ADAPTER_UUID);
        twice.adapter_uuid = view(&uuid);
        assert_eq!(invalid(twice), MLN_STATUS_INVALID_ARGUMENT);
        assert_eq!(invalid(options(MLN_GPU_DEVICE_OPTION_BACKEND)), MLN_STATUS_INVALID_ARGUMENT);
        assert_eq!(invalid(options(MLN_GPU_DEVICE_OPTION_OPEN_GL_CLIENT_API)), MLN_STATUS_INVALID_ARGUMENT);
        assert_eq!(invalid(options(1 << 31)), MLN_STATUS_INVALID_ARGUMENT);
    }

    #[test]
    fn open_gl_options_need_an_open_gl_es_device_with_a_context_each() {
        let request = |client_api, single_context| Request { client_api, single_context, ..Request::default() };
        let status = |request: &Request, backend| check_open_gl_options(request, backend).map_err(|e| e.status());
        assert_eq!(status(&request(Some(MLN_OPEN_GL_CLIENT_API_GLES), Some(false)), BackendType::OpenGL), Ok(()));
        assert_eq!(
            status(&request(Some(MLN_OPEN_GL_CLIENT_API_GLES), None), BackendType::Vulkan),
            Err(MLN_STATUS_UNSUPPORTED)
        );
        assert_eq!(
            status(&request(Some(MLN_OPEN_GL_CLIENT_API_GL), None), BackendType::OpenGL),
            Err(MLN_STATUS_UNSUPPORTED)
        );
        assert_eq!(status(&request(None, Some(true)), BackendType::OpenGL), Err(MLN_STATUS_UNSUPPORTED));
    }

    #[test]
    fn the_mask_names_each_backend_once() {
        let mask = supported_backend_mask();
        assert_eq!(mask.count_ones() as usize, BACKENDS.len());
        let metal = crate::generated::types::MLN_RENDER_BACKEND_FLAG_METAL;
        assert_eq!(mask & metal != 0, cfg!(target_vendor = "apple"));
    }
}
