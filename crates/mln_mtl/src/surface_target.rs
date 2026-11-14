// Frames presented to a `CAMetalLayer`. The window stays with the host (such
// as examples/desktop), which makes the layer and reports framebuffer resizes
// through `resize`. Each frame's drawable is a texture of the backend's
// `MetalGpu`, which draws the frame into it on the backend's command queue
// and presents it after the frame's last submit.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{Gpu as _, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use objc2::Message;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLPixelFormat, MTLTexture};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

use crate::gpu::MetalGpu;
use crate::mtl_fwd::MTLDevicePtr;
use crate::renderer_backend::RendererBackend;

/// The format of the layer's drawables.
const COLOR_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;
const COLOR: TextureFormat = TextureFormat::Bgra8Unorm;
/// Metal's one depth and stencil format of every GPU.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth32FloatStencil8;

/// What an overlay draws into (`SurfaceTarget::set_overlay`):
/// the frame's drawable, after the map and before it is presented.
pub struct OverlayTarget<'a> {
    pub device: &'a ProtocolObject<dyn MTLDevice>,
    /// A command buffer of the overlay's, which the overlay encodes its
    /// passes into, committed after the frame's.
    pub command_buffer: &'a ProtocolObject<dyn MTLCommandBuffer>,
    pub texture: &'a ProtocolObject<dyn MTLTexture>,
    /// In physical pixels.
    pub size: Size,
}

/// Draws over a frame (`SurfaceTarget::set_overlay`).
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

/// The frame's drawable, and the texture of the `Gpu` it is.
struct Frame {
    drawable: Retained<ProtocolObject<dyn CAMetalDrawable>>,
    color: TextureId,
}

/// A `CAMetalLayer` that a [`SurfaceTarget`] presents to, which may move to
/// the thread that renders.
pub struct Layer(Retained<CAMetalLayer>);

// SAFETY: a target uses its layer on the thread that renders only, to take
// drawables and set their size and format, which Core Animation allows off
// the main thread.
unsafe impl Send for Layer {}

impl Layer {
    #[must_use]
    pub fn new(layer: Retained<CAMetalLayer>) -> Layer {
        Layer(layer)
    }

    /// The layer `layer` points to, retained.
    ///
    /// # Safety
    ///
    /// `layer` is a live `CAMetalLayer`.
    #[must_use]
    pub unsafe fn from_raw(layer: std::ptr::NonNull<std::ffi::c_void>) -> Layer {
        // SAFETY: the caller's contract.
        Layer(unsafe { layer.cast::<CAMetalLayer>().as_ref() }.retain())
    }
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Layer").finish_non_exhaustive()
    }
}

/// Frames presented to a `CAMetalLayer`.
pub struct SurfaceTarget {
    gpu: Rc<RefCell<MetalGpu>>,
    device: MTLDevicePtr,
    layer: Retained<CAMetalLayer>,
    size: Cell<Size>,
    /// The frame's drawable, from `begin_frame` until `end_frame`.
    frame: RefCell<Option<Frame>>,
    /// The depth and stencil texture, of the size of the drawables it was
    /// made for.
    depth_stencil: Cell<Option<(Size, TextureId)>>,
    readback_requested: Cell<bool>,
    readback: RefCell<Option<PremultipliedImage>>,
    overlay: RefCell<Option<Overlay>>,
}

impl SurfaceTarget {
    /// A target on `backend` that presents to `layer`, whose drawables it
    /// sizes to `framebuffer_size`. With `cap_frame_rate` off, frames are
    /// presented without waiting for the display.
    pub fn new(backend: &RendererBackend, layer: Layer, framebuffer_size: Size, cap_frame_rate: bool) -> SurfaceTarget {
        let Layer(layer) = layer;
        let device = backend.device().device().clone();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(COLOR_FORMAT);
        layer.setFramebufferOnly(true);
        #[cfg(target_os = "macos")]
        layer.setDisplaySyncEnabled(cap_frame_rate);
        #[cfg(not(target_os = "macos"))]
        let _ = cap_frame_rate;
        let target = SurfaceTarget {
            gpu: Rc::clone(backend.shared_gpu()),
            device,
            layer,
            size: Cell::new(Size::new(0, 0)),
            frame: RefCell::new(None),
            depth_stencil: Cell::new(None),
            readback_requested: Cell::new(false),
            readback: RefCell::new(None),
            overlay: RefCell::new(None),
        };
        target.resize(framebuffer_size);
        target
    }

    /// The format of the layer's drawables.
    pub fn get_pixel_format(&self) -> MTLPixelFormat {
        COLOR_FORMAT
    }

    /// The size of the frames, in physical pixels.
    pub fn size(&self) -> Size {
        self.size.get()
    }

    /// Sets the size of the drawables, in pixels.
    pub fn resize(&self, size: Size) {
        self.size.set(size);
        if size.width > 0 && size.height > 0 {
            self.layer.setDrawableSize(CGSize::new(f64::from(size.width), f64::from(size.height)));
        }
    }

    /// Copies the next presented frame (see `take_readback`). The
    /// layer's drawables can be read until that copy, from the next one it
    /// hands out: a drawable taken ahead of the next frame is given back,
    /// and one that a frame is drawing into now has its copy in the frame
    /// after.
    pub fn request_readback(&self) {
        self.layer.setFramebufferOnly(false);
        self.readback_requested.set(true);
        let taken = self.frame.borrow_mut().take_if(|frame| frame.drawable.texture().isFramebufferOnly());
        if let Some(frame) = taken {
            self.gpu.borrow_mut().destroy_texture(frame.color);
        }
    }

