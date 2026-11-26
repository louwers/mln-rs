// A ring of the host's textures, in its share group, that frames are drawn
// into and given back to the host with what the host waits for before it
// reads them; and the host's own surface, which frames are presented to.

use glow::HasContext;
use mln_gfx::frame::{FrameImage, FrameTarget, Handoff};
use mln_gfx::gpu::{Gpu as _, ResourceId, TargetFormats, TextureDesc, TextureId, TextureUsage, destroy_unused};
use mln_gfx::{ConsumerSync, DepthStencilFormat, DepthStencilFormats, GpuSync, SyncKind, SyncKinds};
use mln_util::error::Error;
use mln_util::size::Size;

use crate::device::SurfaceSwap;
use crate::frame_image::{COLOR, DEPTH_STENCIL, FrameImports, default_framebuffer};
use crate::gpu::GlGpu;
use crate::log_target;
use crate::renderer_backend::RendererBackend;

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
    pub(crate) fn check(&self) -> Result<(), Error> {
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

/// Frames drawn into the host's textures, each given back to the host.
pub struct BorrowedTarget {
    /// `None` once the target let go of them.
    textures: Option<BorrowedTextures>,
    /// Each slot's textures, imported into the backend's `GlGpu` at the
    /// first frame drawn into the slot, until the ring's textures are
    /// replaced or the target goes, so that the `GlGpu` keeps the slot's
    /// framebuffer. Whether the depth and stencil texture is imported too,
    /// rather than the target's own.
    imports: Vec<Option<(FrameImage, bool)>>,
    /// The slot of the frame begun, until it ends.
    begun: Option<usize>,
    /// The target's own depth and stencil texture, of the backend's
    /// `GlGpu`, where the host gave none.
    depth_stencil: Option<TextureId>,
    selected: usize,
    producer: SyncKind,
    /// The fence of each slot's last frame, which the host may wait on
    /// until it gives the slot back.
    fences: Vec<Option<glow::NativeFence>>,
    /// The fence of the last release or replacement of the textures, until
    /// the next one or the target goes.
    handed: Option<glow::NativeFence>,
    /// How many textures the ring has.
    depth: usize,
}

impl BorrowedTarget {
    /// A target, on a backend of a device in the host's share group, that
    /// draws into `textures` and hands each frame to the host with sync of
    /// the kind `producer`, one of [`BorrowedTextures::sync_kinds`]'s.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for a
    /// producer kind the target can't hand over.
    pub fn new(textures: BorrowedTextures, producer: SyncKind) -> Result<Self, Error> {
        if !BorrowedTextures::sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("an OpenGL target hands frames over with no {producer:?}")));
        }
        textures.check()?;
        let depth = textures.textures.len();
        Ok(BorrowedTarget {
            textures: Some(textures),
            imports: vec![None; depth],
            begun: None,
            depth_stencil: None,
            selected: 0,
            producer,
            fences: vec![None; depth],
            handed: None,
            depth,
        })
    }

    /// How many textures the ring has.
    pub(crate) fn depth(&self) -> usize {
        self.depth
    }

    /// Has the next frame draw into the texture at `slot`.
    pub fn select(&mut self, slot: usize) {
        self.selected = slot;
    }

    fn delete_fence(&mut self, gpu: &mut GlGpu, slot: usize) {
        if let Some(fence) = self.fences.get_mut(slot).and_then(Option::take) {
            gpu.delete_sync(fence);
        }
    }

    /// Forgets the slots' imports, and destroys the target's own depth and
    /// stencil texture.
    fn forget_imports(&mut self, gpu: &mut GlGpu) {
        for (image, depth_imported) in self.imports.iter_mut().filter_map(Option::take) {
            let depth_stencil = image.depth_stencil.filter(|_| depth_imported);
            let textures = [Some(image.color), depth_stencil].into_iter().flatten();
            destroy_unused(gpu, textures.map(ResourceId::Texture));
        }
        if let Some(texture) = self.depth_stencil.take() {
            destroy_unused(gpu, [ResourceId::Texture(texture)]);
        }
    }

    /// The host gave the texture at `slot` back: the context's commands from
    /// here on wait for `consumer`, a sync of one of
    /// [`BorrowedTextures::sync_kinds`]'s consumer kinds, and the target
    /// deletes the fence it names.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the release takes the host's sync, as every backend's does, and drops it"
    )]
    pub fn release(&mut self, gpu: &mut GlGpu, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        self.delete_fence(gpu, slot);
        let sync = consumer.sync;
        match sync.kind {
            SyncKind::CpuComplete => Ok(()),
            SyncKind::OpenGlFence => {
                if sync.handle == 0 {
                    return Err(Error::invalid_argument("an OpenGL fence release names no fence"));
                }
                // The host's fence, in the context's share group, which the
                // release hands over.
                gpu.wait_sync(glow::NativeFence(sync.handle as usize as *mut _));
                Ok(())
            }
            kind => Err(Error::unsupported(format!("the OpenGL target waits for no {kind:?}"))),
        }
    }

    /// The host gave up the texture at `slot`, which is never drawn into
    /// again.
    pub fn discard(&mut self, gpu: &mut GlGpu, slot: usize) {
        self.delete_fence(gpu, slot);
    }

    /// Draws into `textures` from now on, which must be as many, with depth
    /// textures when the ring has them. Returns the sync after which the
    /// host may use the old textures, as
    /// [`release_textures`](Self::release_textures) does.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for
    /// another number of textures, with
    /// [`ErrorKind::Unsupported`](mln_util::ErrorKind) for depth textures
    /// where the ring had none, or none where it had them, with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) once the target let
    /// go of its textures, and as [`release_textures`](Self::release_textures)
    /// does. The target then keeps its textures.
    pub fn replace(&mut self, gpu: &mut GlGpu, textures: BorrowedTextures) -> Result<GpuSync, Error> {
        let Some(current) = &self.textures else {
            return Err(Error::invalid_state("the ring let go of its textures"));
        };
        if textures.textures.len() != self.depth() {
            return Err(Error::invalid_argument("a ring keeps its depth"));
        }
        textures.check()?;
        if textures.depth_textures.is_empty() != current.depth_textures.is_empty() {
            return Err(Error::unsupported("a ring keeps its depth textures, or its lack of them"));
        }
        let sync = self.end_work(gpu)?;
        self.textures = Some(textures);
        Ok(sync)
    }

    /// Lets go of the textures, and returns the sync after which the host
    /// may use them, of the producer's kind. The target keeps the fence
    /// that the sync names until its next release or replacement of
    /// textures, or until it goes, and draws no frame until then.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) once the
    /// target let go of its textures, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when it can't make the
    /// fence, or finish its work for [`SyncKind::CpuComplete`]. It keeps
    /// its textures then.
    pub fn release_textures(&mut self, gpu: &mut GlGpu) -> Result<GpuSync, Error> {
        if self.textures.is_none() {
            return Err(Error::invalid_state("the ring let go of its textures"));
        }
        let sync = self.end_work(gpu)?;
        self.textures = None;
        Ok(sync)
    }

    /// Forgets the slots' fences and imports, and a frame begun on the
    /// textures, and returns the sync that ends the context's commands so
    /// far.
    fn end_work(&mut self, gpu: &mut GlGpu) -> Result<GpuSync, Error> {
        for slot in 0..self.depth() {
            self.delete_fence(gpu, slot);
        }
        if self.begun.take().is_some() {
            self.leave_host_depth(gpu);
        }
        self.forget_imports(gpu);
        if let Some(fence) = self.handed.take() {
            gpu.delete_sync(fence);
        }
        if self.producer == SyncKind::OpenGlFence {
            let fence =
                gpu.fence_sync().map_err(|error| Error::native(format!("Making a ring's fence failed: {error}")))?;
            self.handed = Some(fence);
            return Ok(GpuSync { kind: SyncKind::OpenGlFence, handle: fence.0 as usize as u64, value: 0 });
        }
        gpu.flush(true).map_err(|error| Error::native(format!("Finishing a ring's frames failed: {error}")))?;
        Ok(GpuSync::of(SyncKind::CpuComplete))
    }
}

