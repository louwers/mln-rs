// The GPU and queue that WebGPU renderers share.
//
// C++ wgpu-native creates the instance with default options (all backends)
// and takes the HighPerformance adapter. The library's device restricts the
// instance to `WGPU_BACKEND` if set (wgpu's env var, e.g. "vulkan"), else
// Vulkan on Linux, so tests run on lavapipe (VK_DRIVER_FILES) like the C++
// CI; other platforms use wgpu's defaults.

#[cfg(not(target_family = "wasm"))]
use mln_util::error::Error;
#[cfg(not(target_family = "wasm"))]
use mln_util::logging::{Event, Log};

/// A WebGPU device that renderers on any thread share: the library's, or the
/// host's. Clones share it.
#[derive(Clone, Debug)]
pub struct Device {
    /// What window surfaces are made with: the instance that made the
    /// device. The host's device may come without it.
    instance: Option<wgpu::Instance>,
    /// What surfaces are checked against. The host's device has none.
    adapter: Option<wgpu::Adapter>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The format the host configures its surfaces with.
    surface_format: Option<wgpu::TextureFormat>,
}

#[cfg(not(target_family = "wasm"))]
fn instance_backends() -> wgpu::Backends {
    if let Some(backends) = wgpu::Backends::from_env() {
        return backends;
    }
    if cfg!(target_os = "linux") { wgpu::Backends::VULKAN } else { wgpu::Backends::all() }
}

/// An instance for the backends `WGPU_BACKEND` names (Vulkan on Linux by
/// default).
#[cfg(not(target_family = "wasm"))]
pub(crate) fn create_instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: instance_backends(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

impl Device {
    /// The library's device, on the adapter at `adapter_index` in
    /// [`wgpu::Instance::enumerate_adapters`] order, or on the
    /// high-performance adapter when `None`. Its uncaptured errors are
    /// logged.
    ///
    /// Fails with [`ErrorKind::NotFound`](mln_util::ErrorKind) when there is
    /// no adapter at `adapter_index`, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) when there is no
    /// adapter at all, and with [`ErrorKind::Native`](mln_util::ErrorKind)
    /// when the adapter can't create the device.
    #[cfg(not(target_family = "wasm"))]
    pub fn new(adapter_index: Option<usize>) -> Result<Device, Error> {
        let instance = create_instance();
        let adapter = match adapter_index {
            Some(index) => {
                let mut adapters = pollster::block_on(instance.enumerate_adapters(instance_backends()));
                let count = adapters.len();
                if index >= count {
                    return Err(Error::not_found(format!("adapter_index {index} is past the {count} adapters")));
                }
                adapters.swap_remove(index)
            }
            None => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }))
            .map_err(|e| Error::unsupported(format!("No adapter found: {e}")))?,
        };

        // The WebGPU default limits lowered to what the adapter supports:
        // the Metal of the iOS, tvOS and visionOS simulators allows fewer
        // inter-stage shader variables than they ask. A surface's depth and
        // stencil take 32-bit float depth where the adapter has it.
        let required_limits = wgpu::Limits::default().or_worse_values_from(&adapter.limits());
        let required_features = adapter.features() & wgpu::Features::DEPTH32FLOAT_STENCIL8;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("MapLibre WebGPU Device"),
            required_features,
            required_limits,
            ..Default::default()
        }))
        .map_err(|e| Error::native(format!("Failed to create device: {e}")))?;

        // C++ sets no uncaptured-error callback, and Dawn logs the error and
        // invalidates the object. wgpu's default handler panics; log instead
        // (Dawn's logUncapturedError).
        device.on_uncaptured_error(std::sync::Arc::new(|error: wgpu::Error| {
            Log::error(Event::Render, format!("WebGPU validation error: {error}"));
        }));
        Ok(Device { instance: Some(instance), adapter: Some(adapter), device, queue, surface_format: None })
    }

    /// The host's device and queue. Window surfaces need the `instance` that
    /// made the device. A surface is configured with `surface_format`, which
    /// it must support, or else with a format it supports that is the first
    /// of BGRA8 and RGBA8 for the device's adapter. The host keeps the
    /// device's uncaptured-error handler.
    pub fn from_wgpu(
        instance: Option<wgpu::Instance>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface_format: Option<wgpu::TextureFormat>,
    ) -> Device {
        Device { instance, adapter: None, device, queue, surface_format }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn instance(&self) -> Option<&wgpu::Instance> {
        self.instance.as_ref()
    }

    pub(crate) fn adapter(&self) -> Option<&wgpu::Adapter> {
        self.adapter.as_ref()
    }

    pub(crate) fn surface_format(&self) -> Option<wgpu::TextureFormat> {
        self.surface_format
    }

    /// The adapter the device is on (name, backend, driver).
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.device.adapter_info()
    }
}
