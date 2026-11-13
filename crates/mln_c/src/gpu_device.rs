//! `gpu_device.h`: the GPU devices that render sessions render on.

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use ::mln_runtime::{Adapter, BackendType, Device, DeviceOptions, GlApi, SurfaceType, SyncKind, SyncKinds};
use mln_util::ErrorKind;

use crate::abi::{self, Fields};
use crate::buffer;
use crate::callback::UserData;
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
    mln_gpu_device_options, mln_gpu_device_vulkan_context_result, mln_gpu_sync_kind_flags, mln_metal_device_descriptor,
    mln_open_gl_context_descriptor, mln_queue_lock, mln_render_backend, mln_render_backend_flags,
    mln_render_target_kind_flags, mln_status, mln_user_data_release, mln_vulkan_device_descriptor,
    mln_web_gpu_context_descriptor,
};
use crate::handles::{self, HandleKind, HandleTable};

abi::versioned!(
    mln_gpu_device_options,
    mln_gpu_device_capabilities,
    mln_vulkan_device_descriptor,
    mln_metal_device_descriptor,
    mln_gpu_device_vulkan_context_result
);

static DEVICES: HandleTable<Device> = HandleTable::new(HandleKind::GpuDevice);

/// The backends that this build draws with on this platform, each with its
/// `mln_render_backend` and its flag.
pub(crate) const BACKENDS: &[(BackendType, mln_render_backend, mln_render_backend_flags)] = &[
    #[cfg(target_vendor = "apple")]
    (BackendType::Metal, types::MLN_RENDER_BACKEND_METAL, types::MLN_RENDER_BACKEND_FLAG_METAL),
    #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
    (BackendType::Vulkan, MLN_RENDER_BACKEND_VULKAN, types::MLN_RENDER_BACKEND_FLAG_VULKAN),
    // OpenGL comes through EGL, and on Windows through WGL.
    #[cfg(any(windows, all(unix, not(target_vendor = "apple"))))]
    (BackendType::OpenGL, MLN_RENDER_BACKEND_OPEN_GL, types::MLN_RENDER_BACKEND_FLAG_OPEN_GL),
    #[cfg(not(target_family = "wasm"))]
    (BackendType::WebGPU, MLN_RENDER_BACKEND_WEB_GPU, types::MLN_RENDER_BACKEND_FLAG_WEB_GPU),
    // A browser build has one of the two.
    #[cfg(all(target_family = "wasm", feature = "webgpu"))]
    (BackendType::WebGPU, MLN_RENDER_BACKEND_WEB_GPU, types::MLN_RENDER_BACKEND_FLAG_WEB_GPU),
    #[cfg(all(target_family = "wasm", feature = "gl", not(feature = "webgpu")))]
    (BackendType::OpenGL, MLN_RENDER_BACKEND_OPEN_GL, types::MLN_RENDER_BACKEND_FLAG_OPEN_GL),
];

/// The window and layer kinds that this library takes, each with its
/// target kind flag.
pub(crate) const SURFACE_KINDS: &[(SurfaceType, mln_render_target_kind_flags)] = &[
    (SurfaceType::MetalLayer, MLN_RENDER_TARGET_KIND_FLAG_METAL_LAYER),
    (SurfaceType::Win32, MLN_RENDER_TARGET_KIND_FLAG_WIN32_WINDOW),
    (SurfaceType::Xlib, MLN_RENDER_TARGET_KIND_FLAG_XLIB_WINDOW),
    (SurfaceType::Wayland, MLN_RENDER_TARGET_KIND_FLAG_WAYLAND_SURFACE),
    (SurfaceType::AndroidNdk, types::MLN_RENDER_TARGET_KIND_FLAG_ANDROID_WINDOW),
    (SurfaceType::OhosNdk, types::MLN_RENDER_TARGET_KIND_FLAG_OHOS_WINDOW),
];

/// `mln_supported_render_backend_mask()`'s value.
pub(crate) fn supported_backend_mask() -> mln_render_backend_flags {
    BACKENDS.iter().fold(0, |mask, (_, _, flag)| mask | flag)
}

/// The device that `device` names, or `None` for the null handle.
pub(crate) fn resolve(device: mln_gpu_device) -> Result<Option<Device>, Error> {
    if device == handles::NULL { Ok(None) } else { Ok(Some(Device::clone(&*DEVICES.get(device)?))) }
}

