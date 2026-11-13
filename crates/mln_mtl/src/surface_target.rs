// Frames presented to a `CAMetalLayer`. The window stays with the host (such
// as examples/desktop), which makes the layer and reports framebuffer resizes
// through `resize`.
//
// Differences from C++:
// - C++ takes the next drawable and a new command buffer in every `bind()`,
//   which the upload pass and the render pass of a frame both call; the
//   port keeps them until `swap()`, as the headless backend does.
// - The frame is encoded on the backend's command queue; C++ makes a second
//   queue for the window.
// - The depth and stencil attachments are Metal textures of their own, not
//   `mtl::Texture2D`s, which need the context that `bind` has no access to.
// - Port-only: `begin_frame` takes the drawable ahead of a frame,
//   `request_readback` copies a presented frame into an image, with
//   `cap_frame_rate` off the layer doesn't wait for the display, and
//   `set_overlay` draws over each frame before it is presented, such as an
//   app's controls over the map.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandQueue, MTLDevice, MTLPixelFormat, MTLRenderPassDescriptor,
    MTLResourceOptions, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

use crate::draw_target::{DrawTarget, TargetResource};
use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderPassDescriptorPtr, MTLTexturePtr};
use crate::renderer_backend::{BackendImpl, RendererBackend};
use crate::texture2d::{buffer_bytes, encode_texture_copy};

/// The format of the layer's drawables.
const COLOR_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;

/// Port-only: what an overlay draws into (`SurfaceTarget::set_overlay`):
/// the frame's drawable, after the map and before it is presented.
pub struct OverlayTarget<'a> {
    pub device: &'a ProtocolObject<dyn MTLDevice>,
    /// The frame's command buffer, which the overlay encodes its passes into.
    pub command_buffer: &'a ProtocolObject<dyn MTLCommandBuffer>,
    pub texture: &'a ProtocolObject<dyn MTLTexture>,
    /// In physical pixels.
    pub size: Size,
}

/// Port-only: draws over a frame (`SurfaceTarget::set_overlay`).
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

/// The depth and stencil textures of a drawable size.
struct DepthStencil {
    size: (usize, usize),
    depth: MTLTexturePtr,
    stencil: MTLTexturePtr,
}

/// What the draw target and the frame target share.
struct SurfaceState {
    backend: Rc<BackendImpl>,
    layer: Retained<CAMetalLayer>,
    drawable: RefCell<Option<Retained<ProtocolObject<dyn CAMetalDrawable>>>>,
    command_buffer: RefCell<Option<MTLCommandBufferPtr>>,
    render_pass_descriptor: RefCell<Option<MTLRenderPassDescriptorPtr>>,
    depth_stencil: RefCell<Option<DepthStencil>>,
    readback_requested: Cell<bool>,
    readback: RefCell<Option<PremultipliedImage>>,
    overlay: RefCell<Option<Overlay>>,
}

impl SurfaceState {
    /// Takes the layer's next drawable unless the frame has one; false if
    /// the layer has none to give (it waits up to a second).
    fn acquire_drawable(&self) -> bool {
        let mut drawable = self.drawable.borrow_mut();
        if drawable.is_none() {
            *drawable = self.layer.nextDrawable();
        }
        drawable.is_some()
    }

