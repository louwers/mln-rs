// Frames presented to a `CAMetalLayer`. The window stays with the host (such
// as examples/desktop), which makes the layer, or hands over a window whose
// view the layer goes in (`Layer::of_window`), and reports framebuffer
// resizes through `resize`. Each frame's drawable is a texture of the
// backend's `MetalGpu`, which draws the frame into it on the backend's
// command queue and presents it after the frame's last submit.

use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{
    Gpu as _, ResourceId, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage,
    destroy_unused,
};
use mln_gfx::window::Window;
use mln_gfx::window::raw_window_handle::RawWindowHandle;
use mln_gfx::{Error, FramePixels, Size};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, Message};
use objc2_core_foundation::CGSize;
use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLPixelFormat, MTLTexture};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer, CATransaction};

use crate::gpu::MetalGpu;
use crate::renderer_backend::RendererBackend;
use crate::{Id, log_target};

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
    drawable: Id<dyn CAMetalDrawable>,
    color: TextureId,
}

/// A `CAMetalLayer` that a [`SurfaceTarget`] presents to, which may move to
/// the thread that renders.
pub struct Layer {
    layer: Retained<CAMetalLayer>,
    /// Whether the layer is a sublayer that [`Layer::of_window`] put in the
    /// window's view, which the target takes out of the view when it drops.
    made: bool,
}

// SAFETY: a target uses its layer on the thread that renders only, to take
// drawables and set their size and format, which Core Animation allows off
// the main thread.
unsafe impl Send for Layer {}

impl Layer {
    #[must_use]
    pub fn new(layer: Retained<CAMetalLayer>) -> Layer {
        Layer { layer, made: false }
    }

    /// The layer `layer` points to, retained.
    ///
    /// # Safety
    ///
    /// `layer` is a live `CAMetalLayer`.
    #[must_use]
    pub unsafe fn from_raw(layer: std::ptr::NonNull<std::ffi::c_void>) -> Layer {
        // SAFETY: the caller's contract.
        Layer::new(unsafe { layer.cast::<CAMetalLayer>().as_ref() }.retain())
    }

    /// The `CAMetalLayer` of `window`'s `NSView` or `UIView`: the view's own
    /// layer where that is one, and otherwise a sublayer that follows the
    /// view's bounds and scale, which the target that presents to it takes
    /// out of the view when it drops.
    ///
    /// Fails with [`ErrorKind::Native`](mln_gfx::ErrorKind) when the window
    /// has no handle, with [`ErrorKind::Unsupported`](mln_gfx::ErrorKind)
    /// for a window of neither view, and with
    /// [`ErrorKind::InvalidState`](mln_gfx::ErrorKind) off the main thread,
    /// the only one where `AppKit` and `UIKit` take a view's layers.
    pub fn of_window(window: &dyn Window) -> Result<Layer, Error> {
        let handle = window.window_handle().map_err(|e| Error::native(format!("the window has no handle: {e}")))?;
        let (view, app_kit) = match handle.as_raw() {
            RawWindowHandle::AppKit(handle) => (handle.ns_view, true),
            RawWindowHandle::UiKit(handle) => (handle.ui_view, false),
            other => return Err(Error::unsupported(format!("Metal presents to no {other:?} window"))),
        };
        if MainThreadMarker::new().is_none() {
            return Err(Error::invalid_state(
                "a window's Metal layer is made on the main thread: attach the window with Driver::CallerThread \
                 serviced there, or present to a CAMetalLayer of your own",
            ));
        }
        // SAFETY: the view is live while the window is, and this runs on the
        // main thread.
        let layer = unsafe {
            if app_kit {
                raw_window_metal::Layer::from_ns_view(view)
            } else {
                raw_window_metal::Layer::from_ui_view(view)
            }
        };
        let made = !layer.pre_existing();
        // SAFETY: `into_raw` gives up a +1 retained, non-null CAMetalLayer.
        let layer = unsafe { Retained::from_raw(layer.into_raw().as_ptr().cast::<CAMetalLayer>()) }
            .ok_or_else(|| Error::native("the window's view has no layer"))?;
        Ok(Layer { layer, made })
    }
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Layer").finish_non_exhaustive()
    }
}

