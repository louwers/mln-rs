// Frames presented to a `wgpu::Surface`. The window stays with the host,
// which makes the surface, or hands its window over for the target to make
// one, and reports framebuffer resizes through `resize`.
//
// Differences from C++, all structural:
// - The C++ backend reads `glfwGetFramebufferSize` where it needs the size;
//   here the host reports it (`resize`, also what C++'s
//   `onFramebufferResize` calls), so `reconfigureSurface` and the depth
//   texture use the size last reported.
// - C++ reconfigures a resized surface in `swap()`, after rendering one
//   frame into the old-size swapchain texture with the new-size viewport
//   (Dawn logs a validation error for it). The port also reconfigures
//   before acquiring the next texture when a reconfigure is pending.
// - The surface is configured with the first frame, so a target that
//   replaces another on the same window configures it after the other one
//   let go of it.
// - Single-threaded (the renderer runs on one thread): the spin lock,
//   `frameInProgress` (never set in C++) and `isShuttingDown` atomics are
//   plain cells, and `waitForFrame` always succeeds immediately as in C++.
// - Port-only: `request_readback` copies the next presented frame into a
//   `PremultipliedImage` (the surface is configured with COPY_SRC when the
//   surface supports it), so a frame can be checked without screenshots.
//   The copy is mapped asynchronously: natively `swap` waits for it, and in
//   the browser, where nothing can wait for the GPU, `take_readback` returns
//   it once the event loop has mapped it.
// - Port-only: `set_overlay` draws over each frame before it is presented,
//   such as an app's controls over the map.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use mln_gfx::command_encoder::CommandEncoder as GfxCommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::time::Instant;

use crate::command_encoder::CommandEncoder;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::renderer_backend::RendererBackend;

const MAX_CONSECUTIVE_ERRORS: i32 = 3;

fn backend_type_to_string(backend: wgpu::Backend) -> &'static str {
    match backend {
        wgpu::Backend::Noop => "Null",
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        wgpu::Backend::Dx12 => "D3D12",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Gl => "OpenGL",
    }
}

/// The usages and vsync'ed present mode to configure a surface with: `COPY_SRC`
/// for readback where the surface has it, and MAILBOX where it has it (see
/// `SurfaceState::vsync_present_mode`).
fn surface_options(capabilities: &wgpu::SurfaceCapabilities) -> (wgpu::TextureUsages, wgpu::PresentMode) {
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | (capabilities.usages & wgpu::TextureUsages::COPY_SRC);
    let present_mode = if capabilities.present_modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    };
    (usage, present_mode)
}
/// Port-only: what an overlay draws into (`SurfaceTarget::set_overlay`):
/// the frame's swapchain texture, after the map and before it is presented.
pub struct OverlayTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    /// In physical pixels.
    pub size: Size,
}

/// Port-only: draws over a frame (`SurfaceTarget::set_overlay`). It submits
/// its own commands to the queue.
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

/// The current swapchain texture and its view (`currentTexture`,
/// `currentTextureView`).
struct CurrentTexture {
    texture: wgpu::SurfaceTexture,
    view: wgpu::TextureView,
}

/// The members of `GLFWWebGPUBackend` that the draw target reaches through
/// its `backend` reference in C++.
struct SurfaceState {
    surface: RefCell<Option<wgpu::Surface<'static>>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    swap_chain_format: wgpu::TextureFormat,
    surface_usage: Cell<wgpu::TextureUsages>,
    depth_stencil_format: wgpu::TextureFormat,

    current: RefCell<Option<CurrentTexture>>,
    depth_stencil: RefCell<Option<(wgpu::Texture, wgpu::TextureView)>>,

    frame_presented: Cell<bool>,
    surface_configured: Cell<bool>,
    surface_needs_reconfigure: Cell<bool>,
    last_configured_size: Cell<Size>,
    consecutive_errors: Cell<i32>,
    enable_vsync: bool,
    /// symbols (user report: the window froze while hidden): the vsync'ed
    /// present mode. FIFO blocks in acquire/present on Wayland while the
    /// window is not visible (no frame callbacks), which stalls the whole
    /// event loop; MAILBOX is vsync'ed without blocking, so it is used when
    /// the surface supports it.
    vsync_present_mode: Cell<wgpu::PresentMode>,
    /// What `glfwGetFramebufferSize` returns in C++.
    framebuffer_size: Cell<Size>,
    /// When `swap` last ran.
    last_swap_time: Cell<Instant>,

