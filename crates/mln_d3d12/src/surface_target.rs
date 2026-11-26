// Frames presented through a DXGI flip-model swap chain: of a Win32 window,
// or for composition, whose swap chain a DirectComposition visual or a XAML
// `SwapChainPanel` shows.
//
// - Frames present with a sync interval of 0 and without tearing, so a frame
//   that the display hasn't shown yet gives way to the next, as Vulkan's
//   mailbox mode does.
// - The swap chain's frame latency waitable object paces the frames: each
//   frame waits on it before it draws, so that no more than `LATENCY` frames
//   wait to be shown. A frame that has waited keeps its turn until it is
//   presented (`waited`).
// - A resize resizes the swap chain's buffers before the next frame, once the
//   GPU is done with them: DXGI resizes buffers that nothing holds.
// - A window that is minimized or has an empty client area takes no frame
//   (`begin_frame`), and presents nothing while it is occluded.

use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::sync::Arc;

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{
    ColorAttachment, Gpu as _, LoadOp, PassDesc, ResourceId, TargetFormats, TextureDesc, TextureFormat, TextureId,
    TextureUsage, destroy_unused,
};
use mln_gfx::window::raw_window_handle::RawWindowHandle;
use mln_gfx::{Error, Size, Window};
use windows::Win32::Foundation::{DXGI_STATUS_OCCLUDED, HANDLE, HWND, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::{D3D12_RESOURCE_STATE_PRESENT, ID3D12Resource};
use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGISwapChain1, IDXGISwapChain3,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;
use windows::core::{GUID, HRESULT, IUnknown, Interface};

use crate::custom_layer::CustomLayerPass;
use crate::device::Device;
use crate::gpu::D3d12Gpu;
use crate::log_target;
use crate::renderer_backend::RendererBackend;

/// The swap chain's buffers: one that the display shows, one that waits to
/// be shown, and one to draw into.
const BUFFERS: u32 = 3;
/// The frames that may wait to be shown.
const LATENCY: u32 = 2;
/// How long a frame waits for its turn before the target skips it.
const WAIT_MILLISECONDS: u32 = 1000;
/// The format of the swap chain's buffers.
const COLOR: TextureFormat = TextureFormat::Bgra8Unorm;
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;
const FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

/// What a composition swap chain is shown by: an `IDCompositionVisual`,
/// whose content the target sets to its swap chain, or a XAML
/// `SwapChainPanel`'s `ISwapChainPanelNative`, of `WinUI` or UWP, whose swap
/// chain the target sets. A `SwapChainPanel` takes its swap chain only on its
/// UI thread, so make the target there.
pub struct Composition(Shown);

enum Shown {
    Visual(IDCompositionVisual),
    /// An object that a query found to be an `ISwapChainPanelNative`.
    SwapChainPanel(IUnknown),
}

// SAFETY: DirectComposition's objects may be called from any thread, and a
// `SwapChainPanel`'s native interface is only moved, and called on its UI
// thread (`SurfaceTarget::for_composition`); the target that holds one is on
// one thread at a time.
unsafe impl Send for Composition {}
// SAFETY: as above.
unsafe impl Sync for Composition {}

impl std::fmt::Debug for Composition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Shown::Visual(_) => f.write_str("Composition::Visual"),
            Shown::SwapChainPanel(_) => f.write_str("Composition::SwapChainPanel"),
        }
    }
}

/// `ISwapChainPanelNative`'s IID, the same in `WinUI` and UWP.
const SWAP_CHAIN_PANEL_NATIVE: GUID = GUID::from_u128(0x63aa_d0b8_7c24_40ff_85a8_640d_944c_c325);

impl Composition {
    /// The host's `IDCompositionVisual`, which the target references.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_gfx::ErrorKind) for
    /// a null pointer.
    ///
    /// # Safety
    ///
    /// `visual` is null or a live `IDCompositionVisual*`.
    pub unsafe fn from_visual(visual: *mut c_void) -> Result<Composition, Error> {
        // SAFETY: the caller's contract; the clone adds a reference.
        unsafe { IDCompositionVisual::from_raw_borrowed(&visual) }
            .cloned()
            .map(|visual| Composition(Shown::Visual(visual)))
            .ok_or_else(|| Error::invalid_argument("a DirectComposition visual is a live IDCompositionVisual"))
    }

