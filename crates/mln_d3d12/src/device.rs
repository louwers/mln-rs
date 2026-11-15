//! The D3D12 device and its direct queue, which backends on any thread
//! share.

use std::sync::Arc;

use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D_SHADER_MODEL_6_0, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_FEATURE_DATA_SHADER_MODEL,
    D3D12_FEATURE_SHADER_MODEL, D3D12_MESSAGE_SEVERITY_CORRUPTION, D3D12_MESSAGE_SEVERITY_ERROR, D3D12CreateDevice,
    D3D12GetDebugInterface, ID3D12CommandQueue, ID3D12Debug, ID3D12Debug1, ID3D12Device, ID3D12InfoQueue,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_DEBUG, DXGI_CREATE_FACTORY_FLAGS,
    IDXGIAdapter1, IDXGIFactory4,
};
use windows::core::Interface;

use crate::shaders::ShaderCompiler;

/// What `MLN_D3D12_VALIDATION` asks for: D3D12's debug layer with
/// GPU-based validation, which logs what it finds through the debugger's
/// output, or with `abort` also breaks on an error, which ends a process
/// that runs without a debugger.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Validation {
    Off,
    Log,
    Abort,
}

impl Validation {
    fn from_env() -> Validation {
        match std::env::var("MLN_D3D12_VALIDATION").as_deref() {
            Err(_) | Ok("0" | "off") => Validation::Off,
            Ok("abort") => Validation::Abort,
            Ok(_) => Validation::Log,
        }
    }
}

struct Shared {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    /// The adapter's name and whether it rasterizes on the CPU, as WARP
    /// does.
    name: String,
    software: bool,
}

/// A D3D12 device on one adapter, and its direct queue. Clones share them.
#[derive(Clone)]
pub struct Device {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").field("adapter", &self.shared.name).finish_non_exhaustive()
    }
}

/// How [`Device::create`] picks its adapter.
#[derive(Clone, Copy)]
enum Selector {
    /// An index into the hardware adapters in DXGI's order, then WARP; the
    /// first that makes a device for `None`.
    Index(Option<usize>),
    Luid([u8; 8]),
}

fn native(what: &str) -> impl Fn(windows::core::Error) -> Error + '_ {
    move |error| Error::native(format!("{what}: {error}"))
}

impl Device {
    /// A device on the hardware adapter at `adapter_index` in DXGI's order,
    /// where WARP follows the hardware ones, or with `None` on the first
    /// adapter that makes one.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) where
    /// there is no D3D12 device of feature level 11.0 and shader model 6.0,
    /// or no DXC (`dxcompiler.dll`) to compile its shaders, with
    /// [`ErrorKind::NotFound`](mln_util::ErrorKind) when there is no adapter
    /// at `adapter_index`, and with [`ErrorKind::Native`](mln_util::ErrorKind)
    /// when a D3D12 call fails.
    pub fn new(adapter_index: Option<usize>) -> Result<Device, Error> {
        Self::create(Selector::Index(adapter_index))
    }

    /// A device on the adapter of `luid`, its 8 bytes as Vulkan's
    /// `deviceLUID` has them. Fails as [`new`](Self::new) does.
    pub fn with_luid(luid: [u8; 8]) -> Result<Device, Error> {
        Self::create(Selector::Luid(luid))
    }

