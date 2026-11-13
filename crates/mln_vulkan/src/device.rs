// The GPU and queue that Vulkan renderers render on: one `VkDevice` and one
// graphics queue, which every renderer on the device shares, whatever thread
// it renders on. Each renderer has its own command pool and allocator.
//
// The library makes its device with the extensions that its renderers can
// use where the GPU has them: the platforms' surfaces and swapchains,
// timeline semaphores, and sync files. A host's device has the extensions the
// host enabled, which the device finds by looking their functions up.
//
// Queue submission needs external synchronization. Every submission and
// present to a queue holds a mutex of that queue's, which every device on the
// queue shares, then the host's queue lock where the host gave one.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::io::Write as _;
use std::sync::{Arc, LazyLock, OnceLock, Weak};

use ash::vk;
use mln_gfx::{DepthStencilFormat, DepthStencilFormats, ExportKinds, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::logging::{Event, EventSeverity, Log};
use parking_lot::Mutex;

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// The device extension for the storage class naga's SPIR-V declares its
/// storage buffers with, which Vulkan 1.0 doesn't have.
const STORAGE_BUFFER_STORAGE_CLASS: &CStr = c"VK_KHR_storage_buffer_storage_class";

/// `VK_OHOS_surface`, `OpenHarmony`'s surface extension, which ash lacks.
pub const OHOS_SURFACE: &CStr = c"VK_OHOS_surface";

/// The instance extensions of the platforms' surfaces, which the library's
/// instance enables where the loader has them.
const SURFACE_EXTENSIONS: &[&CStr] = &[
    ash::khr::wayland_surface::NAME,
    ash::khr::xlib_surface::NAME,
    ash::khr::xcb_surface::NAME,
    ash::khr::win32_surface::NAME,
    ash::khr::android_surface::NAME,
    OHOS_SURFACE,
    ash::ext::headless_surface::NAME,
];

/// The system's Vulkan library. `OpenHarmony` names its loader `libvulkan.so`,
/// which ash looks for only on Android.
fn load_entry() -> Result<ash::Entry, Error> {
    // SAFETY: the Vulkan library's initializers have no preconditions.
    let entry =
        unsafe { if cfg!(target_env = "ohos") { ash::Entry::load_from("libvulkan.so") } else { ash::Entry::load() } };
    entry.map_err(|e| Error::unsupported(format!("No Vulkan library: {e}")))
}

/// A host's lock on a queue that it submits to too. The device holds it
/// around each submission and present to the queue, never across a wait.
pub trait QueueLock: Send + Sync {
    /// Takes the lock, and returns whether it did. A lock that refuses
    /// leaves the queue alone: the call that wanted it fails.
    fn lock(&self) -> bool;
    /// Gives back the lock that [`QueueLock::lock`] took.
    fn unlock(&self);
}

/// A device's hold on the host's queue lock: each [`Device::lease`] makes
/// one, which its clones share, and [`QueueLease::revoke`] ends.
struct LockHold(Mutex<Option<Arc<dyn QueueLock>>>);

impl LockHold {
    fn new(lock: Arc<dyn QueueLock>) -> Arc<LockHold> {
        Arc::new(LockHold(Mutex::new(Some(lock))))
    }
}

/// The end of a [`Device::lease`]'s hold on the host's queue lock.
pub struct QueueLease(Option<Arc<LockHold>>);

impl QueueLease {
    /// Ends the lease's hold on the host's queue lock: once this returns,
    /// the leased device and its clones call the lock no more, and their
    /// submissions and presents fail with `VK_ERROR_DEVICE_LOST`. It waits
    /// for one in progress, and lets go of the lock, which the last holder
    /// to let go drops.
    pub fn revoke(&self) {
        if let Some(hold) = &self.0 {
            // Dropped after the mutex, so a release that the drop runs
            // doesn't run inside it.
            let lock = hold.0.lock().take();
            drop(lock);
        }
    }
}

impl std::fmt::Debug for QueueLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueLease").field("locked", &self.0.is_some()).finish()
    }
}

/// What `MLN_VULKAN_VALIDATION` asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Validation {
    Off,
    Log,
    Abort,
}

impl Validation {
    /// The requested mode, and whether a missing layer is an error.
    fn requested() -> (Validation, bool) {
        match std::env::var("MLN_VULKAN_VALIDATION").as_deref() {
            Ok("0" | "off") => (Validation::Off, false),
            Ok("abort") => (Validation::Abort, true),
            Ok(_) => (Validation::Log, true),
            Err(_) if cfg!(debug_assertions) => (Validation::Log, false),
            Err(_) => (Validation::Off, false),
        }
    }
}

/// The debug messenger's callback: logs a validation message. `user_data` is
/// not null when an error aborts the process.
unsafe extern "system" fn debug_utils_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    // SAFETY: the layer passes a valid callback data struct, whose message
    // is a NUL-terminated string or null.
    let message = unsafe {
        callback_data
            .as_ref()
            .and_then(|data| data.message_as_c_str())
            .map(|message| message.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let is_error = severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR);
    if is_error && !user_data.is_null() {
        // Written to the stream itself: the test harness captures what
        // `eprintln!` prints, and drops it when the process aborts.
        let backtrace = std::backtrace::Backtrace::force_capture();
        let _ = writeln!(std::io::stderr(), "Vulkan validation error: {message}\n{backtrace}");
        std::process::abort();
    }
    let severity = if is_error {
        EventSeverity::Error
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        EventSeverity::Warning
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::INFO) {
        EventSeverity::Info
    } else {
        EventSeverity::Debug
    };
    Log::record(severity, Event::Render, -1, &message);
    vk::FALSE
}

fn extension_names(properties: &[vk::ExtensionProperties]) -> Vec<&CStr> {
    properties.iter().filter_map(|p| p.extension_name_as_c_str().ok()).collect()
}

fn pointers(names: &[&CStr]) -> Vec<*const c_char> {
    names.iter().map(|name| name.as_ptr()).collect()
}

/// A GPU's ID, as `VkPhysicalDeviceIDProperties` reports it: its
/// `deviceUUID`, or its `deviceLUID`, which on Windows is its DXGI adapter's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdapterId {
    Uuid([u8; vk::UUID_SIZE]),
    Luid([u8; vk::LUID_SIZE]),
}

/// Which GPU the library's device renders on.
#[derive(Clone, Copy, Debug)]
enum Selector {
    /// The GPU at an index of `vkEnumeratePhysicalDevices`, or the first
    /// that can render, discrete GPUs first.
    Index(Option<usize>),
    Id(AdapterId),
}

/// The raw handles of a device, for a host to make objects on it with.
#[derive(Clone, Copy, Debug)]
pub struct RawDevice {
    pub get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pub get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
    pub instance: vk::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: vk::Device,
    pub queue_family_index: u32,
    pub queue: vk::Queue,
}

