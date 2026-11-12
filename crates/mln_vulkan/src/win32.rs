// What Vulkan shares with D3D on Windows: the NT handles of D3D12 fences, and
// the D3D11 textures that an exported ring shares through NT handles. Vulkan
// drivers import D3D11 textures (VK_KHR_external_memory_win32), but export no
// memory that D3D11 opens as a texture, so a D3D11 device on the DXGI adapter
// of the Vulkan GPU makes the textures, and each image imports one.
//
// d3d11.dll and dxgi.dll load at run time, as Vulkan's loader does, so that
// the library loads on a Windows without them.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use mln_util::error::Error;
use mln_util::sync::MutexExt;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
    D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIAdapter, IDXGIFactory4, IDXGIResource1,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{BOOL, GUID, HRESULT, Interface as _, PCWSTR, s, w};

/// The `DXGI_FORMAT` of the ring's textures, `DXGI_FORMAT_B8G8R8A8_UNORM`,
/// which Vulkan names `VK_FORMAT_B8G8R8A8_UNORM`.
pub(crate) const FORMAT: u32 = DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32;

/// `CreateDXGIFactory1`.
type CreateFactory = unsafe extern "system" fn(riid: *const GUID, factory: *mut *mut c_void) -> HRESULT;
/// `D3D11CreateDevice`, with the interfaces as raw pointers.
type CreateDevice = unsafe extern "system" fn(
    adapter: *mut c_void,
    driver_type: D3D_DRIVER_TYPE,
    software: HMODULE,
    flags: u32,
    feature_levels: *const D3D_FEATURE_LEVEL,
    feature_level_count: u32,
    sdk_version: u32,
    device: *mut *mut c_void,
    feature_level: *mut D3D_FEATURE_LEVEL,
    context: *mut *mut c_void,
) -> HRESULT;

/// The two functions that make a D3D11 device on an adapter.
struct Functions {
    create_factory: CreateFactory,
    create_device: CreateDevice,
}

/// The DLLs' functions, loaded once, which stay loaded for the process as
/// linked ones would.
fn functions() -> Result<&'static Functions, Error> {
    static FUNCTIONS: OnceLock<Option<Functions>> = OnceLock::new();
    FUNCTIONS
        .get_or_init(|| {
            // SAFETY: the system's DLLs, whose initializers any process may
            // run, and the names of functions of these types.
            unsafe {
                let dxgi = LoadLibraryW(w!("dxgi.dll")).ok()?;
                let d3d11 = LoadLibraryW(w!("d3d11.dll")).ok()?;
                let create_factory = GetProcAddress(dxgi, s!("CreateDXGIFactory1"))?;
                let create_device = GetProcAddress(d3d11, s!("D3D11CreateDevice"))?;
                Some(Functions {
                    create_factory: std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateFactory>(
                        create_factory,
                    ),
                    create_device: std::mem::transmute::<unsafe extern "system" fn() -> isize, CreateDevice>(
                        create_device,
                    ),
                })
            }
        })
        .as_ref()
        .ok_or_else(|| Error::unsupported("This Windows has no D3D11"))
}

