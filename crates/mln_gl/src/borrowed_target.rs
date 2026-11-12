// A ring of the host's textures, in its share group, that frames are drawn
// into and given back to the host with what the host waits for before it
// reads them; and the host's own surface, which frames are presented to.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use glow::HasContext;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::size::Size;

use crate::context::Context;
use crate::device::SurfaceSwap;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::renderer_backend::RendererBackend;
use crate::types::{FramebufferID, RenderbufferID};

/// A ring of the host's `GL_TEXTURE_2D` textures, in the share group of the
/// device that renders into them, all of `size` pixels with RGBA8 storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowedTextures {
    pub textures: Vec<u32>,
    pub size: Size,
    /// The host's `GL_TEXTURE_2D` textures that frames draw their depth and
    /// stencil into, one for each of `textures`, of their size and a depth
    /// and stencil format, or none for depth of the target's own.
    pub depth_textures: Vec<u32>,
}

impl BorrowedTextures {
    /// The kinds of sync with which the host can wait for a frame (the
    /// producer's) and the target for the host's reads (the consumer's): a
    /// fence in the share group, or a finished frame.
    #[must_use]
    pub fn sync_kinds() -> (SyncKinds, SyncKinds) {
        let kinds = SyncKinds::of(&[SyncKind::CpuComplete, SyncKind::OpenGlFence]);
        (kinds, kinds)
    }

    /// The formats of the depth textures that frames draw into, which
    /// OpenGL ES 3.0 renders into.
    #[must_use]
    pub fn depth_stencil_formats() -> DepthStencilFormats {
        DepthStencilFormats::of(&DepthStencilFormat::BY_PREFERENCE)
    }

    /// Checks the textures against their ring.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// depth textures that aren't one for each texture.
    pub fn check(&self) -> Result<(), Error> {
        if !self.depth_textures.is_empty() && self.depth_textures.len() != self.textures.len() {
            return Err(Error::invalid_argument("a ring's depth textures must be one for each of its textures"));
        }
        Ok(())
    }
}

/// A texture name of the host's.
fn host_texture(name: u32) -> Result<glow::NativeTexture, Error> {
    std::num::NonZeroU32::new(name)
        .map(glow::NativeTexture)
        .ok_or_else(|| Error::invalid_argument("a texture name is zero"))
}

/// The framebuffers over the ring's textures, and their depth and stencil
/// attachments: the host's textures, or a renderbuffer of the target's own.
struct Resource {
    context: Weak<Context>,
    size: Size,
    framebuffers: Vec<FramebufferID>,
    depth_stencil: Option<RenderbufferID>,
    selected: Rc<Cell<usize>>,
}

impl Resource {
    fn new(context: &Context, textures: &BorrowedTextures, selected: Rc<Cell<usize>>) -> Result<Self, Error> {
        let gl = context.gl();
        let size = textures.size;
        // SAFETY: the renderbuffer and the framebuffers are new, and the
        // textures are the host's, in the context's share group.
        unsafe {
            let depth_stencil = if textures.depth_textures.is_empty() {
                let depth_stencil = gl.create_renderbuffer().map_err(Error::native)?;
                gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth_stencil));
                gl.renderbuffer_storage(
                    glow::RENDERBUFFER,
                    glow::DEPTH24_STENCIL8,
                    size.width as i32,
                    size.height as i32,
                );
                gl.bind_renderbuffer(glow::RENDERBUFFER, None);
                Some(depth_stencil)
            } else {
                None
            };
            let mut resource =
                Resource { context: context.weak(), size, framebuffers: Vec::new(), depth_stencil, selected };
            for (slot, &texture) in textures.textures.iter().enumerate() {
                let texture = host_texture(texture)?;
                let framebuffer = gl.create_framebuffer().map_err(Error::native)?;
                resource.framebuffers.push(framebuffer);
                context.state().bind_framebuffer(gl, Some(framebuffer));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                match (depth_stencil, textures.depth_textures.get(slot)) {
                    (Some(depth_stencil), _) => gl.framebuffer_renderbuffer(
                        glow::FRAMEBUFFER,
                        glow::DEPTH_STENCIL_ATTACHMENT,
                        glow::RENDERBUFFER,
                        Some(depth_stencil),
                    ),
                    (None, Some(&depth)) => gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::DEPTH_STENCIL_ATTACHMENT,
                        glow::TEXTURE_2D,
                        Some(host_texture(depth)?),
                        0,
                    ),
                    (None, None) => {}
                }
                let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                if status != glow::FRAMEBUFFER_COMPLETE {
                    return Err(Error::invalid_argument(format!(
                        "a texture of the ring makes no complete framebuffer: status {status:#x}"
                    )));
                }
            }
            context.state().bind_framebuffer(gl, None);
            mln_gfx::context::Context::rendering_stats(context).num_frame_buffers += 1;
            Ok(resource)
        }
    }
}