    fn create(selector: Selector) -> Result<Device, Error> {
        // DXC compiles every shader; without it the device draws nothing.
        ShaderCompiler::new().map_err(|error| Error::unsupported(error.to_string()))?;
        let mut validation = Validation::from_env();
        if validation != Validation::Off {
            let mut debug: Option<ID3D12Debug> = None;
            // SAFETY: plain calls, before any device is made.
            unsafe {
                if D3D12GetDebugInterface(&raw mut debug).is_ok()
                    && let Some(debug) = debug
                {
                    debug.EnableDebugLayer();
                    if let Ok(debug) = debug.cast::<ID3D12Debug1>() {
                        debug.SetEnableGPUBasedValidation(true);
                    }
                } else {
                    Log::error(Event::Render, "D3D12's debug layer isn't installed");
                    validation = Validation::Off;
                }
            }
        }
        let flags =
            if validation == Validation::Off { DXGI_CREATE_FACTORY_FLAGS(0) } else { DXGI_CREATE_FACTORY_DEBUG };
        // SAFETY: plain calls.
        let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory2(flags) }.map_err(native("CreateDXGIFactory2"))?;
        let (adapter, device) = match selector {
            Selector::Luid(luid) => {
                let luid = LUID {
                    LowPart: u32::from_le_bytes([luid[0], luid[1], luid[2], luid[3]]),
                    HighPart: i32::from_le_bytes([luid[4], luid[5], luid[6], luid[7]]),
                };
                // SAFETY: plain calls.
                let adapter: IDXGIAdapter1 = unsafe { factory.EnumAdapterByLuid(luid) }
                    .map_err(|_| Error::not_found("no DXGI adapter has the LUID"))?;
                let device = create_device(&adapter)?;
                (adapter, device)
            }
            Selector::Index(index) => {
                let adapters = adapters(&factory);
                if let Some(index) = index {
                    let adapter = adapters
                        .into_iter()
                        .nth(index)
                        .ok_or_else(|| Error::not_found(format!("no D3D12 adapter at index {index}")))?;
                    let device = create_device(&adapter)?;
                    (adapter, device)
                } else {
                    let mut error = Error::unsupported("there is no DXGI adapter");
                    let mut found = None;
                    for adapter in adapters {
                        match create_device(&adapter) {
                            Ok(device) => {
                                found = Some((adapter, device));
                                break;
                            }
                            Err(e) => error = e,
                        }
                    }
                    found.ok_or(error)?
                }
            }
        };
        if validation == Validation::Abort
            && let Ok(queue) = device.cast::<ID3D12InfoQueue>()
        {
            // SAFETY: plain calls.
            unsafe {
                let _ = queue.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_ERROR, true);
                let _ = queue.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_CORRUPTION, true);
            }
        }
        // SAFETY: plain calls.
        let desc = unsafe { adapter.GetDesc1() }.map_err(native("IDXGIAdapter1::GetDesc1"))?;
        let name_len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..name_len]);
        let software = desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0;
        let queue_desc = D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() };
        // SAFETY: plain calls.
        let queue: ID3D12CommandQueue =
            unsafe { device.CreateCommandQueue(&raw const queue_desc) }.map_err(native("CreateCommandQueue"))?;
        Log::info(Event::Render, format!("graphics device uses D3D12 adapter {name}"));
        Ok(Device { shared: Arc::new(Shared { device, queue, name, software }) })
    }

    /// The device.
    #[must_use]
    pub fn device(&self) -> &ID3D12Device {
        &self.shared.device
    }

    /// The direct queue that backends on the device submit to.
    #[must_use]
    pub fn queue(&self) -> &ID3D12CommandQueue {
        &self.shared.queue
    }

    /// The adapter's name.
    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.shared.name
    }

    /// Whether the adapter rasterizes on the CPU, as WARP does.
    #[must_use]
    pub fn is_software(&self) -> bool {
        self.shared.software
    }
}

/// The hardware adapters in DXGI's order, then WARP.
fn adapters(factory: &IDXGIFactory4) -> Vec<IDXGIAdapter1> {
    let mut adapters = Vec::new();
    // SAFETY: plain calls.
    unsafe {
        for index in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(index) else { break };
            let software = adapter.GetDesc1().is_ok_and(|desc| desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0);
            if !software {
                adapters.push(adapter);
            }
        }
        if let Ok(warp) = factory.EnumWarpAdapter::<IDXGIAdapter1>() {
            adapters.push(warp);
        }
    }
    adapters
}

/// A device of feature level 11.0 on `adapter` that runs shader model 6.0.
fn create_device(adapter: &IDXGIAdapter1) -> Result<ID3D12Device, Error> {
    let mut device: Option<ID3D12Device> = None;
    // SAFETY: plain calls.
    unsafe { D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &raw mut device) }
        .map_err(|error| Error::unsupported(format!("D3D12CreateDevice: {error}")))?;
    let device = device.ok_or_else(|| Error::unsupported("D3D12CreateDevice made no device"))?;
    let mut model = D3D12_FEATURE_DATA_SHADER_MODEL { HighestShaderModel: D3D_SHADER_MODEL_6_0 };
    // SAFETY: the data is of the feature's type and size.
    let supported = unsafe {
        device.CheckFeatureSupport(
            D3D12_FEATURE_SHADER_MODEL,
            (&raw mut model).cast(),
            size_of::<D3D12_FEATURE_DATA_SHADER_MODEL>() as u32,
        )
    }
    .is_ok();
    if !supported || model.HighestShaderModel.0 < D3D_SHADER_MODEL_6_0.0 {
        return Err(Error::unsupported("the D3D12 device runs no shader model 6.0"));
    }
    Ok(device)
}