    fn texture_2d(&self, format: MTLPixelFormat, width: usize, height: usize) -> Option<MTLTexturePtr> {
        // SAFETY: the size is non-zero and the format a valid 2D one.
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                format, width, height, false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::RenderTarget);
        descriptor.setStorageMode(MTLStorageMode::Private);
        self.backend.get_device().newTextureWithDescriptor(&descriptor)
    }

    /// The depth and stencil textures for a drawable of `size`. Intel and
    /// AMD GPUs take one texture for both.
    fn depth_stencil_for(&self, size: (usize, usize)) -> Option<(MTLTexturePtr, MTLTexturePtr)> {
        let mut depth_stencil = self.depth_stencil.borrow_mut();
        if depth_stencil.as_ref().is_none_or(|d| d.size != size) {
            *depth_stencil = if cfg!(target_arch = "x86_64") {
                self.texture_2d(MTLPixelFormat::Depth32Float_Stencil8, size.0, size.1).map(|texture| DepthStencil {
                    size,
                    depth: texture.clone(),
                    stencil: texture,
                })
            } else {
                match (
                    self.texture_2d(MTLPixelFormat::Depth32Float, size.0, size.1),
                    self.texture_2d(MTLPixelFormat::Stencil8, size.0, size.1),
                ) {
                    (Some(depth), Some(stencil)) => Some(DepthStencil { size, depth, stencil }),
                    _ => None,
                }
            };
        }
        depth_stencil.as_ref().map(|d| (d.depth.clone(), d.stencil.clone()))
    }

    /// Encodes a copy of `texture` into a shared buffer.
    fn encode_readback(
        &self,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        texture: &ProtocolObject<dyn MTLTexture>,
    ) -> Option<(Retained<ProtocolObject<dyn MTLBuffer>>, Size)> {
        let (width, height) = (texture.width(), texture.height());
        let bytes_per_row = width * 4;
        let buffer = self
            .backend
            .get_device()
            .newBufferWithLength_options(bytes_per_row * height, MTLResourceOptions::StorageModeShared)?;
        encode_texture_copy(command_buffer, texture, bytes_per_row, &buffer)
            .then(|| (buffer, Size::new(width as u32, height as u32)))
    }

    /// Presents the frame's drawable. Returns whether the frame had one.
    fn swap(&self) -> bool {
        let drawable = self.drawable.borrow_mut().take();
        let command_buffer = self.command_buffer.borrow_mut().take();
        self.render_pass_descriptor.borrow_mut().take();
        let (Some(drawable), Some(command_buffer)) = (drawable, command_buffer) else {
            return false;
        };

        let texture = drawable.texture();
        if let Some(overlay) = self.overlay.borrow_mut().as_mut() {
            overlay(&OverlayTarget {
                device: self.backend.get_device(),
                command_buffer: &command_buffer,
                texture: &texture,
                size: Size::new(texture.width() as u32, texture.height() as u32),
            });
        }

        let mut readback = None;
        if self.readback_requested.get() {
            if texture.isFramebufferOnly() {
                // Taken before the request: copy the next frame.
            } else {
                self.readback_requested.set(false);
                readback = self.encode_readback(&command_buffer, &texture);
                self.layer.setFramebufferOnly(true);
            }
        }

        command_buffer.presentDrawable(ProtocolObject::from_ref(&*drawable));
        command_buffer.commit();

        if let Some((buffer, size)) = readback {
            command_buffer.waitUntilCompleted();
            let mut image = PremultipliedImage::new(size);
            let bgra = buffer_bytes(&buffer, image.data.len());
            for (dst, src) in image.data.as_chunks_mut::<4>().0.iter_mut().zip(bgra.as_chunks::<4>().0) {
                *dst = [src[2], src[1], src[0], src[3]];
            }
            *self.readback.borrow_mut() = Some(image);
        }
        true
    }
}

/// The layer's drawable as a draw target.
struct SurfaceResource {
    state: Rc<SurfaceState>,
}

impl TargetResource for SurfaceResource {
    fn bind(&self) {
        let state = &self.state;
        if !state.acquire_drawable() {
            return;
        }
        let mut command_buffer = state.command_buffer.borrow_mut();
        if command_buffer.is_none() {
            *command_buffer = state.backend.get_command_queue().commandBuffer();
        }
        let Some(texture) = state.drawable.borrow().as_ref().map(|d| d.texture()) else {
            return;
        };
        let descriptor = MTLRenderPassDescriptor::new();
        let size = (texture.width(), texture.height());
        // SAFETY: index 0 is in the attachment array's bounds.
        unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }.setTexture(Some(&texture));
        if let Some((depth, stencil)) = state.depth_stencil_for(size) {
            descriptor.depthAttachment().setTexture(Some(&depth));
            descriptor.stencilAttachment().setTexture(Some(&stencil));
        }
        *state.render_pass_descriptor.borrow_mut() = Some(descriptor);
    }

    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr> {
        self.state.command_buffer.borrow().clone()
    }

    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr> {
        self.state.render_pass_descriptor.borrow().clone()
    }
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

    /// The layer `layer` points to, retained, or `None` for a null pointer.
    ///
    /// # Safety
    ///
    /// `layer` is null or a live `CAMetalLayer`.
    pub unsafe fn from_raw(layer: *mut std::ffi::c_void) -> Option<Layer> {
        // SAFETY: the caller's contract.
        unsafe { Retained::retain(layer.cast()) }.map(Layer)
    }
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Layer").finish_non_exhaustive()
    }
}