/// A host's Vulkan objects, for [`Device::from_raw`].
pub struct HostDevice {
    /// The host's `vkGetInstanceProcAddr`, or `None` for the Vulkan
    /// loader's.
    pub get_instance_proc_addr: Option<vk::PFN_vkGetInstanceProcAddr>,
    /// The host's `vkGetDeviceProcAddr`, or `None` to look it up from the
    /// instance. Renderers on the device call the device's functions through
    /// it.
    pub get_device_proc_addr: Option<vk::PFN_vkGetDeviceProcAddr>,
    pub instance: vk::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: vk::Device,
    pub queue_family_index: u32,
    pub queue: vk::Queue,
    /// The host's lock on `queue`, when the host submits to it too.
    pub queue_lock: Option<Arc<dyn QueueLock>>,
    /// The device extensions that the host enabled on `device`. Vulkan has
    /// no way to ask a device for them, so renderers export memory and use
    /// sync files only through the ones listed here.
    pub enabled_extensions: Vec<CString>,
}

/// The library's instance, with the extensions its devices use and, when the
/// validation layer is on, its messenger.
//
// Deviation: C++'s renderer backend makes an instance of its own
// (`RendererBackend::initInstance`). The library's devices share one, made
// with the first of them and kept for the life of the process: the Vulkan
// loader crashes when one thread names an object (`vkSetDebugUtilsObjectNameEXT`,
// which renderers call with the validation layer on) while another destroys
// an instance, which renderers on many threads did whenever a device's last
// renderer let go.
struct LibraryInstance {
    entry: ash::Entry,
    instance: ash::Instance,
    extensions: Vec<&'static CStr>,
    api_version: u32,
    /// Never destroyed, like the instance it reports for.
    messenger: Option<vk::DebugUtilsMessengerEXT>,
}

// SAFETY: Vulkan handles and the loader's function tables may be used from
// any thread, and nothing here changes once made.
unsafe impl Send for LibraryInstance {}
// SAFETY: as above.
unsafe impl Sync for LibraryInstance {}

impl LibraryInstance {
    /// The instance, with the platforms' surface extensions that the loader
    /// has, and the validation layer and its messenger as
    /// `MLN_VULKAN_VALIDATION` asks.
    fn create() -> Result<LibraryInstance, Error> {
        let entry = load_entry()?;

        // SAFETY: plain queries of the loader.
        let (available_layers, available_extensions, loader_version) = unsafe {
            (
                entry
                    .enumerate_instance_layer_properties()
                    .map_err(|e| Error::native(format!("Vulkan layers: {e}")))?,
                entry
                    .enumerate_instance_extension_properties(None)
                    .map_err(|e| Error::native(format!("Vulkan instance extensions: {e}")))?,
                entry.try_enumerate_instance_version().ok().flatten().unwrap_or(vk::API_VERSION_1_0),
            )
        };
        // The IDs and the external semaphore queries need Vulkan 1.1.
        if loader_version < vk::API_VERSION_1_1 {
            return Err(Error::unsupported("The Vulkan loader is older than 1.1"));
        }
        let available_extensions = extension_names(&available_extensions);

        let (mut validation, validation_required) = Validation::requested();
        if validation != Validation::Off {
            let has_layer = available_layers.iter().any(|l| l.layer_name_as_c_str() == Ok(VALIDATION_LAYER));
            let has_debug_utils = available_extensions.contains(&ash::ext::debug_utils::NAME);
            if !has_layer || !has_debug_utils {
                if validation == Validation::Abort {
                    return Err(Error::unsupported("Vulkan layers not found"));
                }
                if validation_required {
                    Log::error(Event::Render, "Vulkan layers not found");
                }
                validation = Validation::Off;
            }
        }
        let layers: Vec<&CStr> = if validation == Validation::Off { Vec::new() } else { vec![VALIDATION_LAYER] };

        let mut extensions: Vec<&'static CStr> = Vec::new();
        if available_extensions.contains(&ash::khr::surface::NAME) {
            extensions.push(ash::khr::surface::NAME);
            extensions.extend(SURFACE_EXTENSIONS.iter().filter(|name| available_extensions.contains(name)));
        }
        if validation != Validation::Off {
            extensions.push(ash::ext::debug_utils::NAME);
        }

        // Vulkan 1.2 has timeline semaphores in its core; a GPU of an older
        // version renders without them.
        let api_version = loader_version.min(vk::API_VERSION_1_2);
        let app_info = vk::ApplicationInfo::default()
            .application_name(c"maplibre-native")
            .application_version(1)
            .engine_name(c"maplibre-native")
            .engine_version(1)
            .api_version(api_version);
        let layer_pointers = pointers(&layers);
        let extension_pointers = pointers(&extensions);
        let create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layer_pointers)
            .enabled_extension_names(&extension_pointers);
        // SAFETY: the create info points at live names, and the layers and
        // extensions are available.
        let instance = unsafe { entry.create_instance(&create_info, None) }.map_err(|e| {
            let message = format!("Failed to create a Vulkan instance: {e}");
            // A loader that finds no driver offers no Vulkan, as a missing
            // loader doesn't, so the next backend may take its place.
            if e == vk::Result::ERROR_INCOMPATIBLE_DRIVER {
                Error::unsupported(message)
            } else {
                Error::native(message)
            }
        })?;

        let mut messenger = None;
        if validation != Validation::Off {
            let loader = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let abort = usize::from(validation == Validation::Abort);
            let create_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::WARNING | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(debug_utils_callback))
                .user_data(abort as *mut c_void);
            // SAFETY: the callback is a function for the life of the
            // program, and reads its user data only as a flag.
            match unsafe { loader.create_debug_utils_messenger(&create_info, None) } {
                Ok(handle) => messenger = Some(handle),
                Err(_) => Log::error(Event::Render, "Failed to register Vulkan debug utils callback"),
            }
        }
        Ok(LibraryInstance { entry, instance, extensions, api_version, messenger })
    }
}

/// The library's instance, once a device has made it.
static LIBRARY_INSTANCE: Mutex<Option<Arc<LibraryInstance>>> = Mutex::new(None);

/// The library's instance: the one made before, or a new one. A failure
/// isn't kept, so the next device tries again.
fn library_instance() -> Result<Arc<LibraryInstance>, Error> {
    let mut slot = LIBRARY_INSTANCE.lock();
    if let Some(instance) = slot.as_ref() {
        return Ok(Arc::clone(instance));
    }
    let instance = Arc::new(LibraryInstance::create()?);
    *slot = Some(Arc::clone(&instance));
    Ok(instance)
}

/// The mutex of each queue that devices submit to, by its handle, which
/// every device on the queue shares.
static QUEUE_MUTEXES: LazyLock<Mutex<HashMap<u64, Weak<Mutex<()>>>>> = LazyLock::new(Default::default);