    readback_requested: Cell<bool>,
    /// The copy of the frame `request_readback` asked for, until mapped.
    pending_readback: RefCell<Option<PendingReadback>>,
    readback: RefCell<Option<PremultipliedImage>>,
    overlay: RefCell<Option<Overlay>>,
}

/// Port-only: a frame copied into a buffer, waiting to be mapped.
struct PendingReadback {
    buffer: wgpu::Buffer,
    bytes_per_row: u32,
    size: Size,
    mapped: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

impl SurfaceState {
    fn configuration(&self, size: Size) -> wgpu::SurfaceConfiguration {
        wgpu::SurfaceConfiguration {
            usage: self.surface_usage.get(),
            format: self.swap_chain_format,
            width: size.width,
            height: size.height,
            // Use Immediate mode (no vsync) in benchmark mode, Fifo (vsync) otherwise
            present_mode: if self.enable_vsync { self.vsync_present_mode.get() } else { wgpu::PresentMode::Immediate },
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![self.swap_chain_format],
            color_space: wgpu::SurfaceColorSpace::Auto,
        }
    }

    /// Polls the device without waiting.
    fn process_events(&self) {
        let _ = self.device.poll(wgpu::PollType::Poll);
    }

    /// Presents the current frame. Returns whether there was one.
    fn swap(&self) -> bool {
        let mut presented = false;
        let now = Instant::now();
        let time_since_last_swap = now.duration_since(self.last_swap_time.get());

        // If it's been too long since last swap, tick the device to keep it alive
        if time_since_last_swap > Duration::from_millis(1000) {
            self.process_events();
            // Only reconfigure if really needed (> 10 seconds)
            if time_since_last_swap > Duration::from_millis(10000) && self.surface_configured.get() {
                self.surface_needs_reconfigure.set(true);
            }
        }
        self.last_swap_time.set(now);

        // Note: Command buffer is already submitted in CommandEncoder::present()
        // before swap() is called, so we don't need to submit it again here.

        // Present the current frame
        if self.surface_configured.get() {
            // Check if surface needs reconfiguration
            if self.surface_needs_reconfigure.get() {
                self.reconfigure_surface();
            }

            // Only present if we have a valid texture view and haven't presented yet
            let current = if self.frame_presented.get() { None } else { self.current.borrow_mut().take() };
            {
                if let Some(current) = current {
                    if let Some(overlay) = self.overlay.borrow_mut().as_mut() {
                        overlay(&OverlayTarget {
                            device: &self.device,
                            queue: &self.queue,
                            view: &current.view,
                            format: self.swap_chain_format,
                            size: self.last_configured_size.get(),
                        });
                    }
                    if self.readback_requested.get() {
                        self.readback_requested.set(false);
                        *self.pending_readback.borrow_mut() = self.copy_texture(&current.texture.texture);
                        // Natively this waits for the copy; in the browser it
                        // returns at once and the event loop maps it later.
                        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
                        self.finish_readback();
                    }
                    drop(current.view);
                    self.queue.present(current.texture);
                    self.frame_presented.set(true);
                    presented = true;
                }
            }

            // Reset error counter on successful present
            self.consecutive_errors.set(0);
        }

        // signalFrameComplete(): `frameInProgress` is never set (see the
        // file comment). C++ calls glfwPollEvents here; the host does.
        presented
    }

    fn get_current_texture_view(&self) -> Option<wgpu::TextureView> {
        // Port: a surface that had no size to configure with yet (or was
        // just set) is configured once it has one.
        if !self.surface_configured.get() && self.surface_needs_reconfigure.get() {
            self.reconfigure_surface();
        }
        if !self.surface_configured.get() {
            return None;
        }

        // Check error threshold
        if self.consecutive_errors.get() >= MAX_CONSECUTIVE_ERRORS {
            // Too many errors, need to reconfigure
            self.surface_needs_reconfigure.set(true);
            self.reconfigure_surface();
            self.consecutive_errors.set(0);
        }

        if let Some(current) = self.current.borrow().as_ref()
            && !self.frame_presented.get()
        {
            return Some(current.view.clone());
        }

        // Always release textures before acquiring new ones to avoid "already acquired" error
        self.current.borrow_mut().take();
        self.frame_presented.set(false);

        // Port: apply a pending resize before acquiring (see the file comment).
        if self.surface_needs_reconfigure.get() {
            self.reconfigure_surface();
        }

        let acquired = self.surface.borrow().as_ref()?.get_current_texture();
        let surface_texture = match acquired {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface_needs_reconfigure.set(true);
                self.consecutive_errors.set(self.consecutive_errors.get() + 1);
                return None;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => {
                self.consecutive_errors.set(self.consecutive_errors.get() + 1);
                return None;
            }
        };

        // Create texture view with explicit descriptor
        let view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("SwapChain TextureView"),
            format: Some(self.swap_chain_format),
            dimension: Some(wgpu::TextureViewDimension::D2),
            usage: None,
            aspect: wgpu::TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer: 0,
            array_layer_count: Some(1),
        });