impl BorrowedTarget {
    /// Has the context leave the host's depth texture that a frame drew
    /// into, before a fence that hands it over. Mesa marks a depth texture
    /// that a framebuffer rendered into as compressed, for another context
    /// that samples it to decompress it first, only once the context
    /// applies a change of framebuffer, which it does at its next draw or
    /// clear. Without that, radeonsi hands the host's context the
    /// compressed data of its depth texture.
    fn leave_host_depth(&self, gpu: &mut GlGpu) {
        if self.textures.as_ref().is_some_and(|textures| !textures.depth_textures.is_empty()) {
            let mut current = gpu.enter();
            let (gl, state) = current.context_mut().parts();
            state.bind_framebuffer(gl, None);
            // SAFETY: the context is current, on its own surface, whose
            // color nothing reads.
            unsafe { gl.clear(glow::COLOR_BUFFER_BIT) };
        }
    }

    /// The textures of `slot`, imported into `gpu` at its first frame.
    fn slot_image(&mut self, gpu: &mut GlGpu, slot: usize) -> Result<FrameImage, Error> {
        if let Some((image, _)) = self.imports.get(slot).copied().flatten() {
            return Ok(image);
        }
        let imported = self.import(gpu, slot)?;
        if let Some(entry) = self.imports.get_mut(slot) {
            *entry = Some(imported);
        }
        Ok(imported.0)
    }