fn queue_mutex(queue: vk::Queue) -> Arc<Mutex<()>> {
    use ash::vk::Handle as _;
    let mut mutexes = QUEUE_MUTEXES.lock();
    mutexes.retain(|_, mutex| mutex.strong_count() > 0);
    let key = queue.as_raw();
    if let Some(mutex) = mutexes.get(&key).and_then(Weak::upgrade) {
        return mutex;
    }
    let mutex = Arc::new(Mutex::new(()));
    mutexes.insert(key, Arc::downgrade(&mutex));
    mutex
}

/// What the device's GPU and enabled extensions let renderers do.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Features {
    /// Timeline semaphores, of Vulkan 1.2 or `VK_KHR_timeline_semaphore`.
    pub(crate) timeline_semaphores: bool,
    /// Exporting a semaphore's payload as a sync file.
    pub(crate) export_sync_fd: bool,
    /// Importing a sync file into a semaphore.
    pub(crate) import_sync_fd: bool,
    /// Releasing images to `VK_QUEUE_FAMILY_FOREIGN_EXT`.
    pub(crate) foreign_queue_family: bool,
    /// Exporting memory as opaque file descriptors, of
    /// `VK_KHR_external_memory_fd`.
    pub(crate) external_memory_fd: bool,
    /// Exporting images as dmabufs with DRM format modifiers, of
    /// `VK_EXT_external_memory_dma_buf` and
    /// `VK_EXT_image_drm_format_modifier` with `VK_KHR_external_memory_fd`.
    pub(crate) dma_buf: bool,
    /// Importing Android hardware buffers into images, of
    /// `VK_ANDROID_external_memory_android_hardware_buffer` with
    /// `VK_EXT_queue_family_foreign`, which it requires.
    pub(crate) android_hardware_buffer: bool,
    /// Importing D3D11 textures through their NT handles, of
    /// `VK_KHR_external_memory_win32`.
    pub(crate) external_memory_win32: bool,
    /// Exporting a timeline semaphore as a D3D12 fence's NT handle, of
    /// `VK_KHR_external_semaphore_win32`.
    pub(crate) export_d3d12_fence: bool,
    /// Importing a D3D12 fence's NT handle into a timeline semaphore.
    pub(crate) import_d3d12_fence: bool,
}

pub(crate) struct Shared {
    entry: ash::Entry,
    instance: ash::Instance,
    /// `VK_EXT_debug_utils`, loaded when the validation layer is on.
    debug_utils: Option<ash::ext::debug_utils::Device>,
    surface: Option<ash::khr::surface::Instance>,
    swapchain: Option<ash::khr::swapchain::Device>,
    #[cfg_attr(not(unix), expect(dead_code, reason = "sync files are Unix's"))]
    semaphore_fd: Option<ash::khr::external_semaphore_fd::Device>,
    #[cfg_attr(not(unix), allow(dead_code))]
    memory_fd: Option<ash::khr::external_memory_fd::Device>,
    drm_format_modifier: Option<ash::ext::image_drm_format_modifier::Device>,
    /// Only Android has hardware buffers.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    android_hardware_buffer: Option<ash::android::external_memory_android_hardware_buffer::Device>,
    #[cfg_attr(not(windows), allow(dead_code))]
    memory_win32: Option<ash::khr::external_memory_win32::Device>,
    #[cfg_attr(not(windows), allow(dead_code))]
    semaphore_win32: Option<ash::khr::external_semaphore_win32::Device>,
    /// The D3D11 device on the GPU's adapter that makes the textures of
    /// exported rings, made once, when first asked.
    #[cfg(windows)]
    dxgi: OnceLock<Result<crate::win32::TextureDevice, Error>>,
    /// The kinds that the device has exported an image of, probed once.
    export_kinds: OnceLock<ExportKinds>,
    /// The kinds that the device has exported a depth image of, probed once.
    depth_export_kinds: OnceLock<ExportKinds>,
    physical_device: vk::PhysicalDevice,
    device: ash::Device,
    properties: vk::PhysicalDeviceProperties,
    /// The features renderers may use: those the library enabled, or those
    /// a host's GPU has of the ones the library would enable.
    enabled_features: vk::PhysicalDeviceFeatures,
    features: Features,
    queue_family_index: u32,
    queue: vk::Queue,
    queue_mutex: Arc<Mutex<()>>,
    get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
    /// The extensions of the library's instance. A host's instance has those
    /// whose functions it looks up.
    instance_extensions: Option<Vec<&'static CStr>>,
    /// The extensions enabled on the device: the library's, or those the
    /// host listed.
    device_extensions: Vec<CString>,
    /// Whether the library made the device, and destroys it. The library's
    /// instance outlives every device.
    owned: bool,
}

// SAFETY: Vulkan handles may be used from any thread. The queue, the only
// object here that needs external synchronization, is reached through
// `Device::with_queue`, which holds its mutex.
unsafe impl Send for Shared {}
// SAFETY: as above.
unsafe impl Sync for Shared {}

impl Drop for Shared {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        // SAFETY: every renderer held the device, so nothing uses it or its
        // objects any more, and each renderer drained its work from the
        // queue before it let go.
        unsafe { self.device.destroy_device(None) };
    }
}

/// A Vulkan GPU and queue that renderers on any thread share: the library's,
/// or the host's. Clones name the same one.
#[derive(Clone)]
pub struct Device {
    pub(crate) shared: Arc<Shared>,
    /// The hold on the host's lock on the queue, which clones share.
    host_lock: Option<Arc<LockHold>>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("device", &self.shared.device.handle())
            .field("owned", &self.shared.owned)
            .finish_non_exhaustive()
    }
}

/// The queue families of `candidate` that render, if it has the extensions
/// `required` names.
fn graphics_queue_family(instance: &ash::Instance, candidate: vk::PhysicalDevice, required: &[&CStr]) -> Option<u32> {
    // SAFETY: `candidate` is a physical device of `instance`.
    unsafe {
        let available = instance.enumerate_device_extension_properties(candidate).ok()?;
        let available = extension_names(&available);
        if !required.iter().all(|name| available.contains(name)) {
            return None;
        }
        instance
            .get_physical_device_queue_family_properties(candidate)
            .iter()
            .position(|queue| queue.queue_count > 0 && queue.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .map(|index| index as u32)
    }
}

/// Whether `physical_device` exports (`export`) or imports semaphores as
/// sync files.
fn sync_fd_support(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> (bool, bool) {
    let info =
        vk::PhysicalDeviceExternalSemaphoreInfo::default().handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let mut properties = vk::ExternalSemaphoreProperties::default();
    // SAFETY: a plain query of a physical device of the instance, which is
    // of Vulkan 1.1.
    unsafe { instance.get_physical_device_external_semaphore_properties(physical_device, &info, &mut properties) };
    let features = properties.external_semaphore_features;
    (
        features.contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE),
        features.contains(vk::ExternalSemaphoreFeatureFlags::IMPORTABLE),
    )
}

/// Whether `physical_device` exports (`export`) or imports timeline
/// semaphores as D3D12 fences.
fn d3d12_fence_support(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> (bool, bool) {
    let mut timeline = vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
    let info = vk::PhysicalDeviceExternalSemaphoreInfo::default()
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::D3D12_FENCE)
        .push_next(&mut timeline);
    let mut properties = vk::ExternalSemaphoreProperties::default();
    // SAFETY: a plain query of a physical device of the instance, which is
    // of Vulkan 1.1.
    unsafe { instance.get_physical_device_external_semaphore_properties(physical_device, &info, &mut properties) };
    let features = properties.external_semaphore_features;
    (
        features.contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE),
        features.contains(vk::ExternalSemaphoreFeatureFlags::IMPORTABLE),
    )
}

