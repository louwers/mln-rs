//! The D3D12 device and its direct queue, which backends on any thread
//! share.

use std::ffi::c_void;
use std::sync::Arc;

use mln_gfx::gpu::{FixedState, GpuError, ShaderKey, TargetFormats, VertexBufferLayout};
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, EventSeverity, Log};
use parking_lot::Mutex;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D_SHADER_MODEL_6_0, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_DRED_ENABLEMENT_FORCED_ON,
    D3D12_FEATURE_DATA_SHADER_MODEL, D3D12_FEATURE_SHADER_MODEL, D3D12_MESSAGE_CALLBACK_FLAG_NONE,
    D3D12_MESSAGE_CATEGORY, D3D12_MESSAGE_ID, D3D12_MESSAGE_SEVERITY, D3D12_MESSAGE_SEVERITY_CORRUPTION,
    D3D12_MESSAGE_SEVERITY_ERROR, D3D12_MESSAGE_SEVERITY_WARNING, D3D12CreateDevice, D3D12GetDebugInterface,
    ID3D12CommandQueue, ID3D12Debug, ID3D12Debug1, ID3D12Device, ID3D12DeviceRemovedExtendedData,
    ID3D12DeviceRemovedExtendedDataSettings, ID3D12InfoQueue, ID3D12InfoQueue1, ID3D12PipelineState,
    ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_DEBUG, DXGI_CREATE_FACTORY_FLAGS,
    IDXGIAdapter1, IDXGIFactory4,
};
use windows::core::{Interface, PCSTR};

use crate::root_signature;
use crate::shaders::ShaderCompiler;

/// What `MLN_D3D12_VALIDATION` asks for: D3D12's debug layer with
/// GPU-based validation and DRED, whose messages go to the log, and with
/// `abort` the end of the process at the first error.
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

/// The pipeline states of a device, by what made them: the program and its
/// defines, the vertex buffers' layouts, the fixed state and the formats.
pub(crate) type PipelineKey = (ShaderKey, Vec<VertexBufferLayout>, FixedState, TargetFormats);

struct Shared {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    /// The factory that found the adapter, which makes swap chains.
    factory: IDXGIFactory4,
    /// The adapter's name and whether it rasterizes on the CPU, as WARP
    /// does.
    name: String,
    software: bool,
    /// The root signature of every pipeline on the device.
    root_signature: ID3D12RootSignature,
    /// The pipeline states that the device's backends made, which the next
    /// backend to need one takes: a renderer for each map makes the same.
    pipelines: Mutex<HashMap<PipelineKey, ID3D12PipelineState>>,
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
                    let mut dred: Option<ID3D12DeviceRemovedExtendedDataSettings> = None;
                    if D3D12GetDebugInterface(&raw mut dred).is_ok()
                        && let Some(dred) = dred
                    {
                        dred.SetAutoBreadcrumbsEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
                        dred.SetPageFaultEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
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
        if validation != Validation::Off {
            log_messages(&device, validation == Validation::Abort);
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
        let root_signature = root_signature::create(&device).map_err(|error| Error::native(error.to_string()))?;
        Ok(Device {
            shared: Arc::new(Shared {
                device,
                queue,
                factory,
                name,
                software,
                root_signature,
                pipelines: Mutex::default(),
            }),
        })
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

impl Device {
    /// Why the device was removed, with the address of the page fault that
    /// DRED recorded, if any; `None` while it works.
    #[must_use]
    pub fn removed_reason(&self) -> Option<String> {
        // SAFETY: plain queries.
        unsafe {
            let error = self.shared.device.GetDeviceRemovedReason().err()?;
            let fault = self
                .shared
                .device
                .cast::<ID3D12DeviceRemovedExtendedData>()
                .and_then(|dred| dred.GetPageFaultAllocationOutput())
                .ok()
                .filter(|output| output.PageFaultVA != 0)
                .map_or_else(String::new, |output| format!(", a page fault at {:#x}", output.PageFaultVA));
            Some(format!("the D3D12 device was removed: {error}{fault}"))
        }
    }
}

/// Sends the debug layer's messages to the log, and with `abort` ends the
/// process at the first error. Without `ID3D12InfoQueue1`, they go to the
/// debugger's output, and `abort` breaks there.
fn log_messages(device: &ID3D12Device, abort: bool) {
    unsafe extern "system" fn message(
        _: D3D12_MESSAGE_CATEGORY,
        severity: D3D12_MESSAGE_SEVERITY,
        _: D3D12_MESSAGE_ID,
        description: PCSTR,
        abort: *mut c_void,
    ) {
        // SAFETY: the debug layer passes a string, valid during the call.
        let text = unsafe { description.to_string() }.unwrap_or_default();
        let error = matches!(severity, D3D12_MESSAGE_SEVERITY_CORRUPTION | D3D12_MESSAGE_SEVERITY_ERROR);
        let severity = match severity {
            D3D12_MESSAGE_SEVERITY_CORRUPTION | D3D12_MESSAGE_SEVERITY_ERROR => EventSeverity::Error,
            D3D12_MESSAGE_SEVERITY_WARNING => EventSeverity::Warning,
            _ => EventSeverity::Info,
        };
        Log::record(severity, Event::Render, -1, &format!("D3D12: {text}"));
        if error && !abort.is_null() {
            std::process::abort();
        }
    }
    let context = if abort { std::ptr::NonNull::<c_void>::dangling().as_ptr() } else { std::ptr::null_mut() };
    // SAFETY: the callback takes the arguments the debug layer passes, and
    // reads `context` only for whether it is null.
    unsafe {
        if let Ok(queue) = device.cast::<ID3D12InfoQueue1>() {
            let mut cookie = 0;
            if queue
                .RegisterMessageCallback(Some(message), D3D12_MESSAGE_CALLBACK_FLAG_NONE, context, &raw mut cookie)
                .is_ok()
            {
                return;
            }
        }
        if abort && let Ok(queue) = device.cast::<ID3D12InfoQueue>() {
            let _ = queue.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_ERROR, true);
            let _ = queue.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_CORRUPTION, true);
        }
    }
}

impl Device {
    /// The DXGI factory of the device's adapter.
    pub(crate) fn factory(&self) -> &IDXGIFactory4 {
        &self.shared.factory
    }

    /// The root signature of every pipeline on the device.
    pub(crate) fn root_signature(&self) -> &ID3D12RootSignature {
        &self.shared.root_signature
    }

    /// The pipeline state of `key` that a backend on the device made, or
    /// the one `create` makes.
    pub(crate) fn pipeline(
        &self,
        key: PipelineKey,
        create: impl FnOnce() -> Result<ID3D12PipelineState, GpuError>,
    ) -> Result<ID3D12PipelineState, GpuError> {
        if let Some(pipeline) = self.shared.pipelines.lock().get(&key) {
            return Ok(pipeline.clone());
        }
        let pipeline = create()?;
        self.shared.pipelines.lock().insert(key, pipeline.clone());
        Ok(pipeline)
    }
}