/// The callbacks run inside the queue's mutex, which a session on the same
/// queue takes again to submit.
#[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
const QUEUE_LOCK_REENTRY: &str = "the C API cannot be called from a queue lock callback";

/// A host's `mln_queue_lock`, which a Vulkan device holds around its queue
/// calls. Once accepted, it releases its user data when the device goes.
pub(crate) struct HostQueueLock {
    // Only a Vulkan device calls them; other builds still check the lock.
    #[cfg_attr(any(target_vendor = "apple", target_family = "wasm"), allow(dead_code))]
    lock: unsafe extern "C" fn(*mut c_void),
    #[cfg_attr(any(target_vendor = "apple", target_family = "wasm"), allow(dead_code))]
    unlock: unsafe extern "C" fn(*mut c_void),
    user_data: UserData,
    release: mln_user_data_release,
    accepted: AtomicBool,
}

impl HostQueueLock {
    /// Checks a caller's queue lock: both callbacks or neither, and no user
    /// data to release without them. `None` for no lock.
    pub(crate) fn new(lock: &mln_queue_lock) -> Result<Option<HostQueueLock>, Error> {
        match (lock.lock, lock.unlock) {
            (Some(lock_fn), Some(unlock_fn)) => Ok(Some(HostQueueLock {
                lock: lock_fn,
                unlock: unlock_fn,
                user_data: UserData::new(lock.user_data),
                release: lock.release_user_data,
                accepted: AtomicBool::new(false),
            })),
            (None, None) if lock.release_user_data.is_some() => {
                Err(Error::invalid_argument("a disabled queue lock must not retain user data"))
            }
            (None, None) => Ok(None),
            _ => Err(Error::invalid_argument("a queue lock needs both its lock and unlock callbacks")),
        }
    }

    /// Takes the user data: dropping the lock from here on releases it.
    pub(crate) fn accept(&self) {
        self.accepted.store(true, Ordering::Release);
    }
}

#[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
impl mln_vulkan::QueueLock for HostQueueLock {
    fn lock(&self) {
        let _forbid = crate::export::ForbidReentry::enter(QUEUE_LOCK_REENTRY);
        // SAFETY: the host's callback, with its user data, as the header
        // promises.
        unsafe { (self.lock)(self.user_data.get()) }
    }

    fn unlock(&self) {
        let _forbid = crate::export::ForbidReentry::enter(QUEUE_LOCK_REENTRY);
        // SAFETY: as for `lock`.
        unsafe { (self.unlock)(self.user_data.get()) }
    }
}

impl std::fmt::Debug for HostQueueLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostQueueLock").finish_non_exhaustive()
    }
}

impl Drop for HostQueueLock {
    fn drop(&mut self) {
        if *self.accepted.get_mut()
            && let Some(release) = self.release
        {
            // SAFETY: the host's release, called once, after the last call.
            unsafe { release(self.user_data.get()) }
        }
    }
}

/// What a caller's `mln_gpu_device_options` ask for.
#[derive(Debug, Default)]
struct Request {
    backend: Option<BackendType>,
    options: DeviceOptions,
    client_api: Option<u32>,
    single_context: Option<bool>,
    queue_lock: Option<HostQueueLock>,
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
    let gl_api = match client_api {
        None => GlApi::default(),
        Some(MLN_OPEN_GL_CLIENT_API_GLES) => GlApi::Gles,
        Some(MLN_OPEN_GL_CLIENT_API_GL) => GlApi::Gl,
        Some(_) => {
            return Err(Error::invalid_argument("mln_gpu_device_options.open_gl_client_api must name an OpenGL API"));
        }
    };
    let single_context = fields.get(MLN_GPU_DEVICE_OPTION_OPEN_GL_SINGLE_CONTEXT, options.open_gl_single_context);
    Ok(Request {
        backend,
        options: DeviceOptions { adapter: adapters.pop(), gl_api, gl_single_context: single_context.unwrap_or(false) },
        client_api,
        single_context,
        queue_lock: HostQueueLock::new(&options.queue_lock)?,
    })
}