    /// The texture at `slot` and its depth and stencil texture, the host's
    /// imported into `gpu` or the target's own, and whether the depth is
    /// imported.
    fn import(&mut self, gpu: &mut GlGpu, slot: usize) -> Result<(FrameImage, bool), Error> {
        let textures = self.textures.as_ref().ok_or_else(|| Error::invalid_state("the ring let go of its textures"))?;
        let size = textures.size;
        let desc = |label, format| TextureDesc {
            label,
            size,
            format,
            usage: TextureUsage { render_target: true, ..TextureUsage::default() },
            mipmapped: false,
        };
        let color = host_texture(*textures.textures.get(slot).ok_or_else(|| Error::invalid_argument("no such slot"))?)?;
        let host_depth = textures.depth_textures.get(slot).map(|&depth| host_texture(depth)).transpose()?;
        // Of the ring's size, which its textures keep until they are
        // replaced.
        if host_depth.is_none() && self.depth_stencil.is_none() {
            let depth = gpu.create_texture(&desc("borrowed ring depth and stencil", DEPTH_STENCIL));
            self.depth_stencil = Some(depth.map_err(|error| Error::native(error.to_string()))?);
        }
        let color = gpu.import_texture(color, &desc("borrowed texture", COLOR));
        let (depth_stencil, imported) = match (host_depth, self.depth_stencil) {
            (Some(depth), _) => (gpu.import_texture(depth, &desc("borrowed depth", DEPTH_STENCIL)), true),
            (None, Some(depth)) => (depth, false),
            (None, None) => return Err(Error::native("the ring has no depth and stencil texture")),
        };
        let targets = TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) };
        Ok((FrameImage { size, color, depth_stencil: Some(depth_stencil), targets }, imported))
    }
}

impl FrameTarget for BorrowedTarget {
    type Gpu = GlGpu;

    fn begin_frame(&mut self, gpu: &mut GlGpu) -> Option<FrameImage> {
        let slot = self.selected;
        self.begun = None;
        let image = self
            .slot_image(gpu, slot)
            .inspect_err(|error| log::error!(target: log_target::GRAPHICS_BACKEND, "{}", error.message))
            .ok()?;
        self.begun = Some(slot);
        Some(image)
    }

    fn end_frame(&mut self, gpu: &mut GlGpu) -> Handoff {
        let Some(slot) = self.begun.take() else {
            return Handoff::Dropped;
        };
        self.delete_fence(gpu, slot);
        self.leave_host_depth(gpu);
        let sync = if self.producer == SyncKind::OpenGlFence {
            let Ok(fence) = gpu.fence_sync() else { return Handoff::Dropped };
            if let Some(entry) = self.fences.get_mut(slot) {
                *entry = Some(fence);
            }
            GpuSync { kind: SyncKind::OpenGlFence, handle: fence.0 as usize as u64, value: 0 }
        } else {
            if let Err(error) = gpu.flush(true) {
                log::error!(target: log_target::GRAPHICS_BACKEND, "Finishing a borrowed frame failed: {error}");
            }
            GpuSync::of(SyncKind::CpuComplete)
        };
        Handoff::GivenBack { slot, sync }
    }

    fn tear_down(&mut self, gpu: &mut GlGpu) {
        for slot in 0..self.depth() {
            self.delete_fence(gpu, slot);
        }
        if let Some(fence) = self.handed.take() {
            gpu.delete_sync(fence);
        }
        self.forget_imports(gpu);
    }
}

/// Frames presented to the host's EGL surface, through a context in the
/// host's share group current on it.
pub struct HostSurfaceTarget {
    swap: SurfaceSwap,
    size: Size,
    /// The frame's default framebuffer, imported into the backend's `GlGpu`.
    image: FrameImports,
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
    ) -> Result<(RendererBackend, HostSurfaceTarget), Error> {
        let (gl, platform, swap) = device.create_surface_context(surface.0)?;
        let backend = RendererBackend::from_parts(gl, platform);
        Ok((backend, HostSurfaceTarget { swap, size, image: FrameImports::default() }))
    }
}

impl FrameTarget for HostSurfaceTarget {
    type Gpu = GlGpu;

    /// Draws the next frame at `size` pixels, which the host's surface has.
    fn resize(&mut self, _gpu: &mut GlGpu, size: Size) {
        self.size = size;
    }

    fn begin_frame(&mut self, gpu: &mut GlGpu) -> Option<FrameImage> {
        let size = self.size;
        self.image.get_or_import(|| default_framebuffer(gpu, size))
    }

    fn end_frame(&mut self, gpu: &mut GlGpu) -> Handoff {
        if !self.image.release(gpu) {
            return Handoff::Dropped;
        }
        let _current = gpu.enter();
        if self.swap.swap() { Handoff::Presented } else { Handoff::Dropped }
    }

    fn tear_down(&mut self, gpu: &mut GlGpu) {
        self.image.release(gpu);
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