/// An NT handle, which dropping closes.
pub(crate) struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Takes `handle`, an NT handle that the caller owns.
    ///
    /// # Safety
    ///
    /// Nothing else closes `handle`.
    pub(crate) unsafe fn from_raw(handle: isize) -> OwnedHandle {
        OwnedHandle(HANDLE(handle as *mut c_void))
    }

    /// The handle, as an integer, as Vulkan takes it.
    pub(crate) fn as_raw(&self) -> isize {
        self.0.0 as isize
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is this one's to close.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

// SAFETY: an NT handle may be used and closed from any thread.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; nothing here changes the handle.
unsafe impl Sync for OwnedHandle {}

/// A D3D11 device on the DXGI adapter of a Vulkan GPU, which makes the
/// textures of the GPU's exported rings.
pub(crate) struct TextureDevice {
    device: ID3D11Device,
    /// The immediate context, which only one thread uses at once.
    context: Mutex<ID3D11DeviceContext>,
}

// SAFETY: a D3D11 device's methods may be called from any thread, and its
// immediate context is used under the mutex.
unsafe impl Send for TextureDevice {}
// SAFETY: as above.
unsafe impl Sync for TextureDevice {}

impl TextureDevice {
    /// A D3D11 device on the adapter whose LUID is `luid`, as Vulkan's
    /// `VkPhysicalDeviceIDProperties.deviceLUID` reports it, once it has
    /// shared a texture through an NT handle.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) without
    /// D3D11, an adapter of `luid`, or textures that it shares, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when D3D11 can't make the
    /// device.
    pub(crate) fn new(luid: [u8; 8]) -> Result<TextureDevice, Error> {
        let functions = functions()?;
        let [l0, l1, l2, l3, h0, h1, h2, h3] = luid;
        let luid =
            LUID { LowPart: u32::from_ne_bytes([l0, l1, l2, l3]), HighPart: i32::from_ne_bytes([h0, h1, h2, h3]) };
        // SAFETY: the DLL's functions, with out pointers that they fill on
        // success, whose objects the interfaces then own.
        unsafe {
            let mut factory = std::ptr::null_mut();
            (functions.create_factory)(&IDXGIFactory4::IID, &mut factory)
                .ok()
                .map_err(|e| Error::unsupported(format!("DXGI has no factory: {e}")))?;
            let factory = IDXGIFactory4::from_raw(factory);
            let adapter: IDXGIAdapter = factory
                .EnumAdapterByLuid(luid)
                .map_err(|e| Error::unsupported(format!("DXGI has no adapter of the Vulkan GPU's LUID: {e}")))?;
            let (mut device, mut context) = (std::ptr::null_mut(), std::ptr::null_mut());
            (functions.create_device)(
                adapter.as_raw(),
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                0,
                std::ptr::null(),
                0,
                D3D11_SDK_VERSION,
                &mut device,
                std::ptr::null_mut(),
                &mut context,
            )
            .ok()
            .map_err(|e| Error::native(format!("D3D11 can't make a device on the Vulkan GPU's adapter: {e}")))?;
            let this = TextureDevice {
                device: ID3D11Device::from_raw(device),
                context: Mutex::new(ID3D11DeviceContext::from_raw(context)),
            };
            // A driver may make shared textures but no NT handles to them.
            this.texture(1, 1)
                .map_err(|e| Error::unsupported(format!("D3D11 shares no textures through NT handles: {e}")))?;
            Ok(this)
        }
    }

    /// A texture of `width` by `height` pixels of [`FORMAT`], which binds as
    /// a shader resource and a render target, and its NT handle, once the
    /// work that made it has finished.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when D3D11
    /// can't make or share the texture.
    pub(crate) fn texture(&self, width: u32, height: u32) -> Result<SharedTexture, Error> {
        let description = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
        };
        let native = |what: &'static str| move |e: windows::core::Error| Error::native(format!("{what}: {e}"));
        // SAFETY: plain calls of the live device, with out pointers that
        // they fill.
        unsafe {
            let mut texture: Option<ID3D11Texture2D> = None;
            self.device
                .CreateTexture2D(&description, None, Some(&mut texture))
                .map_err(native("D3D11 can't make a shared texture"))?;
            let texture = texture.ok_or_else(|| Error::native("D3D11 made no texture"))?;
            let resource: IDXGIResource1 = texture.cast().map_err(native("The texture is no DXGI resource"))?;
            let handle = resource
                .CreateSharedHandle(None, (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0, PCWSTR::null())
                .map_err(native("D3D11 can't share the texture"))?;
            let shared = SharedTexture { _texture: texture, handle: OwnedHandle(handle) };
            // A driver may clear the texture with work on the immediate
            // context, as DXVK does, which must not land after Vulkan renders
            // into it.
            let mut query = None;
            let done = D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 };
            self.device.CreateQuery(&done, Some(&mut query)).map_err(native("D3D11 can't make a query"))?;
            let query = query.ok_or_else(|| Error::native("D3D11 made no query"))?;
            let context = self.context.lock_unpoisoned();
            context.End(&query);
            context.Flush();
            let mut finished = BOOL(0);
            while !finished.as_bool() {
                context
                    .GetData(&query, Some((&raw mut finished).cast()), size_of::<BOOL>() as u32, 0)
                    .map_err(native("Waiting for D3D11 failed"))?;
                std::thread::yield_now();
            }
            Ok(shared)
        }
    }
}

/// A texture of a [`TextureDevice`], and the NT handle that shares it.
pub(crate) struct SharedTexture {
    _texture: ID3D11Texture2D,
    handle: OwnedHandle,
}

// SAFETY: the texture is only kept, and released from whichever thread drops
// it, which D3D11 allows.
unsafe impl Send for SharedTexture {}
// SAFETY: as above.
unsafe impl Sync for SharedTexture {}

impl SharedTexture {
    /// The texture's NT handle, which the texture owns.
    pub(crate) fn handle(&self) -> isize {
        self.handle.as_raw()
    }
}