impl Device {
    /// The library's device: the GPU at `adapter_index` in
    /// `vkEnumeratePhysicalDevices` order, or the first that can render when
    /// `None`, discrete GPUs first.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) without a
    /// Vulkan library or a GPU that can render, or without what
    /// `MLN_VULKAN_VALIDATION=abort` needs of the instance, with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when there is no GPU at
    /// `adapter_index`, and with [`ErrorKind::Native`](mln_util::ErrorKind)
    /// when a Vulkan call fails.
    pub fn new(adapter_index: Option<usize>) -> Result<Device, Error> {
        Self::create(Selector::Index(adapter_index))
    }

    /// The library's device on the GPU that `id` names, by the IDs that
    /// `VkPhysicalDeviceIDProperties` reports.
    ///
    /// Fails as [`new`](Self::new) does, and with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when no GPU has `id`.
    pub fn with_adapter_id(id: AdapterId) -> Result<Device, Error> {
        Self::create(Selector::Id(id))
    }

    fn create(selector: Selector) -> Result<Device, Error> {
        let library = library_instance()?;
        let entry = library.entry.clone();
        let instance = library.instance.clone();
        let extensions = library.extensions.clone();
        let (physical_device, queue_family_index, device, enabled_features, features, device_extensions) =
            Self::create_device(&instance, selector, library.api_version)?;
        let debug_utils = library.messenger.is_some().then(|| ash::ext::debug_utils::Device::new(&instance, &device));
        let surface =
            extensions.contains(&ash::khr::surface::NAME).then(|| ash::khr::surface::Instance::new(&entry, &instance));
        let swapchain = device_extensions
            .contains(&ash::khr::swapchain::NAME)
            .then(|| ash::khr::swapchain::Device::new(&instance, &device));
        let semaphore_fd = device_extensions
            .contains(&ash::khr::external_semaphore_fd::NAME)
            .then(|| ash::khr::external_semaphore_fd::Device::new(&instance, &device));
        let memory_fd =
            features.external_memory_fd.then(|| ash::khr::external_memory_fd::Device::new(&instance, &device));
        let drm_format_modifier =
            features.dma_buf.then(|| ash::ext::image_drm_format_modifier::Device::new(&instance, &device));
        let android_hardware_buffer = features
            .android_hardware_buffer
            .then(|| ash::android::external_memory_android_hardware_buffer::Device::new(&instance, &device));
        let memory_win32 =
            features.external_memory_win32.then(|| ash::khr::external_memory_win32::Device::new(&instance, &device));
        let semaphore_win32 = (features.export_d3d12_fence || features.import_d3d12_fence)
            .then(|| ash::khr::external_semaphore_win32::Device::new(&instance, &device));
        // SAFETY: the device was made with a queue of the family; plain
        // queries of the physical device.
        let (queue, properties) = unsafe {
            (device.get_device_queue(queue_family_index, 0), instance.get_physical_device_properties(physical_device))
        };
        let get_instance_proc_addr = entry.static_fn().get_instance_proc_addr;
        let get_device_proc_addr = instance.fp_v1_0().get_device_proc_addr;
        Ok(Device {
            shared: Arc::new(Shared {
                entry,
                instance,
                debug_utils,
                surface,
                swapchain,
                semaphore_fd,
                memory_fd,
                drm_format_modifier,
                android_hardware_buffer,
                memory_win32,
                semaphore_win32,
                #[cfg(windows)]
                dxgi: OnceLock::new(),
                export_kinds: OnceLock::new(),
                depth_export_kinds: OnceLock::new(),
                physical_device,
                device,
                properties,
                enabled_features,
                features,
                queue_family_index,
                queue,
                queue_mutex: queue_mutex(queue),
                get_instance_proc_addr,
                get_device_proc_addr,
                instance_extensions: Some(extensions),
                device_extensions: device_extensions.iter().map(|name| (*name).to_owned()).collect(),
                owned: true,
            }),
            host_lock: None,
        })
    }

