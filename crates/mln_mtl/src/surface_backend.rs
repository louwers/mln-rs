// Ports platform/glfw/metal_backend.h and platform/glfw/metal_backend.mm:
// a `mtl::RendererBackend` that is its own default `gfx::Renderable` and
// presents to a `CAMetalLayer`. The window stays with the host (such as
// examples/desktop), which makes the layer and reports framebuffer resizes
// through `set_framebuffer_size`.
//
// Differences from C++:
// - C++ takes the next drawable and a new command buffer in every `bind()`,
//   which the upload pass and the render pass of a frame both call; the
//   port keeps them until `swap()`, as the headless backend does.
// - The frame is encoded on the backend's command queue; C++ makes a second
//   queue for the window.
// - The depth and stencil attachments are Metal textures of their own, not
//   `mtl::Texture2D`s, which need the context that `bind` has no access to.
// - Port-only: `acquire_next_texture` takes the drawable ahead of a frame,
//   `request_readback` copies a presented frame into an image, with
//   `cap_frame_rate` off the layer doesn't wait for the display, and
//   `set_overlay` draws over each frame before it is presented, such as an
//   app's controls over the map.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mln_gfx::context::Context as GfxContext;
use mln_gfx::renderable::{Renderable, RenderableBase};
use mln_gfx::renderer_backend::{self, ContextMode, RendererBackendBase};
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDevice, MTLOrigin,
    MTLPixelFormat, MTLRenderPassDescriptor, MTLResourceOptions, MTLSize, MTLStorageMode, MTLTexture,
    MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

use crate::mtl_fwd::{MTLCommandBufferPtr, MTLRenderPassDescriptorPtr, MTLTexturePtr};
use crate::renderable_resource::{RenderableResource, RenderableResourceBox};
use crate::renderer_backend::{BackendImpl, RendererBackend};

/// The format of the layer's drawables.
const COLOR_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;

/// Port-only: what an overlay draws into (`SurfaceBackend::set_overlay`):
/// the frame's drawable, after the map and before it is presented.
pub struct OverlayTarget<'a> {
    pub device: &'a ProtocolObject<dyn MTLDevice>,
    /// The frame's command buffer, which the overlay encodes its passes into.
    pub command_buffer: &'a ProtocolObject<dyn MTLCommandBuffer>,
    pub texture: &'a ProtocolObject<dyn MTLTexture>,
    /// In physical pixels.
    pub size: Size,
}

/// Port-only: draws over a frame (`SurfaceBackend::set_overlay`).
pub type Overlay = Box<dyn FnMut(&OverlayTarget<'_>)>;

/// The depth and stencil textures of a drawable size.
struct DepthStencil {
    size: (usize, usize),
    depth: MTLTexturePtr,
    stencil: MTLTexturePtr,
}

/// What the renderable resource and the backend share.
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
        let blit = command_buffer.blitCommandEncoder()?;
        // SAFETY: the region is the whole texture and the buffer holds it.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                texture,
                0,
                0,
                MTLOrigin { x: 0, y: 0, z: 0 },
                MTLSize { width, height, depth: 1 },
                &buffer,
                0,
                bytes_per_row,
                bytes_per_row * height,
            );
        }
        blit.endEncoding();
        Some((buffer, Size::new(width as u32, height as u32)))
    }

    fn swap(&self) {
        let drawable = self.drawable.borrow_mut().take();
        let command_buffer = self.command_buffer.borrow_mut().take();
        self.render_pass_descriptor.borrow_mut().take();
        let (Some(drawable), Some(command_buffer)) = (drawable, command_buffer) else {
            return;
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
            }
        }

        command_buffer.presentDrawable(ProtocolObject::from_ref(&*drawable));
        command_buffer.commit();

        if let Some((buffer, size)) = readback {
            command_buffer.waitUntilCompleted();
            let mut image = PremultipliedImage::new(size);
            // SAFETY: the copy is complete and the buffer holds the image.
            let bgra = unsafe { std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), image.data.len()) };
            for (dst, src) in image.data.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
                dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
            }
            *self.readback.borrow_mut() = Some(image);
        }
    }
}

/// `MetalRenderableResource`: the layer as a renderable resource.
struct SurfaceRenderableResource {
    state: Rc<SurfaceState>,
}

impl RenderableResource for SurfaceRenderableResource {
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

    fn swap(&self) {
        self.state.swap();
    }

    fn get_command_buffer(&self) -> Option<MTLCommandBufferPtr> {
        self.state.command_buffer.borrow().clone()
    }