/// `MetalBackend`'s layer without the window: frames presented to a
/// `CAMetalLayer`.
pub struct SurfaceTarget {
    state: Rc<SurfaceState>,
    image: Rc<DrawTarget>,
    size: Cell<Size>,
}

impl SurfaceTarget {
    /// A target on `backend` that presents to `layer`, whose drawables it
    /// sizes to `framebuffer_size`. With `cap_frame_rate` off, frames are
    /// presented without waiting for the display.
    pub fn new(backend: &RendererBackend, layer: Layer, framebuffer_size: Size, cap_frame_rate: bool) -> SurfaceTarget {
        let Layer(layer) = layer;
        layer.setDevice(Some(backend.get_device()));
        layer.setPixelFormat(COLOR_FORMAT);
        layer.setFramebufferOnly(true);
        #[cfg(target_os = "macos")]
        layer.setDisplaySyncEnabled(cap_frame_rate);
        #[cfg(not(target_os = "macos"))]
        let _ = cap_frame_rate;
        let state = Rc::new(SurfaceState {
            backend: Rc::clone(backend.backend_impl()),
            layer,
            drawable: RefCell::new(None),
            command_buffer: RefCell::new(None),
            render_pass_descriptor: RefCell::new(None),
            depth_stencil: RefCell::new(None),
            readback_requested: Cell::new(false),
            readback: RefCell::new(None),
            overlay: RefCell::new(None),
        });
        let image = Rc::new(DrawTarget::new(Size::new(0, 0), Rc::new(SurfaceResource { state: Rc::clone(&state) })));
        let target = SurfaceTarget { state, image, size: Cell::new(Size::new(0, 0)) };
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
        self.image.set_size(size);
        if size.width > 0 && size.height > 0 {
            self.state.layer.setDrawableSize(CGSize::new(f64::from(size.width), f64::from(size.height)));
        }
    }

    /// Port-only: copy the next presented frame (see `take_readback`). The
    /// layer's drawables can be read until that copy, from the next one it
    /// hands out: a drawable taken ahead of the next frame is given back,
    /// and one that a frame is drawing into now has its copy in the frame
    /// after.
    pub fn request_readback(&self) {
        let state = &self.state;
        state.layer.setFramebufferOnly(false);
        state.readback_requested.set(true);
        if state.command_buffer.borrow().is_none() {
            state.drawable.borrow_mut().take_if(|drawable| drawable.texture().isFramebufferOnly());
        }
    }

    /// Port-only: draws `overlay` over each frame from now on, after the map
    /// and before the frame is presented (and read back), or stops with
    /// `None`.
    pub fn set_overlay(&self, overlay: Option<Overlay>) {
        *self.state.overlay.borrow_mut() = overlay;
    }

    /// Port-only: the frame copied after `request_readback`, once presented.
    pub fn take_readback(&self) -> Option<PremultipliedImage> {
        self.state.readback.borrow_mut().take()
    }
}

impl FrameTarget for SurfaceTarget {
    /// Takes the layer's next drawable unless the frame has one, or `None`
    /// when the layer has none right now. A frame without one is skipped: a
    /// render pass without a color attachment trips the drawables' debug
    /// assertions.
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        autoreleasepool(|_| self.state.acquire_drawable())
            .then(|| -> Rc<dyn frame::DrawTarget> { Rc::<DrawTarget>::clone(&self.image) })
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        if self.state.swap() { Handoff::Presented } else { Handoff::Dropped }
    }
}

impl Drop for SurfaceTarget {
    fn drop(&mut self) {
        self.state.drawable.borrow_mut().take();
        self.state.command_buffer.borrow_mut().take();
        self.state.overlay.borrow_mut().take();
    }
}