        // Store the texture to keep it alive
        *self.current.borrow_mut() = Some(CurrentTexture { texture: surface_texture, view: view.clone() });

        // Success - reset error counter
        self.consecutive_errors.set(0);
        Some(view)
    }

    fn get_depth_stencil_view(&self) -> Option<wgpu::TextureView> {
        if self.depth_stencil.borrow().is_none() {
            let size = self.framebuffer_size.get();
            self.create_depth_stencil_texture(size.width, size.height);
        }
        self.depth_stencil.borrow().as_ref().map(|(_, view)| view.clone())
    }

    fn reconfigure_surface(&self) {
        // Wait for any in-progress frame: never in progress (file comment).

        let surface = self.surface.borrow();
        let Some(surface) = surface.as_ref() else {
            return;
        };

        // Get current size
        let size = self.framebuffer_size.get();

        // Skip if size hasn't changed and we're already configured. The
        // current texture stays for the frame to present, since Vulkan never
        // gets back an acquired image that is dropped unpresented, and a
        // swapchain that lost each of its images that way has none left.
        if self.surface_configured.get() && self.last_configured_size.get() == size {
            self.surface_needs_reconfigure.set(false);
            return;
        }

        // wgpu (unlike wgpu-native) rejects a zero-sized configuration, which
        // a minimized window reports; keep the old one until it has a size.
        if size.width == 0 || size.height == 0 {
            return;
        }

        // Clear all texture state
        self.current.borrow_mut().take();

        surface.configure(&self.device, &self.configuration(size));

        self.create_depth_stencil_texture(size.width, size.height);

        // Update state
        self.surface_configured.set(true);
        self.surface_needs_reconfigure.set(false);
        self.last_configured_size.set(size);
        self.consecutive_errors.set(0);
    }

    fn create_depth_stencil_texture(&self, width: u32, height: u32) {
        self.depth_stencil.borrow_mut().take();

        let format = self.depth_stencil_format;
        if width == 0 || height == 0 {
            return;
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("DepthStencilTexture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("DepthStencilTextureView"),
            format: Some(format),
            dimension: Some(wgpu::TextureViewDimension::D2),
            usage: None,
            aspect: wgpu::TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer: 0,
            array_layer_count: Some(1),
        });
        *self.depth_stencil.borrow_mut() = Some((texture, view));
    }

    /// Port-only: copies the swapchain texture into a buffer and starts
    /// mapping it (`finish_readback` makes the image).
    fn copy_texture(&self, texture: &wgpu::Texture) -> Option<PendingReadback> {
        if !self.surface_usage.get().contains(wgpu::TextureUsages::COPY_SRC) {
            Log::warning(Event::Render, "WebGPU: the surface does not support readback (no COPY_SRC)");
            return None;
        }
        let (width, height) = (texture.width(), texture.height());
        let bytes_per_row =
            (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Surface readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Surface readback") });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, mapped) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        Some(PendingReadback { buffer, bytes_per_row, size: Size::new(width, height), mapped })
    }

    /// Port-only: turns the pending copy into an RGBA image once it is
    /// mapped (drops it if mapping failed).
    fn finish_readback(&self) {
        let mut pending = self.pending_readback.borrow_mut();
        let Some(readback) = pending.take() else {
            return;
        };
        match readback.mapped.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                *pending = Some(readback);
                return;
            }
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
        }
        let PendingReadback { buffer, bytes_per_row, size, .. } = readback;
        let (width, height) = (size.width, size.height);
        let bgra =
            matches!(self.swap_chain_format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut image = PremultipliedImage::new(size);
        {
            let Ok(mapped) = buffer.get_mapped_range(..) else {
                return;
            };
            for y in 0..height as usize {
                let src = &mapped[y * bytes_per_row as usize..][..width as usize * 4];
                let dst = &mut image.data[y * width as usize * 4..][..width as usize * 4];
                dst.copy_from_slice(src);
                if bgra {
                    for px in dst.as_chunks_mut::<4>().0 {
                        px.swap(0, 2);
                    }
                }
            }
        }
        buffer.unmap();
        *self.readback.borrow_mut() = Some(image);
    }
}