/// Fails with `MLN_STATUS_UNSUPPORTED` for OpenGL options that a device of
/// `backend` can't take: only an OpenGL device reads them. The OpenGL device
/// itself fails to create where it can't render with the client API.
fn check_open_gl_options(request: &Request, backend: BackendType) -> Result<(), Error> {
    if (request.client_api.is_some() || request.single_context.is_some()) && backend != BackendType::OpenGL {
        return Err(Error::new(ErrorKind::Unsupported, "only an OpenGL device takes OpenGL options"));
    }
    Ok(())
}

/// The device that `request` asks for, and the queue lock it took, which
/// takes its user data once the device has a handle.
fn create(request: Request) -> Result<(Device, Option<std::sync::Arc<HostQueueLock>>), Error> {
    if cfg!(target_family = "wasm") {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "the browser makes WebGPU devices asynchronously, and WebGL2 contexts on canvases: attach without a \
             device, or wrap your WebGL2 context",
        ));
    }
    if let Some(backend) = request.backend {
        if !BACKENDS.iter().any(|(built, _, _)| *built == backend) {
            return Err(Error::new(ErrorKind::Unsupported, format!("this build draws with no {backend:?}")));
        }
        check_open_gl_options(&request, backend)?;
        if request.queue_lock.is_some() && backend != BackendType::Vulkan {
            return Err(Error::new(ErrorKind::Unsupported, "only a Vulkan device takes a queue lock"));
        }
    }
    let device = match request.backend {
        Some(backend) => Device::with_backend(backend, request.options)?,
        None => Device::new(request.options)?,
    };
    check_open_gl_options(&request, device.backend())?;
    match (request.queue_lock, device) {
        (None, device) => Ok((device, None)),
        #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
        (Some(lock), Device::Vulkan(device)) => {
            let lock = std::sync::Arc::new(lock);
            let device = device.with_queue_lock(lock.clone())?;
            Ok((Device::Vulkan(device), Some(lock)))
        }
        (Some(_), _) => Err(Error::new(ErrorKind::Unsupported, "only a Vulkan device takes a queue lock")),
    }
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
        let (device, lock) = create(request)?;
        *out_device = DEVICES.insert(device.into())?;
        if let Some(lock) = lock {
            lock.accept();
        }
        Ok(())
    })
}

/// The `mln_gpu_sync_kind` of each kind.
pub(crate) const SYNC_KINDS: &[(SyncKind, u32, mln_gpu_sync_kind_flags)] = &[
    (SyncKind::CpuComplete, types::MLN_GPU_SYNC_CPU_COMPLETE, types::MLN_GPU_SYNC_KIND_FLAG_CPU_COMPLETE),
    (SyncKind::OpenGlFence, types::MLN_GPU_SYNC_OPEN_GL_FENCE, types::MLN_GPU_SYNC_KIND_FLAG_OPEN_GL_FENCE),
    (SyncKind::QueueOrder, types::MLN_GPU_SYNC_QUEUE_ORDER, types::MLN_GPU_SYNC_KIND_FLAG_QUEUE_ORDER),
    (
        SyncKind::VulkanTimelineSemaphore,
        types::MLN_GPU_SYNC_VULKAN_TIMELINE_SEMAPHORE,
        types::MLN_GPU_SYNC_KIND_FLAG_VULKAN_TIMELINE_SEMAPHORE,
    ),
    (SyncKind::SyncFd, types::MLN_GPU_SYNC_SYNC_FD, types::MLN_GPU_SYNC_KIND_FLAG_SYNC_FD),
    (
        SyncKind::MetalSharedEvent,
        types::MLN_GPU_SYNC_METAL_SHARED_EVENT,
        types::MLN_GPU_SYNC_KIND_FLAG_METAL_SHARED_EVENT,
    ),
    (SyncKind::D3d12Fence, types::MLN_GPU_SYNC_D3D12_FENCE, types::MLN_GPU_SYNC_KIND_FLAG_D3D12_FENCE),
];

/// The flags of `kinds`.
pub(crate) fn sync_flags(kinds: SyncKinds) -> mln_gpu_sync_kind_flags {
    SYNC_KINDS.iter().filter(|(kind, _, _)| kinds.contains(*kind)).fold(0, |flags, (_, _, flag)| flags | flag)
}

/// The kinds that `flags` names.
pub(crate) fn sync_kinds(flags: mln_gpu_sync_kind_flags) -> SyncKinds {
    SYNC_KINDS
        .iter()
        .filter(|(_, _, flag)| flags & flag != 0)
        .fold(SyncKinds::NONE, |kinds, (kind, _, _)| kinds.with(*kind))
}

