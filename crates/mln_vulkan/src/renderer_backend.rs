// The device and its queue are the `Device`'s, which every backend on it
// shares. What a backend makes on the device, its command pool and its
// allocator, is shared (`Rc<BackendImpl>`) by the backend, its context and
// every resource: C++ reaches them through `Context& -> getBackend()`, and
// destroys the device with the backend; here the device goes with its last
// backend.
//
// What the backend draws into is a target built on it (`OffscreenTarget`,
// `SurfaceTarget`, `BorrowedTarget`). Where C++ asks the default renderable's
// resource for the platform's surface and extensions through virtual calls, a
// window target passes a `SurfaceProvider`.
//
// Differences from C++:
// - C++ turns the validation layer on in debug builds and logs an error when
//   it is missing. The port turns it on in debug builds when it is there,
//   and the environment variable MLN_VULKAN_VALIDATION overrides that: `0`
//   for off, `1` for on, `abort` to also abort the process on a validation
//   error, after printing where it happened, and to make no device without
//   the layer (`Device`).
// - Only the validation layer's warnings and errors are logged.
// - `VK_EXT_debug_report`, the fallback for instances without
//   `VK_EXT_debug_utils`, the RenderDoc frame capture and the Windows
//   monitor layer are not ported.
// - macOS's portability enumeration is left out with the platform.
// - The device needs `VK_KHR_storage_buffer_storage_class`, for the SPIR-V
//   naga writes (`shaders::compiler`).
// - C++ waits for the whole device before it rebuilds a swapchain and when it
//   tears a backend down. Another backend or the host may submit to the
//   device's queue meanwhile, so a backend waits for its own work instead
//   (`BackendImpl::wait_idle`), and never needs every queue at once.

use std::cell::RefCell;
use std::ffi::CStr;
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
use mln_util::logging::{Event, Log};

use crate::context::Context;
use crate::device::Device;
use crate::shaders::shader_group::ShaderGroup;
use crate::shaders::shader_source::shader_source;

/// The platform's part of a backend that renders to a window: what C++'s
/// `SurfaceResource::createPlatformSurface` and the platform's
/// `getInstanceExtensions` do.
pub(crate) trait SurfaceProvider {
    /// The instance extensions the surface needs.
    fn instance_extensions(&self) -> Vec<&'static CStr>;

    /// Creates the window's surface.
    fn create_surface(&self, entry: &ash::Entry, instance: &ash::Instance) -> Result<vk::SurfaceKHR, Error>;
}

/// What a backend makes on its device, which the backend, its context and
/// their resources share.
pub(crate) struct BackendImpl {
    device: Device,
    command_pool: vk::CommandPool,
    max_frames: u32,
    /// Dropped with the backend, before the device.
    allocator: RefCell<ManuallyDrop<Allocator>>,
}