/// The swapchain's texture as a draw target.
struct SurfaceResource {
    state: Rc<SurfaceState>,
}

impl TargetResource for SurfaceResource {
    fn get_color_texture_view(&self) -> Option<wgpu::TextureView> {
        self.state.get_current_texture_view()
    }

    fn get_color_texture_format(&self) -> Option<wgpu::TextureFormat> {
        Some(self.state.swap_chain_format)
    }

    fn get_depth_stencil_texture_view(&self) -> Option<wgpu::TextureView> {
        self.state.get_depth_stencil_view()
    }

    fn get_depth_stencil_texture_format(&self) -> Option<wgpu::TextureFormat> {
        Some(self.state.depth_stencil_format)
    }
}

/// How a [`SurfaceTarget`] presents.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceOptions {
    /// Present in step with the display. Off, frames are presented at once,
    /// tearing.
    pub cap_frame_rate: bool,
    /// The usages the surface supports, where the host knows them better
    /// than the device's adapter does, as for a browser's canvas, or where
    /// the device has no adapter. Frames can be read back when they have
    /// `COPY_SRC`.
    pub usages: Option<wgpu::TextureUsages>,
    /// The format to configure the surface with, in place of the device's
    /// surface format.
    pub format: Option<wgpu::TextureFormat>,
}

impl Default for SurfaceOptions {
    fn default() -> Self {
        SurfaceOptions { cap_frame_rate: true, usages: None, format: None }
    }
}

/// `GLFWWebGPUBackend`'s surface without the window: frames presented to a
/// `wgpu::Surface`.
pub struct SurfaceTarget {
    #[expect(
        dead_code,
        reason = "keeps the backend that the surface's frames render with alive as long as the target"
    )]
    backend: Rc<RendererBackend>,
    state: Rc<SurfaceState>,
    image: Rc<DrawTarget>,
}