/// The `mln_gpu_sync_kind` of `kind`.
pub(crate) fn sync_kind_value(kind: SyncKind) -> u32 {
    SYNC_KINDS.iter().find(|(known, _, _)| *known == kind).map_or(0, |(_, value, _)| *value)
}

/// The kind that a `mln_gpu_sync_kind` names.
pub(crate) fn sync_kind_of(value: u32) -> Option<SyncKind> {
    SYNC_KINDS.iter().find(|(_, known, _)| *known == value).map(|(kind, _, _)| *kind)
}

/// The capabilities of `device`.
fn capabilities(device: &Device) -> mln_gpu_device_capabilities {
    let backend = device.backend();
    // The browser build renders into canvases only.
    let offscreen = if cfg!(target_family = "wasm") {
        types::MLN_RENDER_TARGET_KIND_FLAG_WEB_CANVAS
    } else {
        MLN_RENDER_TARGET_KIND_FLAG_OFFSCREEN
    };
    let mut target_kinds = SURFACE_KINDS
        .iter()
        .filter(|(surface, _)| device.presents_to(*surface))
        .fold(offscreen, |kinds, (_, flag)| kinds | flag);
    let mut sync = SyncKinds::NONE;
    #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
    if let Device::Vulkan(vulkan) = device {
        if vulkan.presents() {
            target_kinds |= types::MLN_RENDER_TARGET_KIND_FLAG_VULKAN_SURFACE;
        }
        target_kinds |= types::MLN_RENDER_TARGET_KIND_FLAG_VULKAN_BORROWED_TEXTURES;
        let (producer, consumer) = vulkan.sync_kinds();
        sync = producer.union(consumer);
    }
    #[cfg(any(windows, all(unix, not(target_vendor = "apple"))))]
    if let Device::Gl(gl) = device
        && gl.is_host()
    {
        target_kinds |= types::MLN_RENDER_TARGET_KIND_FLAG_OPEN_GL_SURFACE
            | types::MLN_RENDER_TARGET_KIND_FLAG_OPEN_GL_BORROWED_TEXTURES;
        let (producer, consumer) = mln_gl::BorrowedTextures::sync_kinds();
        sync = producer.union(consumer);
    }
    #[cfg(target_vendor = "apple")]
    if let Device::Metal(metal) = device {
        target_kinds |= types::MLN_RENDER_TARGET_KIND_FLAG_METAL_BORROWED_TEXTURES;
        let (producer, consumer) = mln_mtl::BorrowedTextures::sync_kinds(metal);
        sync = producer.union(consumer);
    }
    let exports = device.export_kinds();
    if !exports.is_empty() {
        target_kinds |= types::MLN_RENDER_TARGET_KIND_FLAG_EXPORTED_RING;
        let (producer, consumer) = device.export_sync_kinds();
        sync = sync.union(producer).union(consumer);
    }
    mln_gpu_device_capabilities {
        size: size_of::<mln_gpu_device_capabilities>() as u32,
        backend: BACKENDS.iter().find(|(built, _, _)| *built == backend).map_or(0, |(_, value, _)| *value),
        target_kinds,
        export_kinds: crate::render_target::export_flags(exports),
        sync_kinds: sync_flags(sync),
        depth_stencil_formats: depth_stencil_flags(device.depth_stencil_formats()),
        depth_export_kinds: crate::render_target::export_flags(device.depth_export_kinds()),
    }
}

/// The `mln_depth_stencil_format_flags` of `formats`.
fn depth_stencil_flags(formats: ::mln_runtime::DepthStencilFormats) -> types::mln_depth_stencil_format_flags {
    formats.iter().fold(0, |flags, format| {
        flags
            | match format {
                ::mln_runtime::DepthStencilFormat::Depth24Stencil8 => {
                    types::MLN_DEPTH_STENCIL_FORMAT_FLAG_DEPTH24_STENCIL8
                }
                ::mln_runtime::DepthStencilFormat::Depth32FloatStencil8 => {
                    types::MLN_DEPTH_STENCIL_FORMAT_FLAG_DEPTH32_FLOAT_STENCIL8
                }
            }
    })
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
/// can't wrap a `GPUDevice` yet (louwers/mln-rs#263).
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
        if context.device == 0 {
            return Err(Error::invalid_argument("mln_web_gpu_context_descriptor.device must not be 0"));
        }
        // SAFETY: the caller's contract.
        unsafe { handles::out_handle(out_device, "out_device") }?;
        Err(Error::new(
            ErrorKind::Unsupported,
            "this library's WebGPU can't wrap a GPUDevice yet (https://github.com/louwers/mln-rs/issues/263)",
        ))
    })
}