/// Frames presented to a `CAMetalLayer`.
pub struct SurfaceTarget {
    device: Id<dyn MTLDevice>,
    layer: Retained<CAMetalLayer>,
    /// Whether the target takes the layer out of its view when it drops
    /// (`Layer::made`).
    remove_layer: bool,
    size: Size,
    /// The frame's drawable, from `begin_frame` until `end_frame`.
    frame: Option<Frame>,
    /// The depth and stencil texture, of the size of the drawables it was
    /// made for.
    depth_stencil: Option<(Size, TextureId)>,
    readback_requested: bool,
    readback: Option<FramePixels>,
    overlay: Option<Overlay>,
}

impl SurfaceTarget {
    /// A target on `backend` that presents to `layer`, whose drawables it
    /// sizes to `framebuffer_size`. With `cap_frame_rate` off, frames are
    /// presented without waiting for the display.
    #[must_use]
    pub fn new(backend: &RendererBackend, layer: Layer, framebuffer_size: Size, cap_frame_rate: bool) -> SurfaceTarget {
        let Layer { layer, made: remove_layer } = layer;
        let device = backend.device().device().clone();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(COLOR_FORMAT);
        layer.setFramebufferOnly(true);
        #[cfg(target_os = "macos")]
        layer.setDisplaySyncEnabled(cap_frame_rate);
        #[cfg(not(target_os = "macos"))]
        let _ = cap_frame_rate;
        let mut target = SurfaceTarget {
            device,
            layer,
            remove_layer,
            size: Size::new(0, 0),
            frame: None,
            depth_stencil: None,
            readback_requested: false,
            readback: None,
            overlay: None,
        };
        target.set_size(framebuffer_size);
        target
    }

    /// The format of the layer's drawables.
    #[must_use]
    pub fn get_pixel_format(&self) -> MTLPixelFormat {
        COLOR_FORMAT
    }

    /// The size of the frames, in physical pixels.
    #[must_use]
    pub fn size(&self) -> Size {
        self.size
    }

    /// Sets the size of the drawables, in pixels.
    fn set_size(&mut self, size: Size) {
        self.size = size;
        if size.width > 0 && size.height > 0 {
            self.layer.setDrawableSize(CGSize::new(f64::from(size.width), f64::from(size.height)));
        }
    }

    /// Copies the next presented frame (see `take_readback`). The
    /// layer's drawables can be read until that copy, from the next one it
    /// hands out: a drawable taken ahead of the next frame is given back,
    /// and one that a frame is drawing into now has its copy in the frame
    /// after.
    pub fn request_readback(&mut self, gpu: &mut MetalGpu) {
        self.layer.setFramebufferOnly(false);
        self.readback_requested = true;
        if let Some(frame) = self.frame.take_if(|frame| frame.drawable.texture().isFramebufferOnly()) {
            destroy_unused(gpu, [ResourceId::Texture(frame.color)]);
        }
    }

    /// Draws `overlay` over each frame from now on, after the map
    /// and before the frame is presented (and read back), or stops with
    /// `None`.
    pub fn set_overlay(&mut self, overlay: Option<Overlay>) {
        self.overlay = overlay;
    }

    /// The frame copied after `request_readback`, once presented.
    pub fn take_readback(&mut self) -> Option<FramePixels> {
        self.readback.take()
    }
}

