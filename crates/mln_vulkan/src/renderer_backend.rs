// A renderer's backend on a `Device`, which backends on any thread share:
// the command pool and allocator it makes on the device (`BackendImpl`),
// which its `VulkanGpu` and the targets built on it share, and the renderer's
// context. A window target gets its surface from a `SurfaceProvider`.

use std::cell::RefCell;
use std::ffi::CStr;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use ash::vk;
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use gpu_allocator::{AllocationSizes, AllocatorDebugSettings};
use mln_gfx::renderer_backend;
use mln_util::error::Error;

use crate::device::Device;
use crate::gpu::VulkanGpu;

/// The platform's part of a target that presents to a window: its surface,
/// and the instance extensions it needs.
pub(crate) trait SurfaceProvider {
    /// The instance extensions the surface needs.
    fn instance_extensions(&self) -> Vec<&'static CStr>;

    /// Creates the window's surface.
    fn create_surface(&self, entry: &ash::Entry, instance: &ash::Instance) -> Result<vk::SurfaceKHR, Error>;
}

/// What a backend makes on its device, which its `VulkanGpu` and its
/// targets share.
pub(crate) struct BackendImpl {
    device: Device,
    command_pool: vk::CommandPool,
    max_frames: u32,
    /// Dropped with the backend, before the device.
    allocator: RefCell<ManuallyDrop<Allocator>>,
}

impl BackendImpl {
    /// Makes the allocator and the command pool on `device`, for
    /// `max_frames` frames in flight.
    pub(crate) fn new(device: &Device, max_frames: u32) -> Result<BackendImpl, Error> {
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

    /// Waits for the work submitted to the device's queue, as
    /// [`Device::wait_idle`] does.
    pub(crate) fn wait_idle(&self, what: &str) {
        self.device.wait_idle(what);
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

/// A renderer's device and context.
pub struct RendererBackend {
    /// The backend's `Gpu`, which owns the renderer's resources and submits
    /// its frames, made with the backend so that failing to make it is the
    /// backend's error. It holds what the backend made on its device.
    gpu: VulkanGpu,
}

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
        let gpu = VulkanGpu::on(impl_).map_err(|error| Error::native(error.to_string()))?;
        Ok(RendererBackend { gpu })
    }

    pub(crate) fn backend_impl(&self) -> &Rc<BackendImpl> {
        self.gpu.backend_impl()
    }

    /// The name of the GPU.
    #[must_use]
    pub fn get_device_name(&self) -> String {
        self.backend_impl()
            .get_device_properties()
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Whether the device renders on the CPU, as lavapipe does.
    #[must_use]
    pub fn is_software_device(&self) -> bool {
        self.backend_impl().get_device_properties().device_type == vk::PhysicalDeviceType::CPU
    }
}

impl renderer_backend::RendererBackend for RendererBackend {
    type Gpu = VulkanGpu;
    type Scope<'a> = &'a mut VulkanGpu;

    fn gpu(&mut self) -> &mut VulkanGpu {
        &mut self.gpu
    }
}
