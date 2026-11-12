// The GPU that Vulkan renderers render on.
//
// A renderer on the library's device makes its own instance and logical
// device on the GPU the device names, on the renderer's thread: the
// backend's objects aren't shared between threads.

use std::sync::{Arc, OnceLock};

use ash::vk;
use mln_util::error::Error;

/// A Vulkan GPU that renderers on any thread render on: the library's, or
/// the host's. Clones name the same one.
#[derive(Clone, Debug)]
pub struct Device {
    kind: Arc<Kind>,
}

#[derive(Debug)]
enum Kind {
    Library { adapter_index: Option<usize> },
    Host(#[allow(dead_code)] HostDevice),
}

/// The host's instance, device and queue, kept for renderers on them, which
/// the backend doesn't make yet.
#[derive(Debug)]
#[allow(dead_code)]
struct HostDevice {
    get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    instance: vk::Instance,
    physical_device: vk::PhysicalDevice,
    device: vk::Device,
    queue_family_index: u32,
    queue: vk::Queue,
}

// SAFETY: Vulkan handles may be used from any thread; the host's contract
// for `from_raw` covers synchronizing the queue.
unsafe impl Send for HostDevice {}
// SAFETY: as above.
unsafe impl Sync for HostDevice {}

/// How many GPUs the platform's Vulkan has, counted once per process. Fails
/// without a Vulkan library, instance or GPU.
fn gpu_count() -> Result<usize, Error> {
    static COUNT: OnceLock<Result<usize, Error>> = OnceLock::new();
    COUNT
        .get_or_init(|| {
            // SAFETY: the Vulkan library's initializers have no preconditions.
            let entry =
                unsafe { ash::Entry::load() }.map_err(|e| Error::unsupported(format!("No Vulkan library: {e}")))?;
            let app_info = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_0);
            let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
            // SAFETY: the create info is valid, with no layers or extensions.
            let instance = unsafe { entry.create_instance(&create_info, None) }
                .map_err(|e| Error::unsupported(format!("Failed to create a Vulkan instance: {e}")))?;
            // SAFETY: a plain query of the live instance, which is destroyed
            // after it with nothing made from it.
            let count = unsafe {
                let count = instance.enumerate_physical_devices().map(|devices| devices.len());
                instance.destroy_instance(None);
                count
            };
            match count {
                Ok(0) => Err(Error::unsupported("No Vulkan compatible GPU found")),
                Ok(count) => Ok(count),
                Err(e) => Err(Error::native(format!("Vulkan physical devices: {e}"))),
            }
        })
        .clone()
}

/// A GPU's ID, as `VkPhysicalDeviceIDProperties` reports it: its
/// `deviceUUID`, or its `deviceLUID`, which on Windows is its DXGI adapter's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdapterId {
    Uuid([u8; vk::UUID_SIZE]),
    Luid([u8; vk::LUID_SIZE]),
}

/// A GPU's UUID, and its LUID where it has one.
type GpuIds = ([u8; vk::UUID_SIZE], Option<[u8; vk::LUID_SIZE]>);

/// The IDs of each GPU, in `vkEnumeratePhysicalDevices` order.
fn adapter_ids() -> Result<Vec<GpuIds>, Error> {
    // SAFETY: the Vulkan library's initializers have no preconditions.
    let entry = unsafe { ash::Entry::load() }.map_err(|e| Error::unsupported(format!("No Vulkan library: {e}")))?;
    // Vulkan 1.1 reports the IDs.
    let app_info = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
    // SAFETY: the create info is valid, with no layers or extensions.
    let instance = unsafe { entry.create_instance(&create_info, None) }
        .map_err(|e| Error::unsupported(format!("Failed to create a Vulkan instance: {e}")))?;
    // SAFETY: plain queries of the live instance and its GPUs, which is
    // destroyed after them with nothing made from it.
    unsafe {
        let ids = instance.enumerate_physical_devices().map(|gpus| {
            gpus.into_iter()
                .map(|gpu| {
                    let mut ids = vk::PhysicalDeviceIDProperties::default();
                    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
                    instance.get_physical_device_properties2(gpu, &mut properties);
                    (ids.device_uuid, (ids.device_luid_valid == vk::TRUE).then_some(ids.device_luid))
                })
                .collect()
        });
        instance.destroy_instance(None);
        ids.map_err(|e| Error::native(format!("Vulkan physical devices: {e}")))
    }
}

impl Device {
    /// The library's device: the GPU at `adapter_index` in
    /// `vkEnumeratePhysicalDevices` order, or the first that can render
    /// when `None`.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) without a
    /// Vulkan library or GPU, with [`ErrorKind::NotFound`](mln_util::ErrorKind)
    /// when there is no GPU at `adapter_index`, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when Vulkan can't list the
    /// GPUs.
    pub fn new(adapter_index: Option<usize>) -> Result<Device, Error> {
        let count = gpu_count()?;
        if let Some(index) = adapter_index
            && index >= count
        {
            return Err(Error::not_found(format!("adapter_index {index} is past the {count} GPUs")));
        }
        Ok(Device { kind: Arc::new(Kind::Library { adapter_index }) })
    }

    /// The library's device on the GPU that `id` names, by the IDs that
    /// `VkPhysicalDeviceIDProperties` reports.
    ///
    /// Fails as [`new`](Self::new) does, and with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when no GPU has `id`.
    pub fn with_adapter_id(id: AdapterId) -> Result<Device, Error> {
        let index = adapter_ids()?
            .iter()
            .position(|(uuid, luid)| match id {
                AdapterId::Uuid(wanted) => *uuid == wanted,
                AdapterId::Luid(wanted) => *luid == Some(wanted),
            })
            .ok_or_else(|| Error::not_found(format!("no Vulkan GPU has the {id:?}")))?;
        Device::new(Some(index))
    }

    /// The host's instance and device, and the queue renderers would submit
    /// to. A renderer on it fails with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind): the library renders
    /// on devices of its own only.
    ///
    /// # Safety
    ///
    /// The handles are live and the host's: `device` is a device of
    /// `physical_device` of `instance`, which `get_instance_proc_addr` loads
    /// the functions of, and `queue` is a queue of `device` in the family
    /// `queue_family_index`. They outlive the device and its renderers.
    pub unsafe fn from_raw(
        get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
        instance: vk::Instance,
        physical_device: vk::PhysicalDevice,
        device: vk::Device,
        queue_family_index: u32,
        queue: vk::Queue,
    ) -> Device {
        Device {
            kind: Arc::new(Kind::Host(HostDevice {
                get_instance_proc_addr,
                instance,
                physical_device,
                device,
                queue_family_index,
                queue,
            })),
        }
    }

    /// The adapter index of the library's device.
    pub(crate) fn library_adapter_index(&self) -> Result<Option<usize>, Error> {
        match &*self.kind {
            Kind::Library { adapter_index } => Ok(*adapter_index),
            Kind::Host(_) => Err(Error::unsupported("the library renders on Vulkan devices of its own only")),
        }
    }
}
