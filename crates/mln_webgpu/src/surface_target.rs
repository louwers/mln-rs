//! Frames presented to a `wgpu::Surface`. The window stays with the host,
//! which makes the surface and reports framebuffer resizes through
//! [`SurfaceTarget::resize`].

use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{Gpu as _, ReadbackId, TargetFormats, TextureId, TextureRegion};
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::gpu::from_wgpu_format;
use crate::renderer_backend::RendererBackend;

/// What an overlay draws into ([`SurfaceTarget::set_overlay`]): the frame's
/// surface texture, after the map and before it is presented.
pub struct OverlayTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    /// In physical pixels.
    pub size: Size,
}

/// Draws over a frame ([`SurfaceTarget::set_overlay`]), submitting its own
/// commands to the queue.
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

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

/// Frames presented to a `wgpu::Surface`.
///
/// The surface is configured when a frame begins at a size it isn't
/// configured at, so a target that replaces another on the same window
/// configures it only after the other one has let go of it. A minimized
/// window's frames keep the size the surface was last configured at.
pub struct SurfaceTarget {
    state: Box<State>,
}

struct State {
    /// The surface texture of the frame begun, until it is presented.
    current: Cell<Option<wgpu::SurfaceTexture>>,
    /// The depth and stencil texture, of the last frame's size.
    depth_stencil: Cell<Option<wgpu::Texture>>,
    overlay: Cell<Option<Overlay>>,
    surface: wgpu::Surface<'static>,

    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    present_mode: wgpu::PresentMode,
    depth_stencil_format: wgpu::TextureFormat,
    /// The size of the frames, as the host last reported it.
    size: Cell<Size>,
    /// The size the surface is configured at; `None` before the first frame
    /// and once the surface has to be configured again.
    configured: Cell<Option<Size>>,

    /// The textures of the frame begun, imported into the backend's
    /// `WebGpu`: the surface texture until the frame ends, and the depth
    /// and stencil texture until it is replaced.
    color_import: Cell<Option<TextureId>>,
    depth_import: Cell<Option<TextureId>>,

    readback_requested: Cell<bool>,
    /// The readback of the frame that `request_readback` asked for, and its
    /// size, until it has the texels.
    pending_readback: Cell<Option<(ReadbackId, Size)>>,
    readback: Cell<Option<PremultipliedImage>>,
    backend: Rc<RendererBackend>,
}

impl SurfaceTarget {
    /// A target on `backend` that presents to `surface`, made with the
    /// instance of the backend's device, at `framebuffer_size` physical
    /// pixels. The surface is configured with the options' format, or the
    /// device's surface format, or else an 8-bit RGBA or BGRA format that
    /// the surface supports: in the browser the one the canvas prefers, and
    /// elsewhere the first of BGRA8 and RGBA8.
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
        let (format, mut usage, vsync_present_mode) = if let Some(adapter) = device.adapter() {
            let capabilities = surface.get_capabilities(adapter);
            if capabilities.formats.is_empty() {
                return Err(Error::unsupported("The WebGPU adapter can't present to the surface"));
            }
            let format = match options.format.or(device.surface_format()) {
                Some(format) if capabilities.formats.contains(&format) => format,
                Some(format) => {
                    return Err(Error::unsupported(format!("The surface does not take the {format:?} format")));
                }
                None => {
                    const RENDERED: [wgpu::TextureFormat; 4] = [
                        wgpu::TextureFormat::Bgra8Unorm,
                        wgpu::TextureFormat::Bgra8UnormSrgb,
                        wgpu::TextureFormat::Rgba8Unorm,
                        wgpu::TextureFormat::Rgba8UnormSrgb,
                    ];
                    // A browser lists the canvas format it prefers first
                    // (`getPreferredCanvasFormat()`), which it presents
                    // without a copy.
                    let format = if cfg!(target_family = "wasm") {
                        capabilities.formats.iter().copied().find(|f| RENDERED.contains(f))
                    } else {
                        RENDERED.into_iter().find(|f| capabilities.formats.contains(f))
                    };
                    format
                        .ok_or_else(|| Error::unsupported("The surface takes none of the formats the map renders in"))?
                }
            };
            let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | (capabilities.usages & wgpu::TextureUsages::COPY_SRC);
            // FIFO blocks in acquire and present on Wayland while the window
            // is hidden, which stalls the host's event loop. Mailbox is in
            // step with the display without blocking.
            let present_mode = if capabilities.present_modes.contains(&wgpu::PresentMode::Mailbox) {
                wgpu::PresentMode::Mailbox
            } else {
                wgpu::PresentMode::Fifo
            };
            (format, usage, present_mode)
        } else {
            let format = options.format.or(device.surface_format()).ok_or_else(|| {
                Error::invalid_argument("A surface on a device without an adapter needs the device's surface format")
            })?;
            (format, wgpu::TextureUsages::RENDER_ATTACHMENT, wgpu::PresentMode::Fifo)
        };
        if let Some(usages) = options.usages {
            usage = wgpu::TextureUsages::RENDER_ATTACHMENT | (usages & wgpu::TextureUsages::COPY_SRC);
        }
        let depth_stencil_format = if device.device().features().contains(wgpu::Features::DEPTH32FLOAT_STENCIL8) {
            wgpu::TextureFormat::Depth32FloatStencil8
        } else {
            wgpu::TextureFormat::Depth24PlusStencil8
        };