impl Drop for Resource {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            for framebuffer in self.framebuffers.drain(..) {
                context.abandon_framebuffer(framebuffer);
            }
            if let Some(depth_stencil) = self.depth_stencil {
                context.abandon_renderbuffer(depth_stencil);
            }
            mln_gfx::context::Context::rendering_stats(&*context).num_frame_buffers -= 1;
        }
    }
}

impl TargetResource for Resource {
    fn bind(&self) {
        if let Some(context) = self.context.upgrade()
            && let Some(framebuffer) = self.framebuffers.get(self.selected.get())
        {
            context.bind_framebuffer(Some(*framebuffer), self.size, true);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Frames drawn into the host's textures, each given back to the host.
pub struct BorrowedTarget {
    backend: Rc<RendererBackend>,
    textures: RefCell<BorrowedTextures>,
    /// Made with the first frame, with the context current.
    image: RefCell<Option<Rc<DrawTarget>>>,
    selected: Rc<Cell<usize>>,
    producer: SyncKind,
    /// The fence of each slot's last frame, which the host may wait on
    /// until it gives the slot back.
    fences: RefCell<Vec<Option<glow::NativeFence>>>,
}

impl BorrowedTarget {
    /// A target on `backend`, of a device in the host's share group, that
    /// draws into `textures` and hands each frame to the host with sync of
    /// the kind `producer`, one of [`BorrowedTextures::sync_kinds`]'s.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// producer kind the target can't hand over.
    pub fn new(backend: Rc<RendererBackend>, textures: BorrowedTextures, producer: SyncKind) -> Result<Self, Error> {
        if !BorrowedTextures::sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("an OpenGL target hands frames over with no {producer:?}")));
        }
        textures.check()?;
        let depth = textures.textures.len();
        Ok(BorrowedTarget {
            backend,
            textures: RefCell::new(textures),
            image: RefCell::new(None),
            selected: Rc::new(Cell::new(0)),
            producer,
            fences: RefCell::new(vec![None; depth]),
        })
    }

    /// How many textures the ring has.
    pub fn depth(&self) -> usize {
        self.textures.borrow().textures.len()
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&self, slot: usize) {
        self.selected.set(slot);
    }

    fn delete_fence(&self, slot: usize) {
        let fence = self.fences.borrow_mut().get_mut(slot).and_then(Option::take);
        if let Some(fence) = fence {
            // SAFETY: the fence is the target's, and the context is current
            // (the caller's backend scope).
            unsafe { self.backend.gl().delete_sync(fence) };
        }
    }