    /// The host's `ISwapChainPanelNative`, which the target references.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_gfx::ErrorKind) for
    /// an object that isn't one.
    ///
    /// # Safety
    ///
    /// `panel` is a live COM object.
    pub unsafe fn from_swap_chain_panel(panel: *mut c_void) -> Result<Composition, Error> {
        // SAFETY: the caller's contract.
        let unknown = unsafe { IUnknown::from_raw_borrowed(&panel) }
            .ok_or_else(|| Error::invalid_argument("a SwapChainPanel is a live ISwapChainPanelNative"))?;
        let mut native = std::ptr::null_mut();
        // SAFETY: a plain query, whose result the `IUnknown` then owns.
        unsafe { unknown.query(&SWAP_CHAIN_PANEL_NATIVE, &raw mut native) }
            .ok()
            .map_err(|_| Error::invalid_argument("the object is no ISwapChainPanelNative"))?;
        // SAFETY: the query returned a referenced interface pointer.
        Ok(Composition(Shown::SwapChainPanel(unsafe { IUnknown::from_raw(native) })))
    }

    /// Whether it is a `SwapChainPanel`, which takes its swap chain only on
    /// its UI thread.
    #[must_use]
    pub fn is_swap_chain_panel(&self) -> bool {
        matches!(self.0, Shown::SwapChainPanel(_))
    }

    /// Shows `swap_chain`.
    fn show(&self, swap_chain: &IDXGISwapChain1) -> windows::core::Result<()> {
        match &self.0 {
            // SAFETY: plain calls of live objects.
            Shown::Visual(visual) => unsafe { visual.SetContent(swap_chain) },
            Shown::SwapChainPanel(panel) => {
                // `ISwapChainPanelNative::SetSwapChain`, the method after
                // IUnknown's: windows-rs has no XAML.
                type SetSwapChain = unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT;
                // SAFETY: the object is an `ISwapChainPanelNative`, whose
                // vtable has the method fourth.
                unsafe {
                    let this = panel.as_raw();
                    let vtable = *this.cast::<*const *const c_void>();
                    let set: SetSwapChain = std::mem::transmute(*vtable.add(3));
                    set(this, swap_chain.as_raw()).ok()
                }
            }
        }
    }
}

/// What an overlay draws with ([`SurfaceTarget::set_overlay`]): the
/// backend's `Gpu`, the size of the frame's buffer in pixels, and its format.
pub struct OverlayTarget<'a> {
    /// For the overlay's buffers and textures. What the overlay writes to
    /// them now is written before its draws run, and what it destroys goes
    /// once they are done ([`D3d12Gpu::destroy_later`] for its own objects).
    pub gpu: &'a mut D3d12Gpu,
    pub size: Size,
    pub format: DXGI_FORMAT,
}

