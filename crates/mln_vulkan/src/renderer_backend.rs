// Ports include/mln/vulkan/renderer_backend.hpp and src/mln/vulkan/renderer_backend.cpp
//
// The instance, the device and what hangs off them are shared
// (`Rc<BackendImpl>`) by the backend, its context and every resource: C++
// reaches them through `Context& -> getBackend()`, and destroys the device
// with the backend; here the device goes with its last user.
//
// What the backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`). Where C++ asks the default renderable's resource for the
// platform's surface and extensions through virtual calls, a window target
// passes a `SurfaceProvider`.
//
// Differences from C++:
// - C++ turns the validation layer on in debug builds and logs an error when
//   it is missing. The port turns it on in debug builds when it is there,
//   and the environment variable MLN_VULKAN_VALIDATION overrides that: `0`
//   for off, `1` for on, `abort` to also abort the process on a validation
//   error, after printing where it happened, and to make no backend without
//   the layer.
// - Only the validation layer's warnings and errors are logged.
// - `VK_EXT_debug_report`, the fallback for instances without
//   `VK_EXT_debug_utils`, the RenderDoc frame capture and the Windows
//   monitor layer are not ported.
// - macOS's portability enumeration is left out with the platform.
// - The device needs `VK_KHR_storage_buffer_storage_class`, for the SPIR-V
//   naga writes (`shaders::compiler`).