impl SurfaceTarget {
    /// A target on `backend` that presents to `surface`, made with the
    /// instance of the backend's device, at `framebuffer_size` physical
    /// pixels. The surface is configured with the options' format, or the
    /// device's surface format, or else the first of BGRA8 and RGBA8 that
    /// the surface supports.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device's adapter can't present to the surface or the surface supports
    /// none of those formats, and with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) when the device
    /// has neither an adapter to ask nor a surface format.
    pub fn new(
        backend: Rc<RendererBackend>,
        surface: wgpu::Surface<'static>,
        framebuffer_size: Size,
        options: SurfaceOptions,
    ) -> Result<SurfaceTarget, Error> {
        let device = backend.device();
        let (swap_chain_format, mut surface_usage, present_mode) = if let Some(adapter) = device.adapter() {
            let capabilities = surface.get_capabilities(adapter);
            if capabilities.formats.is_empty() {
                return Err(Error::unsupported("The WebGPU adapter can't present to the surface"));
            }
            let format = match options.format.or(device.surface_format()) {
                Some(format) if capabilities.formats.contains(&format) => format,
                Some(format) => {
                    return Err(Error::unsupported(format!("The surface does not take the {format:?} format")));
                }
                None => [
                    wgpu::TextureFormat::Bgra8Unorm,
                    wgpu::TextureFormat::Bgra8UnormSrgb,
                    wgpu::TextureFormat::Rgba8Unorm,
                    wgpu::TextureFormat::Rgba8UnormSrgb,
                ]
                .into_iter()
                .find(|f| capabilities.formats.contains(f))
                .ok_or_else(|| Error::unsupported("The surface takes none of the formats the map renders in"))?,
            };
            let (usage, present_mode) = surface_options(&capabilities);
            (format, usage, present_mode)
        } else {
            let format = options.format.or(device.surface_format()).ok_or_else(|| {
                Error::invalid_argument("A surface on a device without an adapter needs the device's surface format")
            })?;
            (format, wgpu::TextureUsages::RENDER_ATTACHMENT, wgpu::PresentMode::Fifo)
        };
        if let Some(usages) = options.usages {
            surface_usage = wgpu::TextureUsages::RENDER_ATTACHMENT | (usages & wgpu::TextureUsages::COPY_SRC);
        }
        let depth_stencil_format = if device.device().features().contains(wgpu::Features::DEPTH32FLOAT_STENCIL8) {
            wgpu::TextureFormat::Depth32FloatStencil8
        } else {
            wgpu::TextureFormat::Depth24PlusStencil8
        };

        let info = device.adapter_info();
        Log::info(
            Event::Render,
            format!(
                "WebGPU: Selected adapter backend = {}, name = {}",
                backend_type_to_string(info.backend),
                if info.name.is_empty() { "<unknown>" } else { &info.name }
            ),
        );

        let state = Rc::new(SurfaceState {
            surface: RefCell::new(Some(surface)),
            device: device.device().clone(),
            queue: device.queue().clone(),
            swap_chain_format,
            surface_usage: Cell::new(surface_usage),
            depth_stencil_format,
            current: RefCell::new(None),
            depth_stencil: RefCell::new(None),
            frame_presented: Cell::new(true),
            surface_configured: Cell::new(false),
            // Configured with the first frame (see the file comment).
            surface_needs_reconfigure: Cell::new(true),
            last_configured_size: Cell::new(Size::default()),
            consecutive_errors: Cell::new(0),
            enable_vsync: options.cap_frame_rate,
            vsync_present_mode: Cell::new(present_mode),
            framebuffer_size: Cell::new(framebuffer_size),
            last_swap_time: Cell::new(Instant::now()),
            readback_requested: Cell::new(false),
            pending_readback: RefCell::new(None),
            readback: RefCell::new(None),
            overlay: RefCell::new(None),
        });
        let image = Rc::new(DrawTarget::new(framebuffer_size, Rc::new(SurfaceResource { state: state.clone() })));
        Ok(SurfaceTarget { backend, state, image })
    }

    /// The format the surface is configured with.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.state.swap_chain_format
    }

    /// The size of the frames, in physical pixels.
    #[must_use]
    pub fn size(&self) -> Size {
        self.state.framebuffer_size.get()
    }

    /// Draws the next frame at `size` physical pixels.
    pub fn resize(&self, size: Size) {
        if self.state.framebuffer_size.replace(size) != size {
            self.image.set_size(size);
            self.state.surface_needs_reconfigure.set(true);
        }
    }

    /// Port-only: copy the next presented frame (see `take_readback`).
    pub fn request_readback(&self) {
        self.state.readback_requested.set(true);
    }

    /// Port-only: the frame copied after `request_readback`, once presented
    /// and mapped (natively at once; in the browser on a later turn of the
    /// event loop).
    #[must_use]
    pub fn take_readback(&self) -> Option<PremultipliedImage> {
        self.state.finish_readback();
        self.state.readback.borrow_mut().take()
    }

    /// Port-only: draws `overlay` over each frame from now on, after the map
    /// and before the frame is presented (and read back), or stops with
    /// `None`.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        *self.state.overlay.borrow_mut() = overlay;
    }
}

impl FrameTarget for SurfaceTarget {
    /// Acquires the surface's next texture, unless the frame has one. A
    /// frame without one is skipped: a render pass without a color
    /// attachment trips the drawables' debug assertions.
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        self.state.get_current_texture_view()?;
        Some(self.image.clone())
    }

    fn end_frame(&self, encoder: &dyn GfxCommandEncoder) -> Handoff {
        if let Some(encoder) = encoder.downcast_ref::<CommandEncoder>() {
            encoder.submit_frame(Some(self.image.resource()));
        }
        if self.state.swap() { Handoff::Presented } else { Handoff::Dropped }
    }
}

impl Drop for SurfaceTarget {
    fn drop(&mut self) {
        // ~GLFWWebGPUBackend: release the textures and pump outstanding
        // callbacks; the swapchain texture goes before the surface.
        self.state.current.borrow_mut().take();
        self.state.depth_stencil.borrow_mut().take();
        self.state.overlay.borrow_mut().take();
        self.state.process_events();
        self.state.surface.borrow_mut().take();
    }
}