    /// The library's `VkDevice` on the GPU `selector` names, with the
    /// extensions and features renderers use where the GPU has them.
    #[allow(clippy::type_complexity)]
    fn create_device(
        instance: &ash::Instance,
        selector: Selector,
        api_version: u32,
    ) -> Result<(vk::PhysicalDevice, u32, ash::Device, vk::PhysicalDeviceFeatures, Features, Vec<&'static CStr>), Error>
    {
        // SAFETY: plain queries of the instance and its GPUs.
        let gpus = unsafe { instance.enumerate_physical_devices() }
            .map_err(|e| Error::native(format!("Vulkan physical devices: {e}")))?;
        if gpus.is_empty() {
            return Err(Error::unsupported("No Vulkan compatible GPU found"));
        }
        let required = [STORAGE_BUFFER_STORAGE_CLASS];
        let candidates: Vec<vk::PhysicalDevice> = match selector {
            Selector::Index(Some(index)) => {
                let count = gpus.len();
                vec![
                    *gpus
                        .get(index)
                        .ok_or_else(|| Error::not_found(format!("adapter_index {index} is past the {count} GPUs")))?,
                ]
            }
            Selector::Index(None) => {
                let mut ranked = gpus;
                // SAFETY: plain queries of the GPUs.
                ranked.sort_by_key(|&gpu| match unsafe { instance.get_physical_device_properties(gpu) }.device_type {
                    vk::PhysicalDeviceType::DISCRETE_GPU => 0,
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
                    _ => 2,
                });
                ranked
            }
            Selector::Id(id) => {
                let found = gpus.iter().copied().find(|&gpu| {
                    let mut ids = vk::PhysicalDeviceIDProperties::default();
                    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
                    // SAFETY: a plain query of the GPU, on a 1.1 instance.
                    unsafe { instance.get_physical_device_properties2(gpu, &mut properties) };
                    match id {
                        AdapterId::Uuid(uuid) => ids.device_uuid == uuid,
                        AdapterId::Luid(luid) => ids.device_luid_valid == vk::TRUE && ids.device_luid == luid,
                    }
                });
                vec![found.ok_or_else(|| Error::not_found(format!("no Vulkan GPU has the {id:?}")))?]
            }
        };
        let (physical_device, queue_family_index) = candidates
            .iter()
            .find_map(|&gpu| graphics_queue_family(instance, gpu, &required).map(|family| (gpu, family)))
            .ok_or_else(|| Error::unsupported("No suitable GPU found"))?;

        // SAFETY: plain queries of the physical device.
        let (available, gpu_version, supported) = unsafe {
            (
                instance
                    .enumerate_device_extension_properties(physical_device)
                    .map_err(|e| Error::native(format!("Vulkan device extensions: {e}")))?,
                instance.get_physical_device_properties(physical_device).api_version,
                instance.get_physical_device_features(physical_device),
            )
        };
        let available = extension_names(&available);
        // Windows' drivers have the win32 extensions: D3D11 textures for
        // exported rings, and D3D12 fences.
        let optional: &[&'static CStr] = &[
            ash::khr::swapchain::NAME,
            ash::khr::external_semaphore_fd::NAME,
            ash::ext::queue_family_foreign::NAME,
            ash::khr::external_memory_win32::NAME,
            ash::khr::external_semaphore_win32::NAME,
        ];
        let mut extensions: Vec<&'static CStr> = vec![STORAGE_BUFFER_STORAGE_CLASS];
        extensions.extend(optional.iter().filter(|name| available.contains(name)));
        // Exported rings: opaque file descriptors, and dmabufs, whose
        // modifiers need VK_KHR_image_format_list before Vulkan 1.2.
        let external_memory_fd = available.contains(&ash::khr::external_memory_fd::NAME);
        if external_memory_fd {
            extensions.push(ash::khr::external_memory_fd::NAME);
        }
        let core_format_list = api_version >= vk::API_VERSION_1_2 && gpu_version >= vk::API_VERSION_1_2;
        let dma_buf = external_memory_fd
            && available.contains(&ash::ext::external_memory_dma_buf::NAME)
            && available.contains(&ash::ext::image_drm_format_modifier::NAME)
            && (core_format_list || available.contains(&ash::khr::image_format_list::NAME));
        if dma_buf {
            extensions.push(ash::ext::external_memory_dma_buf::NAME);
            extensions.push(ash::ext::image_drm_format_modifier::NAME);
            if !core_format_list {
                extensions.push(ash::khr::image_format_list::NAME);
            }
        }

        // Android hardware buffers, whose other required extensions are in
        // Vulkan 1.1.
        let android_hardware_buffer = available.contains(&ash::android::external_memory_android_hardware_buffer::NAME)
            && extensions.contains(&ash::ext::queue_family_foreign::NAME)
            && api_version >= vk::API_VERSION_1_1
            && gpu_version >= vk::API_VERSION_1_1;
        if android_hardware_buffer {
            extensions.push(ash::android::external_memory_android_hardware_buffer::NAME);
        }

        let mut timeline_supported = vk::PhysicalDeviceTimelineSemaphoreFeatures::default();
        if api_version >= vk::API_VERSION_1_2 && gpu_version >= vk::API_VERSION_1_2 {
            let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut timeline_supported);
            // SAFETY: a plain query of the GPU, of Vulkan 1.2.
            unsafe { instance.get_physical_device_features2(physical_device, &mut features) };
        }
        let timeline = timeline_supported.timeline_semaphore == vk::TRUE;

        let mut enabled = vk::PhysicalDeviceFeatures::default();
        // TODO
        // - WideLines disabled on Android (20.77% device coverage
        //   https://vulkan.gpuinfo.org/listfeaturescore10.php)
        // - Rework this to a dynamic toggle based on
        //   MLN_TRIANGULATE_FILL_OUTLINES/MLN_ENABLE_POLYLINE_DRAWABLES
        if !cfg!(target_os = "android") {
            if supported.wide_lines == vk::TRUE {
                enabled.wide_lines = vk::TRUE;
            } else {
                Log::error(Event::Render, "Feature not available: wideLines");
            }
        }
        if supported.sampler_anisotropy == vk::TRUE {
            enabled.sampler_anisotropy = vk::TRUE;
        } else {
            Log::error(Event::Render, "Feature not available: samplerAnisotropy");
        }

        let queue_priority = [1.0f32];
        let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priority)];
        let extension_pointers = pointers(&extensions);
        let mut timeline_enabled = vk::PhysicalDeviceTimelineSemaphoreFeatures::default().timeline_semaphore(true);
        let mut create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_create_infos)
            .enabled_extension_names(&extension_pointers)
            .enabled_features(&enabled);
        if timeline {
            create_info = create_info.push_next(&mut timeline_enabled);
        }
        // SAFETY: the create info points at live data, the queue family is
        // the GPU's, and the extensions and features are supported.
        let device = unsafe { instance.create_device(physical_device, &create_info, None) }
            .map_err(|e| Error::native(format!("Failed to create a Vulkan device: {e}")))?;
        let (export_sync_fd, import_sync_fd) = if extensions.contains(&ash::khr::external_semaphore_fd::NAME) {
            sync_fd_support(instance, physical_device)
        } else {
            (false, false)
        };
        let (export_d3d12_fence, import_d3d12_fence) =
            if timeline && extensions.contains(&ash::khr::external_semaphore_win32::NAME) {
                d3d12_fence_support(instance, physical_device)
            } else {
                (false, false)
            };
        let features = Features {
            timeline_semaphores: timeline,
            export_sync_fd,
            import_sync_fd,
            foreign_queue_family: extensions.contains(&ash::ext::queue_family_foreign::NAME),
            external_memory_fd,
            dma_buf,
            android_hardware_buffer,
            external_memory_win32: extensions.contains(&ash::khr::external_memory_win32::NAME),
            export_d3d12_fence,
            import_d3d12_fence,
        };
        Ok((physical_device, queue_family_index, device, enabled, features, extensions))
    }

    /// The host's instance, device and queue, which renderers on the device
    /// render on and submit to, under the host's queue lock. A renderer uses
    /// the extensions the host enabled on the device: `VK_KHR_swapchain` to
    /// present, and timeline semaphores, and those of `enabled_extensions`
    /// that it lists: `VK_KHR_external_semaphore_fd` for sync files,
    /// `VK_KHR_external_memory_fd` for opaque file descriptors, that with
    /// `VK_EXT_external_memory_dma_buf` and `VK_EXT_image_drm_format_modifier`
    /// for dmabufs, `VK_KHR_external_memory_win32` for DXGI shared textures,
    /// `VK_KHR_external_semaphore_win32` for D3D12 fences, and
    /// `VK_ANDROID_external_memory_android_hardware_buffer` with
    /// `VK_EXT_queue_family_foreign` for Android hardware buffers.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) without a
    /// Vulkan library to look the functions up in, with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a queue family
    /// that the GPU lacks or that has no graphics queue, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when the device's core
    /// functions can't be looked up.
    ///
    /// # Safety
    ///
    /// The handles are live and the host's: `device` is a device of
    /// `physical_device` of `instance`, which `get_instance_proc_addr` loads
    /// the functions of, and `queue` is a queue of `device` in the family
    /// `queue_family_index` that supports graphics. The instance is of
    /// Vulkan 1.1 or later. They outlive the device and its renderers.
    pub unsafe fn from_raw(host: HostDevice) -> Result<Device, Error> {
        let HostDevice {
            get_instance_proc_addr,
            get_device_proc_addr,
            instance,
            physical_device,
            device,
            queue_family_index,
            queue,
            queue_lock,
            enabled_extensions,
        } = host;
        let enabled = |name: &CStr| enabled_extensions.iter().any(|enabled| enabled.as_c_str() == name);
        let entry = match get_instance_proc_addr {
            // SAFETY: the host's function looks up the functions of its
            // instance (the caller's contract).
            Some(get_instance_proc_addr) => unsafe {
                ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr })
            },
            None => load_entry()?,
        };
        let get_instance_proc_addr = entry.static_fn().get_instance_proc_addr;
        let lookup = |name: &CStr| -> *const c_void {
            // SAFETY: the instance is live, and `name` a function name.
            unsafe { std::mem::transmute(get_instance_proc_addr(instance, name.as_ptr())) }
        };
        let instance_get_device_proc_addr = lookup(c"vkGetDeviceProcAddr");
        let get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr = match get_device_proc_addr {
            Some(function) => function,
            None if !instance_get_device_proc_addr.is_null() => {
                // SAFETY: the instance's vkGetDeviceProcAddr has that type.
                unsafe {
                    std::mem::transmute::<*const c_void, vk::PFN_vkGetDeviceProcAddr>(instance_get_device_proc_addr)
                }
            }
            None => return Err(Error::native("The Vulkan instance has no vkGetDeviceProcAddr")),
        };
        // The instance's table looks device functions up through the host's
        // vkGetDeviceProcAddr, so that every device function a renderer
        // calls comes from it.
        // SAFETY: the instance is live.
        let instance = unsafe {
            ash::Instance::load_with(
                |name| {
                    if name == c"vkGetDeviceProcAddr" { get_device_proc_addr as *const c_void } else { lookup(name) }
                },
                instance,
            )
        };
        let device_lookup = |name: &CStr| -> bool {
            // SAFETY: the device is live, and `name` a function name.
            unsafe { get_device_proc_addr(device, name.as_ptr()) }.is_some()
        };
        if !device_lookup(c"vkQueueSubmit") || !device_lookup(c"vkCreateImageView") {
            return Err(Error::native("The Vulkan device's functions can't be looked up"));
        }
        // SAFETY: a plain query of the host's GPU.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
        let graphics = families
            .get(queue_family_index as usize)
            .is_some_and(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS));
        if !graphics {
            return Err(Error::invalid_argument(format!(
                "queue family {queue_family_index} of the Vulkan device has no graphics queue"
            )));
        }
        // SAFETY: the device is the instance's.
        let ash_device = unsafe { ash::Device::load(instance.fp_v1_0(), device) };
        let instance_has = |name: &CStr| !lookup(name).is_null();
        let surface = instance_has(c"vkGetPhysicalDeviceSurfaceSupportKHR")
            .then(|| ash::khr::surface::Instance::new(&entry, &instance));
        let swapchain =
            device_lookup(c"vkCreateSwapchainKHR").then(|| ash::khr::swapchain::Device::new(&instance, &ash_device));
        // A device's functions may look up whether or not the host enabled
        // their extensions, so its list says which ones a renderer uses.
        let sync_files = enabled(ash::khr::external_semaphore_fd::NAME);
        let semaphore_fd = (sync_files
            && (device_lookup(c"vkGetSemaphoreFdKHR") || device_lookup(c"vkImportSemaphoreFdKHR")))
        .then(|| ash::khr::external_semaphore_fd::Device::new(&instance, &ash_device));
        let external_memory_fd = enabled(ash::khr::external_memory_fd::NAME) && device_lookup(c"vkGetMemoryFdKHR");
        // SAFETY: plain queries of the host's GPU.
        let (properties, supported) = unsafe {
            (
                instance.get_physical_device_properties(physical_device),
                instance.get_physical_device_features(physical_device),
            )
        };
        let mut timeline_supported = vk::PhysicalDeviceTimelineSemaphoreFeatures::default();
        if instance_has(c"vkGetPhysicalDeviceFeatures2") {
            let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut timeline_supported);
            // SAFETY: a plain query of the GPU.
            unsafe { instance.get_physical_device_features2(physical_device, &mut features) };
        }
        // DRM format modifiers need VK_KHR_image_format_list before Vulkan 1.2.
        let dma_buf = external_memory_fd
            && enabled(ash::ext::external_memory_dma_buf::NAME)
            && enabled(ash::ext::image_drm_format_modifier::NAME)
            && (properties.api_version >= vk::API_VERSION_1_2 || enabled(ash::khr::image_format_list::NAME))
            && device_lookup(c"vkGetImageDrmFormatModifierPropertiesEXT");
        let android_hardware_buffer = enabled(ash::android::external_memory_android_hardware_buffer::NAME)
            && enabled(ash::ext::queue_family_foreign::NAME)
            && device_lookup(c"vkGetAndroidHardwareBufferPropertiesANDROID");
        let hardware_buffers = android_hardware_buffer
            .then(|| ash::android::external_memory_android_hardware_buffer::Device::new(&instance, &ash_device));
        let memory_fd = external_memory_fd.then(|| ash::khr::external_memory_fd::Device::new(&instance, &ash_device));
        let drm_format_modifier =
            dma_buf.then(|| ash::ext::image_drm_format_modifier::Device::new(&instance, &ash_device));
        let (export_sync_fd, import_sync_fd) =
            if sync_files && instance_has(c"vkGetPhysicalDeviceExternalSemaphoreProperties") {
                sync_fd_support(&instance, physical_device)
            } else {
                (false, false)
            };
        // Vulkan 1.2's, or VK_KHR_timeline_semaphore's: renderers only create
        // timelines and submit with them, which both share.
        let timeline_semaphores = timeline_supported.timeline_semaphore == vk::TRUE
            && (device_lookup(c"vkGetSemaphoreCounterValue") || device_lookup(c"vkGetSemaphoreCounterValueKHR"));
        let external_memory_win32 = enabled(ash::khr::external_memory_win32::NAME)
            && device_lookup(c"vkGetMemoryWin32HandlePropertiesKHR")
            && instance_has(c"vkGetPhysicalDeviceImageFormatProperties2");
        // A D3D12 fence is a timeline.
        let (export_d3d12_fence, import_d3d12_fence) = if timeline_semaphores
            && enabled(ash::khr::external_semaphore_win32::NAME)
            && instance_has(c"vkGetPhysicalDeviceExternalSemaphoreProperties")
        {
            let (export, import) = d3d12_fence_support(&instance, physical_device);
            (
                export && device_lookup(c"vkGetSemaphoreWin32HandleKHR"),
                import && device_lookup(c"vkImportSemaphoreWin32HandleKHR"),
            )
        } else {
            (false, false)
        };
        let features = Features {
            timeline_semaphores,
            export_sync_fd: export_sync_fd && device_lookup(c"vkGetSemaphoreFdKHR"),
            import_sync_fd: import_sync_fd && device_lookup(c"vkImportSemaphoreFdKHR"),
            // The host may not have enabled VK_EXT_queue_family_foreign, and
            // Vulkan offers no way to ask.
            foreign_queue_family: false,
            external_memory_fd,
            dma_buf,
            android_hardware_buffer,
            external_memory_win32,
            export_d3d12_fence,
            import_d3d12_fence,
        };
        let memory_win32 =
            external_memory_win32.then(|| ash::khr::external_memory_win32::Device::new(&instance, &ash_device));
        let semaphore_win32 = (export_d3d12_fence || import_d3d12_fence)
            .then(|| ash::khr::external_semaphore_win32::Device::new(&instance, &ash_device));
        // The features the library would enable, where the GPU has them. A
        // host that didn't enable one gets the validation layer's warning.
        let enabled_features = vk::PhysicalDeviceFeatures {
            wide_lines: if cfg!(target_os = "android") { vk::FALSE } else { supported.wide_lines },
            sampler_anisotropy: supported.sampler_anisotropy,
            ..Default::default()
        };
        Ok(Device {
            shared: Arc::new(Shared {
                entry,
                instance,
                debug_utils: None,
                surface,
                swapchain,
                semaphore_fd,
                memory_fd,
                drm_format_modifier,
                android_hardware_buffer: hardware_buffers,
                memory_win32,
                semaphore_win32,
                #[cfg(windows)]
                dxgi: OnceLock::new(),
                export_kinds: OnceLock::new(),
                depth_export_kinds: OnceLock::new(),
                physical_device,
                device: ash_device,
                properties,
                enabled_features,
                features,
                queue_family_index,
                queue,
                queue_mutex: queue_mutex(queue),
                get_instance_proc_addr,
                get_device_proc_addr,
                instance_extensions: None,
                device_extensions: enabled_extensions,
                owned: false,
            }),
            host_lock: queue_lock.map(LockHold::new),
        })
    }

    /// The library's device `self`, which nothing else holds yet, with the
    /// host's lock on its queue, for a host that submits to the queue too.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) for a
    /// device that renderers or clones hold already.
    pub fn with_queue_lock(mut self, lock: Arc<dyn QueueLock>) -> Result<Device, Error> {
        if Arc::get_mut(&mut self.shared).is_none() {
            return Err(Error::invalid_state("the Vulkan device is shared already"));
        }
        self.host_lock = Some(LockHold::new(lock));
        Ok(self)
    }

    /// A clone of the device for one user, such as a render session, with
    /// a hold of its own on the host's queue lock, which the returned lease
    /// revokes. The device's other clones keep theirs.
    #[must_use]
    pub fn lease(&self) -> (Device, QueueLease) {
        // A revoked hold leases revoked ones, never ones without a lock.
        let host_lock = self.host_lock.as_ref().map(|hold| Arc::new(LockHold(Mutex::new(hold.0.lock().clone()))));
        let lease = QueueLease(host_lock.clone());
        (Device { shared: self.shared.clone(), host_lock }, lease)
    }

    /// Runs `f` with the queue, holding the queue's mutex and the host's
    /// lock. Fails with `VK_ERROR_DEVICE_LOST` when the host's lock refuses,
    /// or the device's hold on it was revoked.
    pub(crate) fn with_queue<T>(
        &self,
        f: impl FnOnce(vk::Queue) -> ash::prelude::VkResult<T>,
    ) -> ash::prelude::VkResult<T> {
        let _guard = self.shared.queue_mutex.lock();
        let Some(hold) = &self.host_lock else { return f(self.shared.queue) };
        // Held through the call, so a revoke waits for it.
        let lock = hold.0.lock();
        let Some(lock) = &*lock else { return Err(vk::Result::ERROR_DEVICE_LOST) };
        if !lock.lock() {
            return Err(vk::Result::ERROR_DEVICE_LOST);
        }
        let result = f(self.shared.queue);
        lock.unlock();
        result
    }

    /// The device's raw handles.
    #[must_use]
    pub fn raw(&self) -> RawDevice {
        let shared = &*self.shared;
        RawDevice {
            get_instance_proc_addr: shared.get_instance_proc_addr,
            get_device_proc_addr: shared.get_device_proc_addr,
            instance: shared.instance.handle(),
            physical_device: shared.physical_device,
            device: shared.device.handle(),
            queue_family_index: shared.queue_family_index,
            queue: shared.queue,
        }
    }

    /// The device's handle and functions, as ash loads them. The device lives
    /// as long as a clone of `self` does.
    #[must_use]
    pub fn ash_device(&self) -> &ash::Device {
        self.shared.device()
    }

    /// Whether renderers on the device can present: its instance has
    /// `VK_KHR_surface` and its device `VK_KHR_swapchain`.
    #[must_use]
    pub fn presents(&self) -> bool {
        self.shared.surface.is_some() && self.shared.swapchain.is_some()
    }

    /// Whether renderers on the device present to the windows whose surfaces
    /// the instance extension `name` makes, such as
    /// `VK_KHR_wayland_surface` or [`OHOS_SURFACE`]: the device presents,
    /// and its instance has the extension.
    #[must_use]
    pub fn presents_through(&self, name: &CStr) -> bool {
        self.presents() && self.shared.has_instance_extension(name)
    }

    /// What the device lets renderers do.
    #[must_use]
    pub(crate) fn features(&self) -> Features {
        self.shared.features
    }

    /// The kinds of sync with which a host can wait for a borrowed image's
    /// frame (the producer's), and with which a renderer can wait for the
    /// host's reads (the consumer's). Both sides share the queue, so queue
    /// order needs no sync object: the host orders its reads after a frame
    /// with a barrier, and the next frame begins with one after the host's
    /// reads.
    #[must_use]
    pub fn sync_kinds(&self) -> (SyncKinds, SyncKinds) {
        let features = self.shared.features;
        let mut producer = SyncKinds::of(&[SyncKind::CpuComplete, SyncKind::QueueOrder]);
        let mut consumer = producer;
        if features.timeline_semaphores {
            producer = producer.with(SyncKind::VulkanTimelineSemaphore);
            consumer = consumer.with(SyncKind::VulkanTimelineSemaphore);
        }
        if features.export_sync_fd && cfg!(unix) {
            producer = producer.with(SyncKind::SyncFd);
        }
        if features.import_sync_fd && cfg!(unix) {
            consumer = consumer.with(SyncKind::SyncFd);
        }
        if features.export_d3d12_fence && cfg!(windows) {
            producer = producer.with(SyncKind::D3d12Fence);
        }
        if features.import_d3d12_fence && cfg!(windows) {
            consumer = consumer.with(SyncKind::D3d12Fence);
        }
        (producer, consumer)
    }

    /// The device extensions enabled on the device: the library's, or the
    /// ones the host listed.
    #[must_use]
    pub fn enabled_extensions(&self) -> &[CString] {
        &self.shared.device_extensions
    }

    /// The kinds of exported ring that the device shares its images in: those
    /// it exported an image of, once, when first asked.
    #[must_use]
    pub fn export_kinds(&self) -> ExportKinds {
        *self.shared.export_kinds.get_or_init(|| crate::exported_target::probe(self))
    }

    /// The kinds of [`export_kinds`](Self::export_kinds) whose rings share
    /// each frame's depth image too: those it exported a depth image of,
    /// once, when first asked.
    #[must_use]
    pub fn depth_export_kinds(&self) -> ExportKinds {
        *self.shared.depth_export_kinds.get_or_init(|| crate::exported_target::probe_depth(self))
    }

    /// The formats of the depth and stencil images that frames draw into.
    #[must_use]
    pub fn depth_stencil_formats(&self) -> DepthStencilFormats {
        DepthStencilFormat::BY_PREFERENCE.into_iter().fold(DepthStencilFormats::NONE, |formats, format| {
            // SAFETY: a plain query of the GPU.
            let properties = unsafe {
                self.shared
                    .instance()
                    .get_physical_device_format_properties(self.shared.physical_device(), depth_vk_format(format))
            };
            if properties.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT) {
                formats.with(format)
            } else {
                formats
            }
        })
    }
}