use std::any::Any;
use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_void};
use std::io::Write as _;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use ash::vk;
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use gpu_allocator::{AllocationSizes, AllocatorDebugSettings};
use mln_gfx::context::Context as GfxContext;
use mln_gfx::renderer_backend::{self, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_util::error::Error;
use mln_util::logging::{Event, EventSeverity, Log};

use crate::context::Context;
use crate::device::Device;
use crate::shaders::shader_group::ShaderGroup;
use crate::shaders::shader_source::shader_source;

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// The device extension for the storage class naga's SPIR-V declares its
/// storage buffers with, which Vulkan 1.0 doesn't have.
const STORAGE_BUFFER_STORAGE_CLASS: &CStr = c"VK_KHR_storage_buffer_storage_class";

/// The platform's part of a backend that renders to a window: what C++'s
/// `SurfaceResource::createPlatformSurface` and the platform's
/// `getInstanceExtensions` do.
pub trait SurfaceProvider {
    /// The instance extensions the surface needs.
    fn instance_extensions(&self) -> Vec<&'static CStr>;

    /// Creates the window's surface.
    fn create_surface(&self, entry: &ash::Entry, instance: &ash::Instance) -> Result<vk::SurfaceKHR, Error>;
}

/// What MLN_VULKAN_VALIDATION asks for.
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

/// `VK_EXT_debug_utils`, loaded when the validation layer is on.
struct DebugUtils {
    instance: ash::ext::debug_utils::Instance,
    device: ash::ext::debug_utils::Device,
    messenger: vk::DebugUtilsMessengerEXT,
}

/// The instance and device state the backend, its context and their
/// resources share.
pub struct BackendImpl {
    entry: ash::Entry,
    instance: ash::Instance,
    debug_utils: Option<DebugUtils>,
    surface_loader: ash::khr::surface::Instance,
    swapchain_loader: Option<ash::khr::swapchain::Device>,
    physical_device: vk::PhysicalDevice,
    device: ash::Device,
    device_properties: vk::PhysicalDeviceProperties,
    device_features: vk::PhysicalDeviceFeatures,
    graphics_queue_index: u32,
    present_queue_index: Option<u32>,
    graphics_queue: vk::Queue,
    present_queue: Option<vk::Queue>,
    command_pool: vk::CommandPool,
    max_frames: u32,
    /// Dropped with the backend, before the device.
    allocator: RefCell<ManuallyDrop<Allocator>>,
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

/// `checkAvailability`: whether every name in `required` is in `available`.
fn check_availability(available: &[&CStr], required: &[&CStr]) -> bool {
    required.iter().all(|required| available.contains(required))
}

fn extension_names(properties: &[vk::ExtensionProperties]) -> Vec<&CStr> {
    properties.iter().filter_map(|p| p.extension_name_as_c_str().ok()).collect()
}

fn pointers(names: &[&CStr]) -> Vec<*const c_char> {
    names.iter().map(|name| name.as_ptr()).collect()
}

/// The queue families `candidate` renders and presents with, if it has the
/// extensions and, with a surface, can present to it (`isPhysicalDeviceCompatible`).
fn compatible_queues(
    instance: &ash::Instance,
    surface_loader: &ash::khr::surface::Instance,
    candidate: vk::PhysicalDevice,
    extensions: &[&CStr],
    surface: Option<vk::SurfaceKHR>,
) -> Option<(u32, Option<u32>)> {
    // SAFETY: `candidate` is a physical device of `instance`, and `surface`
    // a surface of it.
    unsafe {
        let available = instance.enumerate_device_extension_properties(candidate).ok()?;
        if !check_availability(&extension_names(&available), extensions) {
            return None;
        }

        let mut graphics_queue_index = None;
        let mut present_queue_index = None;
        for (i, queue) in instance.get_physical_device_queue_family_properties(candidate).iter().enumerate() {
            let i = i as u32;
            if queue.queue_count == 0 {
                continue;
            }
            if queue.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                graphics_queue_index = Some(i);
            }
            if let Some(surface) = surface
                && surface_loader.get_physical_device_surface_support(candidate, i, surface).unwrap_or(false)
            {
                present_queue_index = Some(i);
            }
            if graphics_queue_index.is_some() && (surface.is_none() || present_queue_index.is_some()) {
                break;
            }
        }
        let graphics_queue_index = graphics_queue_index?;
        if let Some(surface) = surface {
            present_queue_index?;
            if surface_loader.get_physical_device_surface_formats(candidate, surface).ok()?.is_empty() {
                return None;
            }
            if surface_loader.get_physical_device_surface_present_modes(candidate, surface).ok()?.is_empty() {
                return None;
            }
        }
        Some((graphics_queue_index, present_queue_index))
    }
}

impl BackendImpl {
    /// `init()`: the instance, the debug messenger, the platform's surface,
    /// the device (on the GPU at `adapter_index`, or the first that can
    /// render), the allocator and the command pool. Returns the surface for
    /// the target to own.
    fn new(
        adapter_index: Option<usize>,
        provider: Option<&dyn SurfaceProvider>,
    ) -> Result<(BackendImpl, Option<vk::SurfaceKHR>), Error> {
        // SAFETY: the Vulkan library's initializers have no preconditions.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| Error::unsupported(format!("No Vulkan library: {e}")))?;

        // initInstance
        // SAFETY: plain queries of the loader.
        let (available_layers, available_extensions) = unsafe {
            (
                entry
                    .enumerate_instance_layer_properties()
                    .map_err(|e| Error::native(format!("Vulkan layers: {e}")))?,
                entry
                    .enumerate_instance_extension_properties(None)
                    .map_err(|e| Error::native(format!("Vulkan instance extensions: {e}")))?,
            )
        };
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

        let mut extensions = provider.map(SurfaceProvider::instance_extensions).unwrap_or_default();
        if !check_availability(&available_extensions, &extensions) {
            return Err(Error::unsupported("Vulkan extensions not found"));
        }
        if validation != Validation::Off {
            extensions.push(ash::ext::debug_utils::NAME);
        }

        // Vulkan 1.1 on Android is supported on 71% of devices (compared to
        // 1.3 with 6%) as of April 23 2024: https://vulkan.gpuinfo.org/
        let app_info = vk::ApplicationInfo::default()
            .application_name(c"maplibre-native")
            .application_version(1)
            .engine_name(c"maplibre-native")
            .engine_version(1)
            .api_version(vk::API_VERSION_1_0);
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
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
        let mut messenger = None;
        let mut surface = None;
        let result = (|| {
            // initDebug
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

            // initSurface
            if let Some(provider) = provider {
                surface = Some(provider.create_surface(&entry, &instance)?);
            }

            // initDevice
            let mut device_extensions: Vec<&CStr> = vec![STORAGE_BUFFER_STORAGE_CLASS];
            if surface.is_some() {
                device_extensions.push(ash::khr::swapchain::NAME);
            }
            // SAFETY: a plain query of the instance.
            let physical_devices = unsafe { instance.enumerate_physical_devices() }
                .map_err(|e| Error::native(format!("Vulkan physical devices: {e}")))?;
            if physical_devices.is_empty() {
                return Err(Error::unsupported("No Vulkan compatible GPU found"));
            }
            let candidates = match adapter_index {
                Some(index) => {
                    let count = physical_devices.len();
                    let candidate = *physical_devices
                        .get(index)
                        .ok_or_else(|| Error::not_found(format!("adapter_index {index} is past the {count} GPUs")))?;
                    vec![candidate]
                }
                None => physical_devices,
            };
            let (physical_device, graphics_queue_index, present_queue_index) = candidates
                .iter()
                .find_map(|&candidate| {
                    compatible_queues(&instance, &surface_loader, candidate, &device_extensions, surface)
                        .map(|(graphics, present)| (candidate, graphics, present))
                })
                .ok_or_else(|| Error::unsupported("No suitable GPU found"))?;

            let queue_priority = [1.0f32];
            let mut queue_create_infos = vec![
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(graphics_queue_index)
                    .queue_priorities(&queue_priority),
            ];
            if let Some(present_queue_index) = present_queue_index
                && present_queue_index != graphics_queue_index
            {
                queue_create_infos.push(
                    vk::DeviceQueueCreateInfo::default()
                        .queue_family_index(present_queue_index)
                        .queue_priorities(&queue_priority),
                );
            }

            // SAFETY: a plain query of the physical device.
            let supported_features = unsafe { instance.get_physical_device_features(physical_device) };
            let mut device_features = vk::PhysicalDeviceFeatures::default();
            // TODO
            // - WideLines disabled on Android (20.77% device coverage
            //   https://vulkan.gpuinfo.org/listfeaturescore10.php)
            // - Rework this to a dynamic toggle based on
            //   MLN_TRIANGULATE_FILL_OUTLINES/MLN_ENABLE_POLYLINE_DRAWABLES
            if !cfg!(target_os = "android") {
                if supported_features.wide_lines == vk::TRUE {
                    device_features.wide_lines = vk::TRUE;
                } else {
                    Log::error(Event::Render, "Feature not available: wideLines");
                }
            }
            if supported_features.sampler_anisotropy == vk::TRUE {
                device_features.sampler_anisotropy = vk::TRUE;
            } else {
                Log::error(Event::Render, "Feature not available: samplerAnisotropy");
            }

            let device_extension_pointers = pointers(&device_extensions);
            let create_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_create_infos)
                .enabled_extension_names(&device_extension_pointers)
                .enabled_features(&device_features);
            // SAFETY: the create info points at live data, the queue
            // families are the physical device's and the features are
            // supported.
            let device = unsafe { instance.create_device(physical_device, &create_info, None) }
                .map_err(|e| Error::native(format!("Failed to create a Vulkan device: {e}")))?;

            // SAFETY: the device was created with a queue of each family.
            let (device_properties, graphics_queue, present_queue) = unsafe {
                (
                    instance.get_physical_device_properties(physical_device),
                    device.get_device_queue(graphics_queue_index, 0),
                    present_queue_index.map(|index| device.get_device_queue(index, 0)),
                )
            };

            // initAllocator
            const MB: u64 = 1024 * 1024;
            let allocator = Allocator::new(&AllocatorCreateDesc {
                instance: instance.clone(),
                device: device.clone(),
                physical_device,
                debug_settings: AllocatorDebugSettings::default(),
                buffer_device_address: false,
                allocation_sizes: AllocationSizes::new(16 * MB, 8 * MB)
                    .with_max_device_memblock_size(256 * MB)
                    .with_max_host_memblock_size(64 * MB),
            });

            // initCommandPool
            let create_info = vk::CommandPoolCreateInfo::default()
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                .queue_family_index(graphics_queue_index);
            // SAFETY: the queue family is the device's.
            let command_pool = unsafe { device.create_command_pool(&create_info, None) };
            let (allocator, command_pool) = match (allocator, command_pool) {
                (Ok(allocator), Ok(command_pool)) => (allocator, command_pool),
                (allocator, command_pool) => {
                    let error = match (&allocator, &command_pool) {
                        (Err(e), _) => format!("Vulkan allocator init failed: {e}"),
                        (_, Err(e)) => format!("Failed to create a Vulkan command pool: {e}"),
                        _ => String::new(),
                    };
                    drop(allocator);
                    // SAFETY: nothing but the pool was made from the device.
                    unsafe {
                        if let Ok(command_pool) = command_pool {
                            device.destroy_command_pool(command_pool, None);
                        }
                        device.destroy_device(None);
                    }
                    return Err(Error::native(error));
                }
            };

            Ok((
                device,
                physical_device,
                device_properties,
                device_features,
                graphics_queue_index,
                present_queue_index,
                graphics_queue,
                present_queue,
                allocator,
                command_pool,
            ))
        })();

        let (
            device,
            physical_device,
            device_properties,
            device_features,
            graphics_queue_index,
            present_queue_index,
            graphics_queue,
            present_queue,
            allocator,
            command_pool,
        ) = match result {
            Ok(parts) => parts,
            Err(error) => {
                // SAFETY: nothing else refers to these objects, and they are
                // destroyed children first.
                unsafe {
                    if let Some(surface) = surface {
                        surface_loader.destroy_surface(surface, None);
                    }
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
        let swapchain_loader = surface.map(|_| ash::khr::swapchain::Device::new(&instance, &device));

        Ok((
            BackendImpl {
                entry,
                instance,
                debug_utils,
                surface_loader,
                swapchain_loader,
                physical_device,
                device,
                device_properties,
                device_features,
                graphics_queue_index,
                present_queue_index,
                graphics_queue,
                present_queue,
                command_pool,
                // Buffer resources if rendering to a surface; no buffering
                // when using headless.
                max_frames: if surface.is_some() { 2 } else { 1 },
                allocator: RefCell::new(ManuallyDrop::new(allocator)),
            },
            surface,
        ))
    }

    /// The loaded Vulkan library.
    pub fn entry(&self) -> &ash::Entry {
        &self.entry
    }

    pub fn get_instance(&self) -> &ash::Instance {
        &self.instance
    }

    pub fn get_physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    pub fn get_device(&self) -> &ash::Device {
        &self.device
    }

    pub fn get_surface_loader(&self) -> &ash::khr::surface::Instance {
        &self.surface_loader
    }

    /// `VK_KHR_swapchain`, when the backend renders to a surface.
    pub fn get_swapchain_loader(&self) -> Option<&ash::khr::swapchain::Device> {
        self.swapchain_loader.as_ref()
    }

    pub fn get_command_pool(&self) -> vk::CommandPool {
        self.command_pool
    }

    pub fn get_graphics_queue(&self) -> vk::Queue {
        self.graphics_queue
    }

    pub fn get_present_queue(&self) -> Option<vk::Queue> {
        self.present_queue
    }

    pub fn get_max_frames(&self) -> u32 {
        self.max_frames
    }

    /// Runs `f` with the allocator.
    pub fn with_allocator<R>(&self, f: impl FnOnce(&mut Allocator) -> R) -> R {
        f(&mut self.allocator.borrow_mut())
    }

    pub fn get_device_properties(&self) -> &vk::PhysicalDeviceProperties {
        &self.device_properties
    }

    /// The features the device was created with.
    pub fn get_device_features(&self) -> &vk::PhysicalDeviceFeatures {
        &self.device_features
    }

    pub fn get_graphics_queue_index(&self) -> u32 {
        self.graphics_queue_index
    }

    pub fn get_present_queue_index(&self) -> Option<u32> {
        self.present_queue_index
    }

    /// `setDebugName(object, name)`: names an object for the validation
    /// layer's messages and for debuggers.
    pub fn set_debug_name<T: vk::Handle>(&self, object: T, name: &str) {
        let Some(debug_utils) = &self.debug_utils else {
            return;
        };
        let Ok(name) = std::ffi::CString::new(name) else {
            return;
        };
        let info = vk::DebugUtilsObjectNameInfoEXT::default().object_handle(object).object_name(&name);
        // SAFETY: `object` is a live object of the device.
        let _ = unsafe { debug_utils.device.set_debug_utils_object_name(&info) };
    }

    pub fn begin_debug_label(&self, buffer: vk::CommandBuffer, name: &str, color: [f32; 4]) {
        let Some(debug_utils) = &self.debug_utils else {
            return;
        };
        let Ok(name) = std::ffi::CString::new(name) else {
            return;
        };
        let label = vk::DebugUtilsLabelEXT::default().label_name(&name).color(color);
        // SAFETY: `buffer` is a command buffer of the device, recording.
        unsafe { debug_utils.device.cmd_begin_debug_utils_label(buffer, &label) };
    }

    pub fn end_debug_label(&self, buffer: vk::CommandBuffer) {
        if let Some(debug_utils) = &self.debug_utils {
            // SAFETY: as in `begin_debug_label`, which opened the label.
            unsafe { debug_utils.device.cmd_end_debug_utils_label(buffer) };
        }
    }

    pub fn insert_debug_label(&self, buffer: vk::CommandBuffer, name: &str) {
        let Some(debug_utils) = &self.debug_utils else {
            return;
        };
        let Ok(name) = std::ffi::CString::new(name) else {
            return;
        };
        let label = vk::DebugUtilsLabelEXT::default().label_name(&name);
        // SAFETY: as in `begin_debug_label`.
        unsafe { debug_utils.device.cmd_insert_debug_utils_label(buffer, &label) };
    }

    /// Waits for the device to finish its work, logging `what` was going on
    /// if the device is lost.
    pub fn wait_idle(&self, what: &str) {
        // SAFETY: the device is live.
        if unsafe { self.device.device_wait_idle() }.is_err() {
            Log::error(Event::Render, format!("Vulkan device lost during {what}"));
        }
    }
}

impl Drop for BackendImpl {
    /// `destroyResources()`
    fn drop(&mut self) {
        self.wait_idle("backend shutdown");
        // SAFETY: every user of the device held this struct, so nothing
        // refers to these objects any more; children go first.
        unsafe {
            self.device.destroy_command_pool(self.command_pool, None);
            ManuallyDrop::drop(&mut self.allocator.borrow_mut());
            self.device.destroy_device(None);
            // Destroy this last so we have cleanup validation.
            if let Some(debug_utils) = self.debug_utils.take() {
                debug_utils.instance.destroy_debug_utils_messenger(debug_utils.messenger, None);
            }
            self.instance.destroy_instance(None);
        }
    }
}

/// `vulkan::RendererBackend`: a renderer's instance, device and context.
pub struct RendererBackend {
    base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
    /// The context, made with the backend so that failing to make it is the
    /// backend's error.
    context: Rc<Context>,
}

/// The shader types registered by `initShaders`, in the C++ order.
/// `init_shaders` skips the ones without a Vulkan source in the port
/// (`shader_source`): the location indicator and wide vector shaders.
const REGISTERED_SHADERS: &[BuiltIn] = &[
    BuiltIn::BackgroundShader,
    BuiltIn::BackgroundPatternShader,
    BuiltIn::SkyShader,
    BuiltIn::AtmosphereShader,
    BuiltIn::CircleShader,
    BuiltIn::ClippingMaskProgram,
    BuiltIn::CollisionBoxShader,
    BuiltIn::CollisionCircleShader,
    BuiltIn::ColorReliefShader,
    BuiltIn::DebugShader,
    BuiltIn::FillShader,
    BuiltIn::FillOutlineShader,
    BuiltIn::FillPatternShader,
    BuiltIn::FillOutlinePatternShader,
    BuiltIn::FillOutlineTriangulatedShader,
    BuiltIn::FillExtrusionShader,
    BuiltIn::FillExtrusionInstancedShader,
    BuiltIn::FillExtrusionPatternShader,
    BuiltIn::FillExtrusionPatternInstancedShader,
    BuiltIn::HeatmapShader,
    BuiltIn::HeatmapTextureShader,
    BuiltIn::HillshadeShader,
    BuiltIn::HillshadePrepareShader,
    BuiltIn::LineShader,
    BuiltIn::LineGradientShader,
    BuiltIn::LineSDFShader,
    BuiltIn::LineGradientSDFShader,
    BuiltIn::LinePatternShader,
    BuiltIn::LocationIndicatorShader,
    BuiltIn::LocationIndicatorTexturedShader,
    BuiltIn::RasterShader,
    BuiltIn::SymbolIconShader,
    BuiltIn::SymbolSDFShader,
    BuiltIn::SymbolTextAndIconShader,
    BuiltIn::WideVectorShader,
    BuiltIn::GlobeDepthShader,
];

impl RendererBackend {
    /// `RendererBackend()` and `init()`: a backend on `device` for offscreen
    /// targets.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) without a
    /// Vulkan library, a device that can render, or what
    /// MLN_VULKAN_VALIDATION=abort needs of the instance, and on the host's
    /// device, with [`ErrorKind::NotFound`](mln_util::ErrorKind) when the
    /// device's adapter index names no GPU, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan call fails.
    pub fn new(device: &Device) -> Result<Self, Error> {
        Self::with_surface(device, None).map(|(backend, _)| backend)
    }

    /// `RendererBackend()` and `init()`: a backend on `device` that also
    /// presents to the surface `provider` makes, and to other surfaces of
    /// the platform's windows. Returns the surface.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) without a
    /// Vulkan library, a device that can render (and present to the
    /// surface), or what the surface or MLN_VULKAN_VALIDATION=abort needs of
    /// the instance, and on the host's device, with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when the device's
    /// adapter index names no GPU, with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan call fails,
    /// and with `provider`'s error.
    pub(crate) fn with_surface(
        device: &Device,
        provider: Option<&dyn SurfaceProvider>,
    ) -> Result<(Self, Option<vk::SurfaceKHR>), Error> {
        let adapter_index = device.library_adapter_index()?;
        let (impl_, surface) = BackendImpl::new(adapter_index, provider)?;
        let impl_ = Rc::new(impl_);
        let context = match Context::new(impl_.clone()) {
            Ok(context) => context,
            Err(error) => {
                if let Some(surface) = surface {
                    // SAFETY: nothing was made on the surface.
                    unsafe { impl_.get_surface_loader().destroy_surface(surface, None) };
                }
                return Err(Error::native(error));
            }
        };
        Ok((RendererBackend { base: RendererBackendBase::new(), impl_, context }, surface))
    }

    pub fn backend_impl(&self) -> &Rc<BackendImpl> {
        &self.impl_
    }

    /// The backend's context.
    pub fn vulkan_context(&self) -> &Rc<Context> {
        &self.context
    }

    /// The name of the GPU.
    pub fn get_device_name(&self) -> String {
        self.impl_
            .get_device_properties()
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Whether the device renders on the CPU, as lavapipe does.
    pub fn is_software_device(&self) -> bool {
        self.impl_.get_device_properties().device_type == vk::PhysicalDeviceType::CPU
    }

    /// `context.reset()`, which also releases what the context holds
    /// (`Context::destroy_resources`).
    pub(crate) fn reset_context(&mut self) {
        self.base.reset_context();
        self.context.destroy_resources();
    }

    /// `createContext()`: the context made with the backend.
    pub fn create_context(&self) -> Rc<Context> {
        self.context.clone()
    }

    /// `initShaders`: register one `vulkan::ShaderGroup` per built-in
    /// shader. A duplicate registration is an error, which C++ throws and
    /// the port logs.
    pub fn init_shaders(&self, registry: &ShaderRegistry, program_parameters: &ProgramParameters) {
        for &shader_id in REGISTERED_SHADERS {
            let Some(source) = shader_source(shader_id) else {
                continue;
            };
            let group = Rc::new(ShaderGroup::new(shader_id, program_parameters.clone()));
            if !registry.register_shader_group(group, source.name) {
                Log::error(Event::Shader, format!("Failed to register {} with shader registry!", source.name));
                debug_assert!(false, "duplicate shader group");
            }
        }
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        RendererBackend::init_shaders(self, registry, parameters);
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        RendererBackend::create_context(self)
    }

    /// Vulkan has no context to make current.
    fn activate(&self) {}
    fn deactivate(&self) {}
}

impl Drop for RendererBackend {
    fn drop(&mut self) {
        self.reset_context();
    }
}
