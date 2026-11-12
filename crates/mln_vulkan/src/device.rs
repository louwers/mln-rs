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
use std::ffi::{CStr, c_char, c_void};
use std::io::Write as _;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use ash::vk;
use mln_gfx::{SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::logging::{Event, EventSeverity, Log};
use mln_util::sync::MutexExt;

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// The device extension for the storage class naga's SPIR-V declares its
/// storage buffers with, which Vulkan 1.0 doesn't have.
const STORAGE_BUFFER_STORAGE_CLASS: &CStr = c"VK_KHR_storage_buffer_storage_class";

/// The instance extensions of the platforms' surfaces, which the library's
/// instance enables where the loader has them.
const SURFACE_EXTENSIONS: &[&CStr] = &[
    ash::khr::wayland_surface::NAME,
    ash::khr::xlib_surface::NAME,
    ash::khr::xcb_surface::NAME,
    ash::khr::win32_surface::NAME,
    ash::khr::android_surface::NAME,
    ash::ext::headless_surface::NAME,
];

/// A host's lock on a queue that it submits to too. The device holds it
/// around each submission and present to the queue, never across a wait.
pub trait QueueLock: Send + Sync {
    fn lock(&self);
    fn unlock(&self);
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

/// `vkDebugUtilsCallback`: logs a validation message. `user_data` is not
/// null when an error aborts the process.
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
}

/// `VK_EXT_debug_utils`, loaded when the validation layer is on.
struct DebugUtils {
    instance: ash::ext::debug_utils::Instance,
    device: ash::ext::debug_utils::Device,
    /// The library's messenger, which goes with its instance.
    messenger: vk::DebugUtilsMessengerEXT,
}

/// The mutex of each queue that devices submit to, by its handle, which
/// every device on the queue shares.
static QUEUE_MUTEXES: LazyLock<Mutex<HashMap<u64, Weak<Mutex<()>>>>> = LazyLock::new(Default::default);

fn queue_mutex(queue: vk::Queue) -> Arc<Mutex<()>> {
    use ash::vk::Handle as _;
    let mut mutexes = QUEUE_MUTEXES.lock_unpoisoned();
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
pub struct Features {
    /// Timeline semaphores, of Vulkan 1.2 or `VK_KHR_timeline_semaphore`.
    pub timeline_semaphores: bool,
    /// Exporting a semaphore's payload as a sync file.
    pub export_sync_fd: bool,
    /// Importing a sync file into a semaphore.
    pub import_sync_fd: bool,
    /// Releasing images to `VK_QUEUE_FAMILY_FOREIGN_EXT`.
    pub foreign_queue_family: bool,
}

pub(crate) struct Shared {
    entry: ash::Entry,
    instance: ash::Instance,
    debug_utils: Option<DebugUtils>,
    surface: Option<ash::khr::surface::Instance>,
    swapchain: Option<ash::khr::swapchain::Device>,
    semaphore_fd: Option<ash::khr::external_semaphore_fd::Device>,
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
    host_lock: Option<Arc<dyn QueueLock>>,
    get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
    /// The extensions of the library's instance. A host's instance has those
    /// whose functions it looks up.
    instance_extensions: Option<Vec<&'static CStr>>,
    /// Whether the library made the instance and device, and destroys them.
    owned: bool,
}

// SAFETY: Vulkan handles may be used from any thread. The queue, the only
// object here that needs external synchronization, is reached through
// `with_queue`, which holds its mutex.
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
        unsafe {
            self.device.destroy_device(None);
            if let Some(debug_utils) = self.debug_utils.take() {
                debug_utils.instance.destroy_debug_utils_messenger(debug_utils.messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// A Vulkan GPU and queue that renderers on any thread share: the library's,
/// or the host's. Clones name the same one.
#[derive(Clone)]
pub struct Device {
    pub(crate) shared: Arc<Shared>,
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
        // SAFETY: the Vulkan library's initializers have no preconditions.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| Error::unsupported(format!("No Vulkan library: {e}")))?;

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
        let instance = unsafe { entry.create_instance(&create_info, None) }
            .map_err(|e| Error::native(format!("Failed to create a Vulkan instance: {e}")))?;

        // From here on, an error destroys what was made.
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
                Ok(handle) => messenger = Some((loader, handle)),
                Err(_) => Log::error(Event::Render, "Failed to register Vulkan debug utils callback"),
            }
        }
        let made = Self::create_device(&instance, selector, api_version);
        let (physical_device, queue_family_index, device, enabled_features, features, device_extensions) = match made {
            Ok(made) => made,
            Err(error) => {
                // SAFETY: nothing else refers to these objects, and they are
                // destroyed children first.
                unsafe {
                    if let Some((loader, messenger)) = messenger {
                        loader.destroy_debug_utils_messenger(messenger, None);
                    }
                    instance.destroy_instance(None);
                }
                return Err(error);
            }
        };
        let debug_utils = messenger.map(|(loader, messenger)| DebugUtils {
            instance: loader,
            device: ash::ext::debug_utils::Device::new(&instance, &device),
            messenger,
        });
        let surface =
            extensions.contains(&ash::khr::surface::NAME).then(|| ash::khr::surface::Instance::new(&entry, &instance));
        let swapchain = device_extensions
            .contains(&ash::khr::swapchain::NAME)
            .then(|| ash::khr::swapchain::Device::new(&instance, &device));
        let semaphore_fd = device_extensions
            .contains(&ash::khr::external_semaphore_fd::NAME)
            .then(|| ash::khr::external_semaphore_fd::Device::new(&instance, &device));
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
                physical_device,
                device,
                properties,
                enabled_features,
                features,
                queue_family_index,
                queue,
                queue_mutex: queue_mutex(queue),
                host_lock: None,
                get_instance_proc_addr,
                get_device_proc_addr,
                instance_extensions: Some(extensions),
                owned: true,
            }),
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
                let mut ranked = gpus.clone();
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
        let optional: &[&'static CStr] =
            &[ash::khr::swapchain::NAME, ash::khr::external_semaphore_fd::NAME, ash::ext::queue_family_foreign::NAME];
        let mut extensions: Vec<&'static CStr> = vec![STORAGE_BUFFER_STORAGE_CLASS];
        extensions.extend(optional.iter().filter(|name| available.contains(name)));

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
        let features = Features {
            timeline_semaphores: timeline,
            export_sync_fd,
            import_sync_fd,
            foreign_queue_family: extensions.contains(&ash::ext::queue_family_foreign::NAME),
        };
        Ok((physical_device, queue_family_index, device, enabled, features, extensions))
    }

    /// The host's instance, device and queue, which renderers on the device
    /// render on and submit to, under the host's queue lock. A renderer uses
    /// the extensions the host enabled on the device: `VK_KHR_swapchain` to
    /// present, timeline semaphores, `VK_KHR_external_semaphore_fd`
    /// for sync files.
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
        } = host;
        let entry = match get_instance_proc_addr {
            // SAFETY: the host's function looks up the functions of its
            // instance (the caller's contract).
            Some(get_instance_proc_addr) => unsafe {
                ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr })
            },
            // SAFETY: the Vulkan library's initializers have no
            // preconditions.
            None => unsafe { ash::Entry::load() }.map_err(|e| Error::unsupported(format!("No Vulkan library: {e}")))?,
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
        let semaphore_fd = (device_lookup(c"vkGetSemaphoreFdKHR") || device_lookup(c"vkImportSemaphoreFdKHR"))
            .then(|| ash::khr::external_semaphore_fd::Device::new(&instance, &ash_device));
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
        let (export_sync_fd, import_sync_fd) = if instance_has(c"vkGetPhysicalDeviceExternalSemaphoreProperties") {
            sync_fd_support(&instance, physical_device)
        } else {
            (false, false)
        };
        let features = Features {
            // Vulkan 1.2's, or VK_KHR_timeline_semaphore's: renderers only
            // create timelines and submit with them, which both share.
            timeline_semaphores: timeline_supported.timeline_semaphore == vk::TRUE
                && (device_lookup(c"vkGetSemaphoreCounterValue") || device_lookup(c"vkGetSemaphoreCounterValueKHR")),
            export_sync_fd: export_sync_fd && device_lookup(c"vkGetSemaphoreFdKHR"),
            import_sync_fd: import_sync_fd && device_lookup(c"vkImportSemaphoreFdKHR"),
            // The host may not have enabled VK_EXT_queue_family_foreign, and
            // Vulkan offers no way to ask.
            foreign_queue_family: false,
        };
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
                physical_device,
                device: ash_device,
                properties,
                enabled_features,
                features,
                queue_family_index,
                queue,
                queue_mutex: queue_mutex(queue),
                host_lock: queue_lock,
                get_instance_proc_addr,
                get_device_proc_addr,
                instance_extensions: None,
                owned: false,
            }),
        })
    }

    /// The library's device `self`, which nothing else holds yet, with the
    /// host's lock on its queue, for a host that submits to the queue too.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) for a
    /// device that renderers or clones hold already.
    pub fn with_queue_lock(mut self, lock: Arc<dyn QueueLock>) -> Result<Device, Error> {
        let shared = Arc::get_mut(&mut self.shared)
            .ok_or_else(|| Error::invalid_state("the Vulkan device is shared already"))?;
        shared.host_lock = Some(lock);
        Ok(self)
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

    /// Whether `self` and `other` are clones of one device.
    #[must_use]
    pub fn same_as(&self, other: &Device) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Whether renderers on the device can present: its instance has
    /// `VK_KHR_surface` and its device `VK_KHR_swapchain`.
    #[must_use]
    pub fn presents(&self) -> bool {
        self.shared.surface.is_some() && self.shared.swapchain.is_some()
    }

    /// What the device lets renderers do.
    #[must_use]
    pub fn features(&self) -> Features {
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
        (producer, consumer)
    }

    /// The name of the GPU.
    #[must_use]
    pub fn name(&self) -> String {
        self.shared
            .properties
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
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

    pub(crate) fn semaphore_fd(&self) -> Option<&ash::khr::external_semaphore_fd::Device> {
        self.semaphore_fd.as_ref()
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
        self.debug_utils.as_ref().map(|debug_utils| &debug_utils.device)
    }

    /// Runs `f` with the queue, holding the queue's mutex and the host's
    /// lock.
    pub(crate) fn with_queue<R>(&self, f: impl FnOnce(vk::Queue) -> R) -> R {
        let _guard = self.queue_mutex.lock_unpoisoned();
        if let Some(lock) = &self.host_lock {
            lock.lock();
        }
        let result = f(self.queue);
        if let Some(lock) = &self.host_lock {
            lock.unlock();
        }
        result
    }
}