/// Draws over each frame of a [`SurfaceTarget`]
/// ([`set_overlay`](SurfaceTarget::set_overlay)).
pub trait Overlay {
    /// Writes what the frame's draws read, before they are recorded.
    fn prepare(&mut self, target: &mut OverlayTarget<'_>);

    /// Records the overlay's draws into a render pass over the frame's
    /// buffer, which keeps what the map drew and has no depth or stencil, as
    /// a custom layer draws ([`CustomLayerPass`]).
    fn draw(&mut self, pass: &CustomLayerPass, size: Size);

    /// Destroys what the overlay made on `gpu`, which no frame it records
    /// will use: it is replaced, or its target goes.
    fn destroy(&mut self, gpu: &mut D3d12Gpu);
}

/// The window or composition that a target presents to.
enum Presentee {
    /// The window, which the target keeps.
    Window(Arc<dyn Window>),
    Composition(#[expect(dead_code, reason = "held, not read")] Composition),
}

/// A swap chain's buffers as textures of the backend's `Gpu`, and a depth
/// and stencil texture of their size.
struct Buffers {
    size: Size,
    color: Vec<TextureId>,
    depth_stencil: TextureId,
}

/// Frames presented through a flip-model swap chain.
pub struct SurfaceTarget {
    device: Device,
    swap_chain: IDXGISwapChain3,
    waitable: OwnedHandle,
    presentee: Presentee,
    /// The size of the frames, which the buffers take before the next
    /// frame.
    size: Size,
    buffers: Option<Buffers>,
    /// Whether a frame has waited for its turn and not yet been presented.
    waited: bool,
    /// The frame begun and not yet ended.
    frame: Option<(usize, FrameImage)>,
    overlay: Option<Box<dyn Overlay>>,
}

fn native(what: &'static str) -> impl Fn(windows::core::Error) -> Error {
    move |error| Error::native(format!("{what}: {error}"))
}

impl SurfaceTarget {
    /// A target on `backend` that presents to `window`, a Win32 window,
    /// through a swap chain of `size` pixels.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_gfx::ErrorKind) for a
    /// window that isn't a Win32 window, with
    /// [`ErrorKind::InvalidState`](mln_gfx::ErrorKind) for one that is
    /// minimized or has no client area, and with
    /// [`ErrorKind::Native`](mln_gfx::ErrorKind) when DXGI makes no swap
    /// chain for it.
    pub fn new(backend: &RendererBackend, window: Arc<dyn Window>, size: Size) -> Result<SurfaceTarget, Error> {
        let handle = window.window_handle().map_err(|e| Error::native(format!("The window has no handle: {e}")))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(Error::unsupported("D3D12 presents to Win32 windows only"));
        };
        if !mln_gfx::window::has_area(&*window) {
            return Err(Error::invalid_state("The window is minimized or has no client area"));
        }
        let hwnd = HWND(handle.hwnd.get() as *mut c_void);
        let device = backend.device().clone();
        let factory = device.factory();
        let desc = swap_chain_desc(size, DXGI_ALPHA_MODE_IGNORE);
        // SAFETY: the queue is the device's, the window is live (the
        // contract of `mln_gfx::Window`), and the description outlives the
        // call.
        let swap_chain = unsafe { factory.CreateSwapChainForHwnd(device.queue(), hwnd, &raw const desc, None, None) }
            .map_err(native("CreateSwapChainForHwnd"))?;
        // Fullscreen is the window's host's to switch, not DXGI's.
        // SAFETY: a plain call of the live window.
        let _ = unsafe { factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER) };
        Self::with_swap_chain(device, &swap_chain, Presentee::Window(window), size)
    }

    /// A target on `backend` that presents through a composition swap chain
    /// of `size` pixels, which `composition` shows. A `DirectComposition`
    /// visual shows it once the host commits its device. Make the target of a
    /// `SwapChainPanel` on the panel's UI thread.
    ///
    /// Fails with [`ErrorKind::Native`](mln_gfx::ErrorKind) when DXGI makes
    /// no swap chain, or `composition` takes none.
    pub fn for_composition(
        backend: &RendererBackend,
        composition: Composition,
        size: Size,
    ) -> Result<SurfaceTarget, Error> {
        let device = backend.device().clone();
        // A composition swap chain has a size even before its host has one.
        let desc = swap_chain_desc(Size::new(size.width.max(1), size.height.max(1)), DXGI_ALPHA_MODE_PREMULTIPLIED);
        // SAFETY: the queue is the device's, and the description outlives
        // the call.
        let swap_chain =
            unsafe { device.factory().CreateSwapChainForComposition(device.queue(), &raw const desc, None) }
                .map_err(native("CreateSwapChainForComposition"))?;
        composition.show(&swap_chain).map_err(native("Showing the swap chain"))?;
        Self::with_swap_chain(device, &swap_chain, Presentee::Composition(composition), size)
    }

    fn with_swap_chain(
        device: Device,
        swap_chain: &IDXGISwapChain1,
        presentee: Presentee,
        size: Size,
    ) -> Result<SurfaceTarget, Error> {
        let swap_chain: IDXGISwapChain3 = swap_chain.cast().map_err(native("IDXGISwapChain3"))?;
        // SAFETY: plain calls of the new swap chain, which has a waitable
        // object (`FLAGS`) that the target then owns.
        let waitable = unsafe {
            swap_chain.SetMaximumFrameLatency(LATENCY).map_err(native("SetMaximumFrameLatency"))?;
            OwnedHandle::from_raw_handle(swap_chain.GetFrameLatencyWaitableObject().0)
        };
        Ok(SurfaceTarget {
            device,
            swap_chain,
            waitable,
            presentee,
            size,
            buffers: None,
            waited: false,
            frame: None,
            overlay: None,
        })
    }