/// `mln_gpu_device_wrap_vulkan()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_gpu_device_wrap_vulkan(
    descriptor: *const mln_vulkan_device_descriptor,
    out_device: *mut mln_gpu_device,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        let descriptor = unsafe { abi::read_sized(descriptor) }?;
        let context = descriptor.context;
        let name = "mln_vulkan_device_descriptor.context";
        for (pointer, field) in [
            (context.instance, "instance"),
            (context.physical_device, "physical_device"),
            (context.device, "device"),
            (context.graphics_queue, "graphics_queue"),
        ] {
            if pointer.is_null() {
                return Err(Error::invalid_argument(format!("{name}.{field} must not be null")));
            }
        }
        // SAFETY: the caller's contract.
        let enabled_extensions =
            unsafe { vulkan_extensions(descriptor.enabled_extensions, descriptor.enabled_extension_count) }?;
        let lock = HostQueueLock::new(&descriptor.queue_lock)?;
        // SAFETY: the caller's contract.
        let out_device = unsafe { handles::out_handle(out_device, "out_device") }?;
        #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
        {
            use mln_vulkan::ash::vk::{self, Handle as _};
            // SAFETY: function pointers that the header names, or null.
            let (get_instance_proc_addr, get_device_proc_addr) = unsafe {
                (
                    core::mem::transmute::<*mut c_void, Option<vk::PFN_vkGetInstanceProcAddr>>(
                        context.get_instance_proc_addr,
                    ),
                    core::mem::transmute::<*mut c_void, Option<vk::PFN_vkGetDeviceProcAddr>>(
                        context.get_device_proc_addr,
                    ),
                )
            };
            let lock = lock.map(std::sync::Arc::new);
            let queue_lock = lock.clone().map(|lock| lock as std::sync::Arc<dyn mln_vulkan::QueueLock>);
            let host = mln_vulkan::HostDevice {
                get_instance_proc_addr,
                get_device_proc_addr,
                instance: vk::Instance::from_raw(context.instance as u64),
                physical_device: vk::PhysicalDevice::from_raw(context.physical_device as u64),
                device: vk::Device::from_raw(context.device as u64),
                queue_family_index: context.graphics_queue_family_index,
                queue: vk::Queue::from_raw(context.graphics_queue as u64),
                queue_lock,
                enabled_extensions,
            };
            // SAFETY: the host's live objects, which outlive the device and
            // its sessions (the header's contract).
            let device = unsafe { mln_vulkan::Device::from_raw(host) }?;
            *out_device = DEVICES.insert(Device::Vulkan(device).into())?;
            // The user data is the caller's until the call succeeds.
            if let Some(lock) = lock {
                lock.accept();
            }
            Ok(())
        }
        #[cfg(any(target_vendor = "apple", target_family = "wasm"))]
        {
            let _ = (lock, out_device, enabled_extensions);
            Err(Error::new(ErrorKind::Unsupported, "this build draws with no Vulkan"))
        }
    })
}

/// The names of the device extensions that a host lists: `count` views at
/// `extensions`, each the text of a name.
///
/// # Safety
///
/// `extensions` is null or points to `count` readable views, whose data is
/// readable.
unsafe fn vulkan_extensions(
    extensions: *const crate::generated::types::mln_buffer_view,
    count: usize,
) -> Result<Vec<std::ffi::CString>, Error> {
    let name = "mln_vulkan_device_descriptor.enabled_extensions";
    if count == 0 {
        return Ok(Vec::new());
    }
    if extensions.is_null() {
        return Err(Error::invalid_argument(format!("{name} must not be null")));
    }
    // SAFETY: the caller's contract.
    let views = unsafe { core::slice::from_raw_parts(extensions, count) };
    views
        .iter()
        .map(|view| {
            // SAFETY: the caller's contract.
            let text = unsafe { crate::buffer::text(*view, name) }?;
            std::ffi::CString::new(text)
                .map_err(|_| Error::invalid_argument(format!("{name} must not hold a NUL character")))
        })
        .collect()
}