impl BackendImpl {
    /// `init()`'s allocator and command pool on `device`, for `max_frames`
    /// frames in flight.
    fn new(device: &Device, max_frames: u32) -> Result<BackendImpl, Error> {
        const MB: u64 = 1024 * 1024;
        let shared = &device.shared;
        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: shared.instance().clone(),
            device: shared.device().clone(),
            physical_device: shared.physical_device(),
            debug_settings: AllocatorDebugSettings::default(),
            buffer_device_address: false,
            allocation_sizes: AllocationSizes::new(16 * MB, 8 * MB)
                .with_max_device_memblock_size(256 * MB)
                .with_max_host_memblock_size(64 * MB),
        })
        .map_err(|e| Error::native(format!("Vulkan allocator init failed: {e}")))?;
        let create_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(shared.queue_family_index());
        // SAFETY: the queue family is the device's.
        let command_pool = unsafe { shared.device().create_command_pool(&create_info, None) }
            .map_err(|e| Error::native(format!("Failed to create a Vulkan command pool: {e}")))?;
        Ok(BackendImpl {
            device: device.clone(),
            command_pool,
            max_frames,
            allocator: RefCell::new(ManuallyDrop::new(allocator)),
        })
    }

    /// The device the backend renders on.
    pub(crate) fn device(&self) -> &Device {
        &self.device
    }

    /// The loaded Vulkan library.
    pub(crate) fn entry(&self) -> &ash::Entry {
        self.device.shared.entry()
    }

    pub(crate) fn get_instance(&self) -> &ash::Instance {
        self.device.shared.instance()
    }

    pub(crate) fn get_physical_device(&self) -> vk::PhysicalDevice {
        self.device.shared.physical_device()
    }

    pub(crate) fn get_device(&self) -> &ash::Device {
        self.device.shared.device()
    }

    /// `VK_KHR_surface`, when the instance has it.
    pub(crate) fn get_surface_loader(&self) -> Option<&ash::khr::surface::Instance> {
        self.device.shared.surface()
    }

    /// `VK_KHR_swapchain`, when the device has it.
    pub(crate) fn get_swapchain_loader(&self) -> Option<&ash::khr::swapchain::Device> {
        self.device.shared.swapchain()
    }

    /// `VK_KHR_external_semaphore_fd`, when the device has it.
    #[cfg_attr(not(unix), expect(dead_code, reason = "sync files are Unix's"))]
    pub(crate) fn get_semaphore_fd_loader(&self) -> Option<&ash::khr::external_semaphore_fd::Device> {
        self.device.shared.semaphore_fd()
    }

    pub(crate) fn get_command_pool(&self) -> vk::CommandPool {
        self.command_pool
    }

    pub(crate) fn get_max_frames(&self) -> u32 {
        self.max_frames
    }

    /// Runs `f` with the allocator.
    pub(crate) fn with_allocator<R>(&self, f: impl FnOnce(&mut Allocator) -> R) -> R {
        f(&mut self.allocator.borrow_mut())
    }

    pub(crate) fn get_device_properties(&self) -> &vk::PhysicalDeviceProperties {
        self.device.shared.properties()
    }

    /// The features the device was created with.
    pub(crate) fn get_device_features(&self) -> &vk::PhysicalDeviceFeatures {
        self.device.shared.enabled_features()
    }

    /// The family of the device's queue, which renders and presents.
    pub(crate) fn get_graphics_queue_index(&self) -> u32 {
        self.device.shared.queue_family_index()
    }

    /// Submits `submits` to the device's queue, with `fence` signaled when
    /// they finish.
    pub(crate) fn submit(&self, submits: &[vk::SubmitInfo<'_>], fence: vk::Fence) -> ash::prelude::VkResult<()> {
        let device = self.get_device();
        // SAFETY: the caller's submissions are valid for the device, and the
        // queue is held for the call.
        self.device.with_queue(|queue| unsafe { device.queue_submit(queue, submits, fence) })
    }

    /// Presents to the device's queue, which presents to the swapchains
    /// that `info` names. `Ok(true)` when a swapchain is suboptimal.
    pub(crate) fn present(&self, info: &vk::PresentInfoKHR<'_>) -> ash::prelude::VkResult<bool> {
        let Some(loader) = self.get_swapchain_loader() else {
            return Err(vk::Result::ERROR_EXTENSION_NOT_PRESENT);
        };
        // SAFETY: the caller's present info is valid for the device, and the
        // queue is held for the call.
        self.device.with_queue(|queue| unsafe { loader.queue_present(queue, info) })
    }

    /// Names an object for the validation layer's messages and for
    /// debuggers.
    pub(crate) fn set_debug_name<T: vk::Handle>(&self, object: T, name: &str) {
        let Some(debug_utils) = self.device.shared.debug_utils() else {
            return;
        };
        let Ok(name) = std::ffi::CString::new(name) else {
            return;
        };
        let info = vk::DebugUtilsObjectNameInfoEXT::default().object_handle(object).object_name(&name);
        // SAFETY: `object` is a live object of the device.
        let _ = unsafe { debug_utils.set_debug_utils_object_name(&info) };
    }

    pub(crate) fn begin_debug_label(&self, buffer: vk::CommandBuffer, name: &str, color: [f32; 4]) {
        let Some(debug_utils) = self.device.shared.debug_utils() else {
            return;
        };
        let Ok(name) = std::ffi::CString::new(name) else {
            return;
        };
        let label = vk::DebugUtilsLabelEXT::default().label_name(&name).color(color);
        // SAFETY: `buffer` is a command buffer of the device, recording.
        unsafe { debug_utils.cmd_begin_debug_utils_label(buffer, &label) };
    }

    pub(crate) fn end_debug_label(&self, buffer: vk::CommandBuffer) {
        if let Some(debug_utils) = self.device.shared.debug_utils() {
            // SAFETY: as in `begin_debug_label`, which opened the label.
            unsafe { debug_utils.cmd_end_debug_utils_label(buffer) };
        }
    }

    pub(crate) fn insert_debug_label(&self, buffer: vk::CommandBuffer, name: &str) {
        let Some(debug_utils) = self.device.shared.debug_utils() else {
            return;
        };
        let Ok(name) = std::ffi::CString::new(name) else {
            return;
        };
        let label = vk::DebugUtilsLabelEXT::default().label_name(&name);
        // SAFETY: as in `begin_debug_label`.
        unsafe { debug_utils.cmd_insert_debug_utils_label(buffer, &label) };
    }

    /// Waits for the work the backend submitted to the device's queue, by
    /// submitting an empty batch with a fence: the fence signals once
    /// everything submitted to the queue before it has finished. It never
    /// waits on the whole device, which would need every queue of it, nor
    /// holds the queue while it waits. Logs `what` was going on when the
    /// wait fails.
    pub(crate) fn wait_idle(&self, what: &str) {
        let device = self.get_device();
        // SAFETY: the fence is made, waited for and destroyed here, and the
        // empty submission is valid for the queue.
        let waited = unsafe {
            device.create_fence(&vk::FenceCreateInfo::default(), None).is_ok_and(|fence| {
                let done =
                    self.submit(&[], fence).and_then(|()| device.wait_for_fences(&[fence], true, u64::MAX)).is_ok();
                device.destroy_fence(fence, None);
                done
            })
        };
        if !waited {
            Log::error(Event::Render, format!("Vulkan device lost during {what}"));
        }
    }
}