impl Shared {
    pub(crate) fn entry(&self) -> &ash::Entry {
        &self.entry
    }

    pub(crate) fn instance(&self) -> &ash::Instance {
        &self.instance
    }

    pub(crate) fn device(&self) -> &ash::Device {
        &self.device
    }

    pub(crate) fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    pub(crate) fn properties(&self) -> &vk::PhysicalDeviceProperties {
        &self.properties
    }

    pub(crate) fn enabled_features(&self) -> &vk::PhysicalDeviceFeatures {
        &self.enabled_features
    }

    pub(crate) fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    pub(crate) fn surface(&self) -> Option<&ash::khr::surface::Instance> {
        self.surface.as_ref()
    }

    pub(crate) fn swapchain(&self) -> Option<&ash::khr::swapchain::Device> {
        self.swapchain.as_ref()
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn memory_fd(&self) -> Option<&ash::khr::external_memory_fd::Device> {
        self.memory_fd.as_ref()
    }

    pub(crate) fn drm_format_modifier(&self) -> Option<&ash::ext::image_drm_format_modifier::Device> {
        self.drm_format_modifier.as_ref()
    }

    #[cfg(target_os = "android")]
    pub(crate) fn android_hardware_buffer(
        &self,
    ) -> Option<&ash::android::external_memory_android_hardware_buffer::Device> {
        self.android_hardware_buffer.as_ref()
    }

    #[cfg_attr(not(unix), expect(dead_code, reason = "sync files are Unix's"))]
    pub(crate) fn semaphore_fd(&self) -> Option<&ash::khr::external_semaphore_fd::Device> {
        self.semaphore_fd.as_ref()
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn memory_win32(&self) -> Option<&ash::khr::external_memory_win32::Device> {
        self.memory_win32.as_ref()
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn semaphore_win32(&self) -> Option<&ash::khr::external_semaphore_win32::Device> {
        self.semaphore_win32.as_ref()
    }

    /// The D3D11 device on the DXGI adapter of the GPU, made when first
    /// asked.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a GPU
    /// without a LUID, and as [`TextureDevice::new`](crate::win32::TextureDevice)
    /// does.
    #[cfg(windows)]
    pub(crate) fn dxgi(&self) -> Result<&crate::win32::TextureDevice, Error> {
        self.dxgi
            .get_or_init(|| {
                let mut ids = vk::PhysicalDeviceIDProperties::default();
                let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
                // SAFETY: a plain query of the GPU, of Vulkan 1.1.
                unsafe { self.instance.get_physical_device_properties2(self.physical_device, &mut properties) };
                if ids.device_luid_valid != vk::TRUE {
                    return Err(Error::unsupported("The Vulkan GPU has no LUID to find its DXGI adapter by"));
                }
                crate::win32::TextureDevice::new(ids.device_luid)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Whether the instance has the surface extension `name`. A host's
    /// instance has it when it looks the extension's function up.
    pub(crate) fn has_instance_extension(&self, name: &CStr) -> bool {
        if let Some(extensions) = &self.instance_extensions {
            return extensions.contains(&name);
        }
        let function = [
            (ash::khr::surface::NAME, c"vkGetPhysicalDeviceSurfaceSupportKHR"),
            (ash::khr::wayland_surface::NAME, c"vkCreateWaylandSurfaceKHR"),
            (ash::khr::xlib_surface::NAME, c"vkCreateXlibSurfaceKHR"),
            (ash::khr::xcb_surface::NAME, c"vkCreateXcbSurfaceKHR"),
            (ash::khr::win32_surface::NAME, c"vkCreateWin32SurfaceKHR"),
            (ash::khr::android_surface::NAME, c"vkCreateAndroidSurfaceKHR"),
            (OHOS_SURFACE, c"vkCreateSurfaceOHOS"),
            (ash::ext::headless_surface::NAME, c"vkCreateHeadlessSurfaceEXT"),
        ]
        .into_iter()
        .find_map(|(extension, function)| (extension == name).then_some(function));
        match function {
            // SAFETY: the instance is live, and `function` a function name.
            Some(function) => {
                unsafe { (self.get_instance_proc_addr)(self.instance.handle(), function.as_ptr()) }.is_some()
            }
            None => false,
        }
    }

    pub(crate) fn debug_utils(&self) -> Option<&ash::ext::debug_utils::Device> {
        self.debug_utils.as_ref()
    }
}

/// The `VkFormat` of `format`.
#[must_use]
pub(crate) fn depth_vk_format(format: DepthStencilFormat) -> vk::Format {
    match format {
        DepthStencilFormat::Depth24Stencil8 => vk::Format::D24_UNORM_S8_UINT,
        DepthStencilFormat::Depth32FloatStencil8 => vk::Format::D32_SFLOAT_S8_UINT,
    }
}

/// The depth and stencil format that `format` is, if it is one.
#[must_use]
pub fn depth_stencil_format(format: vk::Format) -> Option<DepthStencilFormat> {
    DepthStencilFormat::BY_PREFERENCE.into_iter().find(|candidate| depth_vk_format(*candidate) == format)
}