        let info = device.adapter_info();
        let name = if info.name.is_empty() { "<unknown>" } else { &info.name };
        Log::info(Event::Render, format!("WebGPU: Selected adapter backend = {}, name = {name}", info.backend));

        Ok(SurfaceTarget {
            state: Box::new(State {
                current: Cell::new(None),
                depth_stencil: Cell::new(None),
                overlay: Cell::new(None),
                surface,
                format,
                usage,
                present_mode: if options.cap_frame_rate { vsync_present_mode } else { wgpu::PresentMode::Immediate },
                depth_stencil_format,
                size: Cell::new(framebuffer_size),
                configured: Cell::new(None),
                color_import: Cell::new(None),
                depth_import: Cell::new(None),
                readback_requested: Cell::new(false),
                pending_readback: Cell::new(None),
                readback: Cell::new(None),
                backend,
            }),
        })
    }

    /// The format the surface is configured with.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.state.format
    }

    /// The size of the frames, in physical pixels.
    #[must_use]
    pub fn size(&self) -> Size {
        self.state.size.get()
    }

    /// Draws the next frame at `size` physical pixels.
    pub fn resize(&self, size: Size) {
        self.state.size.set(size);
    }

    /// Copies the next presented frame, for [`take_readback`](Self::take_readback).
    pub fn request_readback(&self) {
        self.state.readback_requested.set(true);
    }

    /// The frame copied after [`request_readback`](Self::request_readback),
    /// once it is presented and mapped: natively at once, and in the browser
    /// on a later turn of the event loop.
    #[must_use]
    pub fn take_readback(&self) -> Option<PremultipliedImage> {
        self.state.finish_readback(false);
        self.state.readback.take()
    }

    /// Draws `overlay` over each frame from now on, after the map and before
    /// the frame is presented and read back, or stops with `None`.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        self.state.overlay.set(overlay);
    }
}

impl State {
    /// The size of the frame's surface texture, which is acquired, first
    /// configuring the surface, if the frame has none yet. `None` when the
    /// surface gives none, and the frame is skipped.
    fn acquire(&self) -> Option<Size> {
        let size_of = |texture: &wgpu::SurfaceTexture| Size::new(texture.texture.width(), texture.texture.height());
        if let Some(current) = self.current.take() {
            let size = size_of(&current);
            self.current.set(Some(current));
            return Some(size);
        }
        let size = self.size.get();
        // wgpu rejects a configuration without area, which a minimized
        // window reports.
        if self.configured.get() != Some(size) && size.width != 0 && size.height != 0 {
            self.surface.configure(
                self.backend.device().device(),
                &wgpu::SurfaceConfiguration {
                    usage: self.usage,
                    format: self.format,
                    width: size.width,
                    height: size.height,
                    present_mode: self.present_mode,
                    desired_maximum_frame_latency: 2,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![self.format],
                    color_space: wgpu::SurfaceColorSpace::Auto,
                },
            );
            self.configured.set(Some(size));
        }
        self.configured.get()?;
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.configured.set(None);
                return None;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return None,
        };
        let size = size_of(&texture);
        self.current.set(Some(texture));
        Some(size)
    }

    /// The depth and stencil texture of `size`, imported into the backend's
    /// `WebGpu`.
    fn depth_stencil_import(&self, size: Size) -> TextureId {
        let mut gpu = self.backend.gpu();
        let texture = match self.depth_stencil.take() {
            Some(texture) if (texture.width(), texture.height()) == (size.width, size.height) => texture,
            _ => {
                if let Some(import) = self.depth_import.take() {
                    gpu.destroy_texture(import);
                }
                self.backend.device().device().create_texture(&wgpu::TextureDescriptor {
                    label: Some("surface depth and stencil"),
                    size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.depth_stencil_format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
            }
        };
        let import = self.depth_import.get().unwrap_or_else(|| gpu.import_texture(&texture));
        self.depth_import.set(Some(import));
        self.depth_stencil.set(Some(texture));
        import
    }

    /// Starts reading `texture`, the surface's, back.
    fn read_back(&self, texture: &wgpu::Texture) {
        if !self.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            Log::warning(Event::Render, "WebGPU: the surface does not support readback (no COPY_SRC)");
            return;
        }
        let size = Size::new(texture.width(), texture.height());
        let mut gpu = self.backend.gpu();
        let imported = gpu.import_texture(texture);
        let readback = gpu.read_texture(imported, TextureRegion::whole(size));
        gpu.destroy_texture(imported);
        match readback {
            Ok(readback) => {
                if let Some((stale, _)) = self.pending_readback.replace(Some((readback, size))) {
                    gpu.destroy_readback(stale);
                }
            }
            Err(error) => Log::error(Event::Render, format!("WebGPU: reading the frame back failed: {error}")),
        }
    }

    /// Turns the pending readback into an RGBA image once it has the
    /// texels, waiting for them with `wait` where the platform can.
    fn finish_readback(&self, wait: bool) {
        let Some((readback, size)) = self.pending_readback.take() else {
            return;
        };
        let texels = self.backend.gpu().take_readback(readback, wait);
        let mut texels = match texels {
            Ok(Some(texels)) => texels,
            Ok(None) => return self.pending_readback.set(Some((readback, size))),
            Err(error) => {
                return Log::error(Event::Render, format!("WebGPU: reading the frame back failed: {error}"));
            }
        };
        if matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
            for texel in texels.as_chunks_mut::<4>().0 {
                texel.swap(0, 2);
            }
        }
        self.readback.set(Some(PremultipliedImage::from_vec(size, texels)));
    }
}