    /// The host gave the texture at `slot` back: the context's commands from
    /// here on wait for `consumer`, a sync of one of
    /// [`BorrowedTextures::sync_kinds`]'s consumer kinds, and the target
    /// deletes the fence it names. Call it with the backend's context
    /// current.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the release takes the host's sync, as every backend's does, and drops it"
    )]
    pub fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        self.delete_fence(slot);
        let sync = consumer.sync;
        match sync.kind {
            SyncKind::CpuComplete => Ok(()),
            SyncKind::OpenGlFence => {
                if sync.handle == 0 {
                    return Err(Error::invalid_argument("an OpenGL fence release names no fence"));
                }
                let fence = glow::NativeFence(sync.handle as usize as *mut _);
                let gl = self.backend.gl();
                // SAFETY: the host's fence, in the context's share group,
                // which the release hands over.
                unsafe {
                    gl.wait_sync(fence, 0, glow::TIMEOUT_IGNORED);
                    gl.delete_sync(fence);
                }
                Ok(())
            }
            kind => Err(Error::unsupported(format!("the OpenGL target waits for no {kind:?}"))),
        }
    }

    /// The host gave up the texture at `slot`, which is never drawn into
    /// again. Call it with the backend's context current.
    pub fn discard(&self, slot: usize) {
        self.delete_fence(slot);
    }

    /// Draws into `textures` from now on, which must be as many, with depth
    /// textures when the ring has them. Call it with the backend's context
    /// current.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// another number of textures, and with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for depth textures
    /// where the ring had none, or none where it had them.
    pub fn replace(&self, textures: BorrowedTextures) -> Result<(), Error> {
        if textures.textures.len() != self.depth() {
            return Err(Error::invalid_argument("a ring keeps its depth"));
        }
        textures.check()?;
        if textures.depth_textures.is_empty() != self.textures.borrow().depth_textures.is_empty() {
            return Err(Error::unsupported("a ring keeps its depth textures, or its lack of them"));
        }
        for slot in 0..self.depth() {
            self.delete_fence(slot);
        }
        *self.image.borrow_mut() = None;
        *self.textures.borrow_mut() = textures;
        Ok(())
    }
}

impl FrameTarget for BorrowedTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        let mut image = self.image.borrow_mut();
        if image.is_none() {
            let context = self.backend.gl_context()?;
            let textures = self.textures.borrow();
            match Resource::new(&context, &textures, self.selected.clone()) {
                Ok(resource) => *image = Some(Rc::new(DrawTarget::new(textures.size, resource))),
                Err(error) => {
                    mln_util::logging::Log::error(mln_util::logging::Event::GraphicsBackend, error.message);
                    return None;
                }
            }
        }
        image.clone().map(|image| image as Rc<dyn frame::DrawTarget>)
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        if self.image.borrow().is_none() {
            return Handoff::Dropped;
        }
        let slot = self.selected.get();
        self.delete_fence(slot);
        let gl = self.backend.gl();
        // Mesa marks a depth texture that a framebuffer rendered into as
        // compressed, for another context that samples it to decompress it
        // first, only once the context applies a change of framebuffer,
        // which it does at its next draw or clear. Without that, radeonsi
        // hands the host's context the compressed data of its depth texture.
        if !self.textures.borrow().depth_textures.is_empty()
            && let Some(context) = self.backend.gl_context()
        {
            context.state().bind_framebuffer(gl, None);
            // SAFETY: the context is current (the renderer's backend scope),
            // on its own surface, whose color nothing reads.
            unsafe { gl.clear(glow::COLOR_BUFFER_BIT) };
        }
        let sync = if self.producer == SyncKind::OpenGlFence {
            // SAFETY: the context is current (the renderer's backend
            // scope).
            let fence = unsafe { gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) };
            match fence {
                Ok(fence) => {
                    // SAFETY: as above; a flush makes the fence reach the
                    // GPU, so that another context's wait ends.
                    unsafe { gl.flush() };
                    if let Some(entry) = self.fences.borrow_mut().get_mut(slot) {
                        *entry = Some(fence);
                    }
                    GpuSync { kind: SyncKind::OpenGlFence, handle: fence.0 as usize as u64, value: 0 }
                }
                Err(_) => return Handoff::Dropped,
            }
        } else {
            // SAFETY: as above.
            unsafe { gl.finish() };
            GpuSync::of(SyncKind::CpuComplete)
        };
        Handoff::GivenBack { slot, sync }
    }
}

impl Drop for BorrowedTarget {
    fn drop(&mut self) {
        let _scope = mln_gfx::BackendScope::new(&*self.backend, mln_gfx::ScopeType::Explicit);
        for slot in 0..self.depth() {
            self.delete_fence(slot);
        }
        self.image.borrow_mut().take();
    }
}