impl Drop for BackendImpl {
    fn drop(&mut self) {
        self.wait_idle("backend shutdown");
        // SAFETY: every user of the pool and the allocator held this struct,
        // so nothing refers to them any more.
        unsafe {
            self.get_device().destroy_command_pool(self.command_pool, None);
            ManuallyDrop::drop(&mut self.allocator.borrow_mut());
        }
    }
}

/// A renderer's instance, device and context.
pub struct RendererBackend {
    base: RendererBackendBase,
    impl_: Rc<BackendImpl>,
    /// The context, made with the backend so that failing to make it is the
    /// backend's error.
    context: Rc<Context>,
}

/// The shader types registered by `initShaders`, in the C++ order.
/// `init_shaders` skips the ones without a Vulkan source in the port
/// (`shader_source`): the wide vector shader.
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
    BuiltIn::RasterShader,
    BuiltIn::SymbolIconShader,
    BuiltIn::SymbolSDFShader,
    BuiltIn::SymbolTextAndIconShader,
    BuiltIn::WideVectorShader,
    BuiltIn::GlobeDepthShader,
];

impl RendererBackend {
    /// A backend on `device` for offscreen and borrowed targets, with one
    /// frame in flight.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when a Vulkan
    /// call fails.
    pub fn new(device: &Device) -> Result<Self, Error> {
        Self::with_frames(device, 1)
    }

    /// A backend on `device` that presents to windows, with two frames in
    /// flight.
    ///
    /// Fails as [`new`](Self::new) does, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the device can't
    /// present.
    pub fn presenting(device: &Device) -> Result<Self, Error> {
        if !device.presents() {
            return Err(Error::unsupported("The Vulkan device presents to no windows"));
        }
        Self::with_frames(device, 2)
    }

    fn with_frames(device: &Device, max_frames: u32) -> Result<Self, Error> {
        let impl_ = Rc::new(BackendImpl::new(device, max_frames)?);
        let context = Context::new(Rc::clone(&impl_)).map_err(Error::native)?;
        Ok(RendererBackend { base: RendererBackendBase::new(), impl_, context })
    }

    pub(crate) fn backend_impl(&self) -> &Rc<BackendImpl> {
        &self.impl_
    }

    /// The backend's context.
    pub(crate) fn vulkan_context(&self) -> &Rc<Context> {
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

    /// Drops the context, releasing what it holds
    /// (`Context::destroy_resources`).
    pub(crate) fn reset_context(&mut self) {
        self.base.reset_context();
        self.context.destroy_resources();
    }

    /// The context made with the backend.
    pub(crate) fn create_context(&self) -> Rc<Context> {
        Rc::clone(&self.context)
    }

    /// Registers one shader group per built-in shader. A duplicate
    /// registration is an error, which C++ throws and this logs.
    #[expect(clippy::unused_self, reason = "each backend registers its shaders through its own `init_shaders`")]
    pub(crate) fn init_shaders(&self, registry: &ShaderRegistry, program_parameters: &ProgramParameters) {
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