/// `mln_gpu_device_wrap_metal()`.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_gpu_device_wrap_metal(
    descriptor: *const mln_metal_device_descriptor,
    out_device: *mut mln_gpu_device,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        let descriptor = unsafe { abi::read_sized(descriptor) }?;
        if descriptor.device.is_null() {
            return Err(Error::invalid_argument("mln_metal_device_descriptor.device must not be null"));
        }
        // SAFETY: the caller's contract.
        let out_device = unsafe { handles::out_handle(out_device, "out_device") }?;
        #[cfg(target_vendor = "apple")]
        {
            // SAFETY: the host's device and queue, which the device retains.
            let device = unsafe { mln_mtl::Device::from_raw(descriptor.device, descriptor.command_queue) }?;
            *out_device = DEVICES.insert(Device::Metal(device).into())?;
            Ok(())
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            let _ = out_device;
            Err(Error::new(ErrorKind::Unsupported, "Metal devices are not supported by this build"))
        }
    })
}

/// `mln_gpu_device_wrap_open_gl()`: the share group of the host's EGL
/// context, of OpenGL ES 3, or of desktop OpenGL on Linux and FreeBSD.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_gpu_device_wrap_open_gl(
    context: *const mln_open_gl_context_descriptor,
    out_device: *mut mln_gpu_device,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        // SAFETY: the caller's contract.
        let context = unsafe { context.as_ref() }
            .ok_or_else(|| Error::invalid_argument("mln_open_gl_context_descriptor must not be null"))?;
        let platform = open_gl_platform(context)?;
        // SAFETY: the caller's contract.
        let out_device = unsafe { handles::out_handle(out_device, "out_device") }?;
        #[cfg(all(unix, not(target_vendor = "apple")))]
        if context.platform == types::MLN_OPEN_GL_CONTEXT_PLATFORM_EGL {
            // SAFETY: the variant that `platform` names, checked above.
            let egl = unsafe { context.data.egl };
            let pointer =
                |pointer: *mut c_void| core::ptr::NonNull::new(pointer).ok_or_else(|| Error::invalid_argument("null"));
            // SAFETY: the host's live display, config and context (the
            // header's contract).
            let device = unsafe {
                mln_gl::Device::from_raw(pointer(egl.display)?, pointer(egl.config)?, pointer(egl.share_context)?)
            }?;
            *out_device = DEVICES.insert(Device::Gl(device).into())?;
            return Ok(());
        }
        #[cfg(all(target_family = "wasm", feature = "gl"))]
        if context.platform == types::MLN_OPEN_GL_CONTEXT_PLATFORM_WEB_GL {
            // SAFETY: the variant that `platform` names, checked above.
            let handle = unsafe { context.data.web_gl.context };
            let context = crate::web::required(handle, "mln_open_gl_context_descriptor.data.web_gl.context")?;
            *out_device = DEVICES.insert(Device::web_gl(context)?.into())?;
            return Ok(());
        }
        #[cfg(windows)]
        if context.platform == types::MLN_OPEN_GL_CONTEXT_PLATFORM_WGL {
            // SAFETY: the variant that `platform` names, checked above.
            let wgl = unsafe { context.data.wgl };
            let pointer =
                |pointer: *mut c_void| core::ptr::NonNull::new(pointer).ok_or_else(|| Error::invalid_argument("null"));
            // SAFETY: the host's live device context and context, and its
            // `wglGetProcAddress` or null (the header's contract).
            let device = unsafe {
                mln_gl::Device::from_wgl(
                    pointer(wgl.device_context)?,
                    pointer(wgl.share_context)?,
                    wgl.get_proc_address,
                )
            }?;
            *out_device = DEVICES.insert(Device::Gl(device).into())?;
            return Ok(());
        }
        let _ = out_device;
        Err(Error::new(ErrorKind::Unsupported, format!("{platform} contexts are not supported by this build")))
    })
}