    /// The size of the frames, in physical pixels.
    #[must_use]
    pub fn size(&self) -> Size {
        self.size
    }

    /// Draws `overlay` over each frame from now on, after the map and
    /// before the frame is presented, or stops with `None`. The overlay it
    /// replaces destroys what it made on `gpu` ([`Overlay::destroy`]).
    pub fn set_overlay(&mut self, gpu: &mut D3d12Gpu, overlay: Option<Box<dyn Overlay>>) {
        if let Some(mut old) = std::mem::replace(&mut self.overlay, overlay) {
            old.destroy(gpu);
        }
    }

    /// The buffers at the target's size, made or resized for it in `gpu`:
    /// the textures of buffers of another size go, and the GPU finishes with
    /// them, before the swap chain resizes them.
    fn buffers(&mut self, gpu: &mut D3d12Gpu) -> Result<(), Error> {
        let size = self.size;
        let buffers = &mut self.buffers;
        if buffers.as_ref().is_some_and(|buffers| buffers.size == size) {
            return Ok(());
        }
        if let Some(old) = buffers.take() {
            destroy_unused(gpu, old.color.into_iter().chain([old.depth_stencil]).map(ResourceId::Texture));
            gpu.flush(true).map_err(|error| Error::native(format!("Waiting for the GPU failed: {error}")))?;
            // SAFETY: nothing holds the buffers any more: their textures
            // went with the work that used them.
            unsafe { self.swap_chain.ResizeBuffers(0, size.width, size.height, DXGI_FORMAT_UNKNOWN, FLAGS) }
                .map_err(native("ResizeBuffers"))?;
        } else {
            // SAFETY: a plain query.
            let desc = unsafe { self.swap_chain.GetDesc1() }.map_err(native("GetDesc1"))?;
            if desc.Width != size.width || desc.Height != size.height {
                // SAFETY: nothing holds the new swap chain's buffers.
                unsafe { self.swap_chain.ResizeBuffers(0, size.width, size.height, DXGI_FORMAT_UNKNOWN, FLAGS) }
                    .map_err(native("ResizeBuffers"))?;
            }
        }
        let mut color = Vec::new();
        for index in 0..BUFFERS {
            // SAFETY: a plain query of a buffer the swap chain has.
            let buffer: ID3D12Resource =
                unsafe { self.swap_chain.GetBuffer(index) }.map_err(native("IDXGISwapChain::GetBuffer"))?;
            match gpu.import_texture(&buffer, D3D12_RESOURCE_STATE_PRESENT) {
                Ok(texture) => color.push(texture),
                Err(error) => {
                    destroy_unused(gpu, color.into_iter().map(ResourceId::Texture));
                    return Err(Error::native(format!("Importing a swap chain buffer failed: {error}")));
                }
            }
        }
        let desc = TextureDesc {
            label: "surface depth and stencil",
            size,
            format: DEPTH_STENCIL,
            usage: TextureUsage { render_target: true, ..TextureUsage::default() },
            mipmapped: false,
        };
        let depth_stencil = match gpu.create_texture(&desc) {
            Ok(texture) => texture,
            Err(error) => {
                destroy_unused(gpu, color.into_iter().map(ResourceId::Texture));
                return Err(Error::native(format!("Making the depth and stencil texture failed: {error}")));
            }
        };
        *buffers = Some(Buffers { size, color, depth_stencil });
        Ok(())
    }

    /// Waits for the frame's turn, once until it is presented. Returns
    /// whether it came.
    fn wait_turn(&mut self) -> bool {
        if self.waited {
            return true;
        }
        // SAFETY: the swap chain's waitable object, which the target owns.
        let waited = unsafe { WaitForSingleObjectEx(HANDLE(self.waitable.as_raw_handle()), WAIT_MILLISECONDS, true) }
            == WAIT_OBJECT_0;
        self.waited = waited;
        waited
    }