    /// Draws `overlay` over each frame from now on, after the map
    /// and before the frame is presented (and read back), or stops with
    /// `None`.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        *self.overlay.borrow_mut() = overlay;
    }

    /// The frame copied after `request_readback`, once presented.
    pub fn take_readback(&self) -> Option<PremultipliedImage> {
        self.readback.borrow_mut().take()
    }
}

impl SurfaceTarget {
    /// The frame's drawable as a texture of the `Gpu`, taken from the layer
    /// unless the frame has one; `None` when the layer has none to give (it
    /// waits up to a second). Call it in an autorelease pool.
    fn frame_image(&self) -> Option<FrameImage> {
        let mut frame = self.frame.borrow_mut();
        let mut gpu = self.gpu.borrow_mut();
        if frame.is_none() {
            let drawable = self.layer.nextDrawable()?;
            let color = match gpu.import_texture(&drawable.texture(), COLOR) {
                Ok(color) => color,
                Err(error) => {
                    Log::error(Event::Render, format!("Importing the layer's drawable failed: {error}"));
                    return None;
                }
            };
            *frame = Some(Frame { drawable, color });
        }
        let frame = frame.as_ref()?;
        let texture = frame.drawable.texture();
        let size = Size::new(texture.width() as u32, texture.height() as u32);
        let depth_stencil = match self.depth_stencil.get() {
            Some((made, depth_stencil)) if made == size => depth_stencil,
            made => {
                if let Some((_, old)) = made {
                    gpu.destroy_texture(old);
                }
                self.depth_stencil.set(None);
                let desc = TextureDesc {
                    label: "SurfaceDepthStencil",
                    size,
                    format: DEPTH_STENCIL,
                    usage: TextureUsage { sampled: false, render_target: true, copy_src: false },
                };
                match gpu.create_texture(&desc) {
                    Ok(depth_stencil) => {
                        self.depth_stencil.set(Some((size, depth_stencil)));
                        depth_stencil
                    }
                    Err(error) => {
                        Log::error(Event::Render, format!("Making the depth and stencil texture failed: {error}"));
                        return None;
                    }
                }
            }
        };
        Some(FrameImage {
            size,
            color: frame.color,
            depth_stencil: Some(depth_stencil),
            targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
        })
    }

    /// Draws the overlay over the frame, reads it back if asked, and
    /// presents it. Returns whether the frame had a drawable. Call it in an
    /// autorelease pool.
    fn present(&self) -> bool {
        let Some(Frame { drawable, color }) = self.frame.borrow_mut().take() else {
            return false;
        };
        let texture = drawable.texture();
        let size = Size::new(texture.width() as u32, texture.height() as u32);
        let mut gpu = self.gpu.borrow_mut();
        if let Some(overlay) = self.overlay.borrow_mut().as_mut() {
            gpu.take_waits_into_next_submit(color);
            let submitted = gpu.submit_native(|command_buffer| {
                overlay(&OverlayTarget { device: &self.device, command_buffer, texture: &texture, size });
            });
            if let Err(error) = submitted {
                Log::error(Event::Render, format!("Submitting the overlay failed: {error}"));
            }
        }

        let mut readback = None;
        // A drawable taken before the request can't be read: the next frame
        // is.
        if self.readback_requested.get() && !texture.isFramebufferOnly() {
            self.readback_requested.set(false);
            match gpu.read_texture(color, TextureRegion::whole(size)) {
                Ok(id) => readback = Some(id),
                Err(error) => Log::error(Event::Render, format!("Reading the frame back failed: {error}")),
            }
            self.layer.setFramebufferOnly(true);
        }

        gpu.take_waits_into_next_submit(color);
        gpu.present_after_next_submit(ProtocolObject::from_ref(&*drawable));
        if let Err(error) = gpu.submit(&[]) {
            Log::error(Event::Render, format!("Presenting the frame failed: {error}"));
        }
        gpu.destroy_texture(color);

        if let Some(id) = readback {
            match gpu.take_readback(id, true) {
                Ok(Some(mut bgra)) => {
                    for pixel in bgra.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    *self.readback.borrow_mut() = Some(PremultipliedImage::from_vec(size, bgra));
                }
                Ok(None) => {}
                Err(error) => Log::error(Event::Render, format!("Reading the frame back failed: {error}")),
            }
        }
        true
    }
}

impl FrameTarget for SurfaceTarget {
    /// Takes the layer's next drawable unless the frame has one, or `None`
    /// when the layer has none right now. A frame without one is skipped: a
    /// render pass without a color attachment trips the drawables' debug
    /// assertions.
    fn begin_frame(&self) -> Option<FrameImage> {
        autoreleasepool(|_| self.frame_image())
    }

    fn end_frame(&self) -> Handoff {
        if autoreleasepool(|_| self.present()) { Handoff::Presented } else { Handoff::Dropped }
    }
}

impl Drop for SurfaceTarget {
    fn drop(&mut self) {
        self.overlay.borrow_mut().take();
        let mut gpu = self.gpu.borrow_mut();
        if let Some(frame) = self.frame.borrow_mut().take() {
            gpu.destroy_texture(frame.color);
        }
        if let Some((_, depth_stencil)) = self.depth_stencil.take() {
            gpu.destroy_texture(depth_stencil);
        }
    }
}