impl FrameTarget for SurfaceTarget {
    /// Acquires the surface's next texture, unless the frame has one. A
    /// frame without one is skipped.
    fn begin_frame(&self) -> Option<FrameImage> {
        self.state.begin_frame()
    }

    /// Draws the overlay over the frame, reads it back if that was asked
    /// for, and presents it.
    fn end_frame(&self) -> Handoff {
        self.state.end_frame()
    }
}

impl State {
    fn begin_frame(&self) -> Option<FrameImage> {
        let size = self.acquire()?;
        let depth_stencil = self.depth_stencil_import(size);
        let current = self.current.take()?;
        let color = self.color_import.get().unwrap_or_else(|| self.backend.gpu().import_texture(&current.texture));
        self.color_import.set(Some(color));
        self.current.set(Some(current));
        let targets = TargetFormats {
            color: from_wgpu_format(self.format),
            depth_stencil: from_wgpu_format(self.depth_stencil_format),
        };
        Some(FrameImage { size, color, depth_stencil: Some(depth_stencil), targets })
    }

    fn end_frame(&self) -> Handoff {
        if let Some(color) = self.color_import.take() {
            self.backend.gpu().destroy_texture(color);
        }
        let Some(current) = self.current.take() else {
            return Handoff::Dropped;
        };
        if let Some(mut overlay) = self.overlay.take() {
            let device = self.backend.device();
            let view = current.texture.create_view(&wgpu::TextureViewDescriptor::default());
            overlay(&OverlayTarget {
                device: device.device(),
                queue: device.queue(),
                view: &view,
                format: self.format,
                size: Size::new(current.texture.width(), current.texture.height()),
            });
            // Unless the overlay replaced itself.
            let replaced = self.overlay.take();
            self.overlay.set(replaced.or(Some(overlay)));
        }
        if self.readback_requested.replace(false) {
            self.read_back(&current.texture);
            // Natively this waits for the copy; in the browser the event
            // loop maps it later.
            self.finish_readback(true);
        }
        self.backend.device().queue().present(current);
        Handoff::Presented
    }
}

impl Drop for State {
    fn drop(&mut self) {
        // The surface texture goes before the surface.
        drop(self.current.take());
        drop(self.depth_stencil.take());
        drop(self.overlay.take());
        let mut gpu = self.backend.gpu();
        if let Some((readback, _)) = self.pending_readback.take() {
            gpu.destroy_readback(readback);
        }
        for import in [self.color_import.take(), self.depth_import.take()].into_iter().flatten() {
            gpu.destroy_texture(import);
        }
        drop(gpu);
        // Runs the callbacks still outstanding.
        let _ = self.backend.device().device().poll(wgpu::PollType::Poll);
    }
}