impl SurfaceTarget {
    /// The frame's drawable as a texture of `gpu`, taken from the layer
    /// unless the frame has one; `None` when the layer has none to give (it
    /// waits up to a second). Call it in an autorelease pool.
    fn frame_image(&mut self, gpu: &mut MetalGpu) -> Option<FrameImage> {
        if self.frame.is_none() {
            let drawable = self.layer.nextDrawable()?;
            let color = match gpu.import_texture(&drawable.texture(), COLOR) {
                Ok(color) => color,
                Err(error) => {
                    log::error!(target: log_target::RENDER, "Importing the layer's drawable failed: {error}");
                    return None;
                }
            };
            self.frame = Some(Frame { drawable, color });
        }
        let frame = self.frame.as_ref()?;
        let texture = frame.drawable.texture();
        let size = Size::new(texture.width() as u32, texture.height() as u32);
        let depth_stencil = match self.depth_stencil {
            Some((made, depth_stencil)) if made == size => depth_stencil,
            made => {
                if let Some((_, old)) = made {
                    destroy_unused(gpu, [ResourceId::Texture(old)]);
                }
                self.depth_stencil = None;
                let desc = TextureDesc {
                    label: "SurfaceDepthStencil",
                    size,
                    format: DEPTH_STENCIL,
                    usage: TextureUsage { sampled: false, render_target: true, copy_src: false },
                    mipmapped: false,
                };
                match gpu.create_texture(&desc) {
                    Ok(depth_stencil) => self.depth_stencil.insert((size, depth_stencil)).1,
                    Err(error) => {
                        log::error!(target: log_target::RENDER, "Making the depth and stencil texture failed: {error}");
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
    fn present(&mut self, gpu: &mut MetalGpu) -> bool {
        let Some(Frame { drawable, color }) = self.frame.take() else {
            return false;
        };
        let texture = drawable.texture();
        let size = Size::new(texture.width() as u32, texture.height() as u32);
        if let Some(overlay) = self.overlay.as_mut() {
            gpu.take_waits_into_next_submit(color);
            let device = &self.device;
            let submitted = gpu.submit_native(|command_buffer| {
                overlay(&OverlayTarget { device, command_buffer, texture: &texture, size });
            });
            if let Err(error) = submitted {
                log::error!(target: log_target::RENDER, "Submitting the overlay failed: {error}");
            }
        }

        let mut readback = None;
        // A drawable taken before the request can't be read: the next frame
        // is.
        if self.readback_requested && !texture.isFramebufferOnly() {
            self.readback_requested = false;
            match gpu.read_texture(color, TextureRegion::whole(size)) {
                Ok(id) => readback = Some(id),
                Err(error) => log::error!(target: log_target::RENDER, "Reading the frame back failed: {error}"),
            }
            self.layer.setFramebufferOnly(true);
        }

        gpu.take_waits_into_next_submit(color);
        gpu.present_after_next_submit(ProtocolObject::from_ref(&*drawable));
        if let Err(error) = gpu.flush(false) {
            log::error!(target: log_target::RENDER, "Presenting the frame failed: {error}");
        }
        destroy_unused(gpu, [ResourceId::Texture(color)]);

        if let Some(id) = readback {
            match gpu.take_readback(id, true) {
                Ok(Some(mut bgra)) => {
                    for pixel in bgra.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    self.readback = Some(FramePixels { size, data: bgra });
                }
                Ok(None) => {}
                Err(error) => log::error!(target: log_target::RENDER, "Reading the frame back failed: {error}"),
            }
        }
        true
    }
}

impl FrameTarget for SurfaceTarget {
    type Gpu = MetalGpu;

    fn resize(&mut self, _gpu: &mut MetalGpu, size: Size) {
        self.set_size(size);
    }

    /// Takes the layer's next drawable unless the frame has one, or `None`
    /// when the layer has none right now. A frame without one is skipped: a
    /// render pass without a color attachment trips the drawables' debug
    /// assertions.
    fn begin_frame(&mut self, gpu: &mut MetalGpu) -> Option<FrameImage> {
        autoreleasepool(|_| self.frame_image(gpu))
    }

    fn end_frame(&mut self, gpu: &mut MetalGpu) -> Handoff {
        if autoreleasepool(|_| self.present(gpu)) { Handoff::Presented } else { Handoff::Dropped }
    }

    fn tear_down(&mut self, gpu: &mut MetalGpu) {
        self.overlay = None;
        if let Some(frame) = self.frame.take() {
            destroy_unused(gpu, [ResourceId::Texture(frame.color)]);
        }
        if let Some((_, depth_stencil)) = self.depth_stencil.take() {
            destroy_unused(gpu, [ResourceId::Texture(depth_stencil)]);
        }
    }
}

impl Drop for SurfaceTarget {
    fn drop(&mut self) {
        if self.remove_layer {
            // An explicit transaction: the thread that renders may have no
            // run loop to commit an implicit one.
            CATransaction::begin();
            self.layer.removeFromSuperlayer();
            CATransaction::commit();
        }
    }
}