/// The default framebuffer of the host's surface that the context is current
/// on.
struct SurfaceResource {
    context: Weak<Context>,
    size: Rc<Cell<Size>>,
}

impl TargetResource for SurfaceResource {
    fn bind(&self) {
        if let Some(context) = self.context.upgrade() {
            context.bind_framebuffer(None, self.size.get(), true);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Frames presented to the host's EGL surface, through a context in the
/// host's share group current on it.
pub struct HostSurfaceTarget {
    backend: Rc<RendererBackend>,
    swap: SurfaceSwap,
    size: Rc<Cell<Size>>,
    image: RefCell<Option<Rc<DrawTarget>>>,
}

impl HostSurfaceTarget {
    /// A backend of a context in `device`'s share group, the host's, current
    /// on the host's `surface`, and a target that presents to it at `size`
    /// pixels.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) on a
    /// device of the library's, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when EGL makes no context.
    pub fn with_backend(
        device: &crate::device::Device,
        surface: HostSurface,
        size: Size,
    ) -> Result<(Rc<RendererBackend>, HostSurfaceTarget), Error> {
        let (gl, platform, swap) = device.create_surface_context(surface.0)?;
        let backend = Rc::new(RendererBackend::from_parts(gl, platform));
        let target = HostSurfaceTarget {
            backend: backend.clone(),
            swap,
            size: Rc::new(Cell::new(size)),
            image: RefCell::new(None),
        };
        Ok((backend, target))
    }

    pub fn size(&self) -> Size {
        self.size.get()
    }

    /// Draws the next frame at `size` pixels, which the host's surface has.
    pub fn resize(&self, size: Size) {
        self.size.set(size);
    }
}

impl FrameTarget for HostSurfaceTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        let size = self.size.get();
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let mut image = self.image.borrow_mut();
        if image.is_none() {
            let context = self.backend.gl_context()?;
            let resource = SurfaceResource { context: Rc::downgrade(&context), size: self.size.clone() };
            *image = Some(Rc::new(DrawTarget::new(size, resource)));
        }
        let image = image.clone()?;
        image.set_size(size);
        Some(image)
    }

    fn end_frame(&self, _encoder: &dyn CommandEncoder) -> Handoff {
        if self.image.borrow().is_none() {
            return Handoff::Dropped;
        }
        self.swap.swap();
        Handoff::Presented
    }
}

/// The host's `EGLSurface`, on the display of a device that wraps the host's
/// EGL context, with that context's config, or on Windows the host's `HDC`,
/// of the pixel format of the WGL context that the device wraps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostSurface(pub(crate) SurfaceHandle);

/// What names a host's surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceHandle {
    Egl(khronos_egl::Surface),
    #[cfg(windows)]
    Wgl(windows_sys::Win32::Graphics::Gdi::HDC),
}

// SAFETY: an EGL handle or a device context, which the target's context uses
// on one thread.
unsafe impl Send for HostSurface {}
// SAFETY: as above.
unsafe impl Sync for HostSurface {}

impl HostSurface {
    /// # Safety
    ///
    /// `surface` is a live `EGLSurface` of the display of the device whose
    /// targets present to it, which outlives them.
    #[must_use]
    pub unsafe fn from_raw(surface: std::ptr::NonNull<std::ffi::c_void>) -> HostSurface {
        // SAFETY: the caller's contract.
        HostSurface(SurfaceHandle::Egl(unsafe { khronos_egl::Surface::from_ptr(surface.as_ptr()) }))
    }

    /// # Safety
    ///
    /// `dc` is a live `HDC` with the pixel format of the WGL context that
    /// the device whose targets present to it wraps, and it outlives them.
    #[cfg(windows)]
    #[must_use]
    pub unsafe fn from_hdc(dc: std::ptr::NonNull<std::ffi::c_void>) -> HostSurface {
        HostSurface(SurfaceHandle::Wgl(dc.as_ptr()))
    }
}