    fn get_render_pass_descriptor(&self) -> Option<MTLRenderPassDescriptorPtr> {
        self.state.render_pass_descriptor.borrow().clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `MetalBackend` without the window: a `mtl::RendererBackend` and
/// `gfx::Renderable` presenting to a `CAMetalLayer`.
pub struct SurfaceBackend {
    renderer_backend: RendererBackend,
    renderable: RenderableBase,
    state: Rc<SurfaceState>,
}

impl SurfaceBackend {
    /// A backend on the system's default device that presents to `layer`,
    /// whose drawables it sizes to `framebuffer_size`. With
    /// `cap_frame_rate` off, frames are presented without waiting for the
    /// display.
    pub fn new(layer: Retained<CAMetalLayer>, framebuffer_size: Size, cap_frame_rate: bool) -> Result<Self, String> {
        let renderer_backend = RendererBackend::new(ContextMode::Unique)?;
        layer.setDevice(Some(renderer_backend.get_device()));
        layer.setPixelFormat(COLOR_FORMAT);
        layer.setFramebufferOnly(true);
        #[cfg(target_os = "macos")]
        layer.setDisplaySyncEnabled(cap_frame_rate);
        #[cfg(not(target_os = "macos"))]
        let _ = cap_frame_rate;
        let state = Rc::new(SurfaceState {
            backend: renderer_backend.backend_impl().clone(),
            layer,
            drawable: RefCell::new(None),
            command_buffer: RefCell::new(None),
            render_pass_descriptor: RefCell::new(None),
            depth_stencil: RefCell::new(None),
            readback_requested: Cell::new(false),
            readback: RefCell::new(None),
            overlay: RefCell::new(None),
        });
        let resource = RenderableResourceBox::new_rc(SurfaceRenderableResource { state: state.clone() });
        let backend = SurfaceBackend {
            renderer_backend,
            renderable: RenderableBase::new(Size::new(0, 0), Some(resource)),
            state,
        };
        backend.set_framebuffer_size(framebuffer_size);
        Ok(backend)
    }

    pub fn mtl_backend(&self) -> &RendererBackend {
        &self.renderer_backend
    }

    /// The name of the GPU.
    pub fn get_device_name(&self) -> String {
        self.renderer_backend.get_device().name().to_string()
    }

    /// The format of the layer's drawables.
    pub fn get_pixel_format(&self) -> MTLPixelFormat {
        COLOR_FORMAT
    }

    /// `getFramebufferSize()`
    pub fn get_framebuffer_size(&self) -> Size {
        self.get_size()
    }

    /// `setSize(Size)`: the size of the drawables, in pixels.
    pub fn set_framebuffer_size(&self, size: Size) {
        self.set_renderable_size(size);
        if size.width > 0 && size.height > 0 {
            self.state.layer.setDrawableSize(CGSize::new(f64::from(size.width), f64::from(size.height)));
        }
    }

    /// Port-only: takes the layer's next drawable ahead of a frame; false
    /// when the layer has none right now. The caller skips the frame: a
    /// render pass without a color attachment trips the drawables' debug
    /// assertions.
    pub fn acquire_next_texture(&self) -> bool {
        autoreleasepool(|_| self.state.acquire_drawable())
    }

    /// Port-only: copy the next presented frame (see `take_readback`). The
    /// layer's drawables can be read from the next one it hands out; a frame
    /// on one taken before is copied from the frame after.
    pub fn request_readback(&self) {
        self.state.layer.setFramebufferOnly(false);
        self.state.readback_requested.set(true);
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

impl Drop for SurfaceBackend {
    fn drop(&mut self) {
        self.renderer_backend.reset_context();
        self.state.drawable.borrow_mut().take();
        self.state.command_buffer.borrow_mut().take();
        self.state.overlay.borrow_mut().take();
        self.set_resource(None);
    }
}

impl Renderable for SurfaceBackend {
    fn renderable_base(&self) -> &RenderableBase {
        &self.renderable
    }
}

impl renderer_backend::RendererBackend for SurfaceBackend {
    fn renderer_backend_base(&self) -> &RendererBackendBase {
        &self.renderer_backend.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_default_renderable(&self) -> &dyn Renderable {
        self
    }

    fn init_shaders(&self, registry: &ShaderRegistry, parameters: &ProgramParameters) {
        self.renderer_backend.init_shaders(registry, parameters);
    }

    fn create_context(&self) -> Rc<dyn GfxContext> {
        self.renderer_backend.create_context()
    }

    fn activate(&self) {
        self.renderer_backend.activate();
    }

    fn deactivate(&self) {
        self.renderer_backend.deactivate();
    }
}