/// Checks a caller's OpenGL context: a platform the header names, with the
/// handles it needs. Returns the platform's name.
fn open_gl_platform(context: &mln_open_gl_context_descriptor) -> Result<&'static str, Error> {
    let null = |field: &str| {
        Err(Error::invalid_argument(format!("mln_open_gl_context_descriptor.data.{field} must not be null")))
    };
    // SAFETY: each arm reads the variant that `platform` names.
    unsafe {
        match context.platform {
            types::MLN_OPEN_GL_CONTEXT_PLATFORM_EGL => {
                let egl = context.data.egl;
                if egl.display.is_null() {
                    return null("egl.display");
                }
                if egl.config.is_null() {
                    return null("egl.config");
                }
                if egl.share_context.is_null() {
                    return null("egl.share_context");
                }
                Ok("EGL")
            }
            types::MLN_OPEN_GL_CONTEXT_PLATFORM_WGL => {
                let wgl = context.data.wgl;
                if wgl.device_context.is_null() {
                    return null("wgl.device_context");
                }
                if wgl.share_context.is_null() {
                    return null("wgl.share_context");
                }
                Ok("WGL")
            }
            types::MLN_OPEN_GL_CONTEXT_PLATFORM_WEB_GL => {
                if context.data.web_gl.context == 0 {
                    return null("web_gl.context");
                }
                Ok("WebGL")
            }
            _ => Err(Error::invalid_argument("mln_open_gl_context_descriptor.platform is invalid")),
        }
    }
}

/// `mln_gpu_device_get_vulkan_context()`.
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
        match &*device {
            #[cfg(not(any(target_vendor = "apple", target_family = "wasm")))]
            Device::Vulkan(vulkan) => {
                use mln_vulkan::ash::vk::Handle as _;
                let raw = vulkan.raw();
                let result = mln_gpu_device_vulkan_context_result {
                    size: size_of::<mln_gpu_device_vulkan_context_result>() as u32,
                    context: types::mln_vulkan_context_descriptor {
                        instance: raw.instance.as_raw() as *mut c_void,
                        physical_device: raw.physical_device.as_raw() as *mut c_void,
                        device: raw.device.as_raw() as *mut c_void,
                        graphics_queue: raw.queue.as_raw() as *mut c_void,
                        graphics_queue_family_index: raw.queue_family_index,
                        get_instance_proc_addr: raw.get_instance_proc_addr as *mut c_void,
                        get_device_proc_addr: raw.get_device_proc_addr as *mut c_void,
                    },
                };
                // SAFETY: the caller's contract.
                unsafe { abi::write_sized(out_result, &result) }
            }
            _ => Err(Error::new(ErrorKind::Unsupported, "the device doesn't render with Vulkan")),
        }
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
    fn open_gl_options_need_an_open_gl_device() {
        let request = |client_api, single_context| Request { client_api, single_context, ..Request::default() };
        let status = |request: &Request, backend| check_open_gl_options(request, backend).map_err(|e| e.status());
        assert_eq!(status(&request(Some(MLN_OPEN_GL_CLIENT_API_GLES), Some(false)), BackendType::OpenGL), Ok(()));
        assert_eq!(status(&request(Some(MLN_OPEN_GL_CLIENT_API_GL), Some(true)), BackendType::OpenGL), Ok(()));
        assert_eq!(
            status(&request(Some(MLN_OPEN_GL_CLIENT_API_GLES), None), BackendType::Vulkan),
            Err(MLN_STATUS_UNSUPPORTED)
        );
        assert_eq!(status(&request(None, Some(false)), BackendType::Vulkan), Err(MLN_STATUS_UNSUPPORTED));
    }

    #[test]
    fn open_gl_options_reach_the_device_options() {
        let mut desktop =
            options(MLN_GPU_DEVICE_OPTION_OPEN_GL_CLIENT_API | MLN_GPU_DEVICE_OPTION_OPEN_GL_SINGLE_CONTEXT);
        desktop.open_gl_client_api = MLN_OPEN_GL_CLIENT_API_GL;
        desktop.open_gl_single_context = true;
        let request = checked(&desktop).unwrap();
        assert_eq!(request.options.gl_api, GlApi::Gl);
        assert!(request.options.gl_single_context);
        assert_eq!(checked(&options(0)).unwrap().options, DeviceOptions::default());
    }

    #[test]
    fn the_mask_names_each_backend_once() {
        let mask = supported_backend_mask();
        assert_eq!(mask.count_ones() as usize, BACKENDS.len());
        let metal = crate::generated::types::MLN_RENDER_BACKEND_FLAG_METAL;
        assert_eq!(mask & metal != 0, cfg!(target_vendor = "apple"));
    }
}