    /// Draws the overlay over the frame's buffer.
    fn draw_overlay(&mut self, gpu: &mut D3d12Gpu, color: TextureId, size: Size) {
        let Some(overlay) = self.overlay.as_mut() else { return };
        overlay.prepare(&mut OverlayTarget { gpu, size, format: DXGI_FORMAT_B8G8R8A8_UNORM });
        let pass = PassDesc {
            label: "overlay",
            color: Some(ColorAttachment { texture: color, load: LoadOp::Load }),
            depth_stencil: None,
        };
        let drawn = gpu.begin_pass(&pass).and_then(|()| {
            let lent = gpu.lend(&mut |lent| {
                if let Some(pass) = lent.downcast_ref::<CustomLayerPass>() {
                    overlay.draw(pass, size);
                }
            });
            let ended = gpu.end_pass();
            lent.and(ended)
        });
        if let Err(error) = drawn {
            log::error!(target: log_target::RENDER, "Drawing the overlay failed: {error}");
        }
    }
}

/// A flip-model swap chain of `size` pixels, of the target's format.
fn swap_chain_desc(
    size: Size,
    alpha_mode: windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE,
) -> DXGI_SWAP_CHAIN_DESC1 {
    DXGI_SWAP_CHAIN_DESC1 {
        Width: size.width,
        Height: size.height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: BUFFERS,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: alpha_mode,
        Flags: FLAGS.0 as u32,
    }
}

impl FrameTarget for SurfaceTarget {
    type Gpu = D3d12Gpu;

    /// Sets the size of the frames, in pixels, which the swap chain's buffers
    /// take before the next frame. No frame is drawn at an empty size.
    fn resize(&mut self, _gpu: &mut D3d12Gpu, size: Size) {
        self.size = size;
    }

    /// Waits for the frame's turn and takes the swap chain's next buffer. No
    /// frame is drawn at an empty size or into a window without an area.
    fn begin_frame(&mut self, gpu: &mut D3d12Gpu) -> Option<FrameImage> {
        if let Some((_, image)) = self.frame {
            return Some(image);
        }
        let size = self.size;
        if size.width == 0 || size.height == 0 {
            return None;
        }
        if let Presentee::Window(window) = &self.presentee
            && !mln_gfx::window::has_area(&**window)
        {
            return None;
        }
        if let Err(error) = self.buffers(gpu) {
            log::error!(target: log_target::RENDER, "Making the swap chain's buffers failed: {}", error.message);
            return None;
        }
        if !self.wait_turn() {
            return None;
        }
        // SAFETY: a plain query.
        let index = unsafe { self.swap_chain.GetCurrentBackBufferIndex() } as usize;
        let buffers = self.buffers.as_ref()?;
        let image = FrameImage {
            size,
            color: *buffers.color.get(index)?,
            depth_stencil: Some(buffers.depth_stencil),
            targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
        };
        self.frame = Some((index, image));
        Some(image)
    }

    /// Draws the overlay over the frame, submits it, and presents it.
    fn end_frame(&mut self, gpu: &mut D3d12Gpu) -> Handoff {
        let Some((_, image)) = self.frame.take() else {
            return Handoff::Dropped;
        };
        self.draw_overlay(gpu, image.color, image.size);
        if let Err(error) = gpu.flush(false) {
            log::error!(target: log_target::RENDER, "Submitting the frame failed: {error}");
            return Handoff::Dropped;
        }
        // SAFETY: the frame's work went to the queue that the swap chain
        // presents on.
        let presented = unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)) };
        self.waited = false;
        match presented {
            DXGI_STATUS_OCCLUDED => Handoff::Dropped,
            result if result.is_ok() => Handoff::Presented,
            result => {
                let reason = self.device.removed_reason();
                log::error!(
                    target: log_target::RENDER,
                    "Presenting failed: {}",
                    reason.unwrap_or_else(|| windows::core::Error::from_hresult(result).to_string())
                );
                Handoff::Dropped
            }
        }
    }

    /// Destroys the buffers' textures and the overlay's objects, and waits
    /// for the GPU's work on the buffers, which the swap chain releases as
    /// it drops.
    fn tear_down(&mut self, gpu: &mut D3d12Gpu) {
        if let Some(mut overlay) = self.overlay.take() {
            overlay.destroy(gpu);
        }
        self.frame = None;
        if let Some(buffers) = self.buffers.take() {
            destroy_unused(gpu, buffers.color.into_iter().chain([buffers.depth_stencil]).map(ResourceId::Texture));
        }
        let _ = gpu.flush(true);
    }
}
