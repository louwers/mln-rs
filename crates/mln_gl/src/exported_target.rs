// A ring of images that the target allocates and renders into through
// EGLImages, for the host to import elsewhere: dmabufs on Linux and FreeBSD,
// such as for a compositor (dma_buf.rs), and Android hardware buffers on
// Android (hardware_buffer.rs).
//
// GL renders into memory that it imports rather than exports: the target
// allocates each buffer, makes an EGLImage of it, and draws into a texture
// over the image (GL_OES_EGL_image). A resize makes a new generation of
// buffers; the frames that the host holds keep the old generation's buffers
// until they are given back.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;

use glow::HasContext;
use khronos_egl as egl;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::export::Generations;
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{
    ConsumerSync, ExportKind, ExportKinds, ExportedFrame, ExportedRing, FrameDepth, NativeTexture, SyncKind, SyncKinds,
};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::borrowed_target::{BorrowedTarget, BorrowedTextures};
use crate::context::Context;
use crate::device::Device;
use crate::renderer_backend::RendererBackend;
use crate::types::{FramebufferID, TextureID};

#[cfg(not(target_os = "android"))]
mod dma_buf;
#[cfg(not(target_os = "android"))]
use dma_buf as platform;
#[cfg(target_os = "android")]
mod hardware_buffer;
#[cfg(target_os = "android")]
use hardware_buffer as platform;
use platform::{Layout, Memory, Source};

type CreateImage =
    unsafe extern "system" fn(*mut c_void, *mut c_void, egl::Enum, *mut c_void, *const egl::Int) -> *mut c_void;
type DestroyImage = unsafe extern "system" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type ImageTargetTexture = unsafe extern "system" fn(u32, *mut c_void);

/// The display's `EGLImage`s (`EGL_KHR_image_base`), and the GL function that
/// makes a texture of one (`GL_OES_EGL_image`).
struct Images {
    display: *mut c_void,
    create_image: CreateImage,
    destroy_image: DestroyImage,
    image_target_texture: ImageTargetTexture,
}

impl Images {
    /// An image of `buffer`, of the kind `target`, without a context.
    ///
    /// # Safety
    ///
    /// `buffer` and `attributes`, which end with `EGL_NONE`, are valid for
    /// `target`.
    unsafe fn create(&self, target: egl::Enum, buffer: *mut c_void, attributes: &[egl::Int]) -> Option<*mut c_void> {
        // SAFETY: the caller's contract.
        let image =
            unsafe { (self.create_image)(self.display, std::ptr::null_mut(), target, buffer, attributes.as_ptr()) };
        (!image.is_null()).then_some(image)
    }

    /// Destroys `image`, which nothing uses but the textures over it, which
    /// keep its storage.
    fn destroy(&self, image: *mut c_void) {
        // SAFETY: the image is the display's, and the caller's.
        unsafe { (self.destroy_image)(self.display, image) };
    }
}

/// What renders into a display's shared images: the EGL and GL functions
/// that import them, what allocates them, and on Android the sync files that
/// hand them over.
pub(crate) struct Exporter {
    images: Images,
    source: Source,
    #[cfg(target_os = "android")]
    fences: Option<hardware_buffer::Fences>,
}

// SAFETY: the display, the source's allocator and the functions may be used
// from any thread; the target calls the GL function only with its context
// current.
unsafe impl Send for Exporter {}
// SAFETY: as above.
unsafe impl Sync for Exporter {}

impl Exporter {
    /// The exporter of `device`'s display, once an image of it imports, or
    /// `None` where the display or the system can't.
    pub(crate) fn probe(device: &Device) -> Option<Exporter> {
        let exporter = Self::load(device)?;
        let layout = exporter.source.probe_layout()?;
        match exporter.source.allocate(&exporter.images, Size::new(1, 1), &layout) {
            Ok((_memory, image)) => {
                exporter.images.destroy(image);
                Some(exporter)
            }
            Err(error) => {
                Log::info(
                    Event::GraphicsBackend,
                    format!("The EGL display imports no {:?}: {}", platform::KIND, error.message),
                );
                None
            }
        }
    }

    fn load(device: &Device) -> Option<Exporter> {
        let (egl, display) = device.display();
        let extensions = egl.query_string(Some(display), egl::EXTENSIONS).ok()?.to_string_lossy().into_owned();
        let has = |name: &str| extensions.split(' ').any(|extension| extension == name);
        if !has("EGL_KHR_image_base") {
            return None;
        }
        let lookup = |name: &str| egl.get_proc_address(name).map(|function| function as *const c_void);
        // SAFETY: each function has its extension's type.
        let images = unsafe {
            Images {
                display: display.as_ptr(),
                create_image: std::mem::transmute::<*const c_void, CreateImage>(lookup("eglCreateImageKHR")?),
                destroy_image: std::mem::transmute::<*const c_void, DestroyImage>(lookup("eglDestroyImageKHR")?),
                image_target_texture: std::mem::transmute::<*const c_void, ImageTargetTexture>(lookup(
                    "glEGLImageTargetTexture2DOES",
                )?),
            }
        };
        let source = Source::load(egl, display, has)?;
        Some(Exporter {
            images,
            source,
            #[cfg(target_os = "android")]
            fences: hardware_buffer::Fences::load(egl, display, has),
        })
    }
}

/// The kinds of exported ring that sessions on `device` share their images
/// in: dmabufs on Linux and FreeBSD, and hardware buffers on Android, where
/// the display imports one that the target allocated.
#[must_use]
pub fn export_kinds(device: &Device) -> ExportKinds {
    if device.exporter().is_some() { ExportKinds::of(&[platform::KIND]) } else { ExportKinds::NONE }
}

/// The kind of `ring` that a session on `device` shares its images in.
///
/// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
/// host takes no image of the kind the device exports, or for a dmabuf none
/// of its formats that the device renders, or for a hardware buffer none
/// that Android allocates with the host's usage.
pub fn plan(device: &Device, ring: &ExportedRing) -> Result<ExportKind, Error> {
    if !ring.kinds.contains(platform::KIND) {
        return Err(Error::unsupported(format!("an OpenGL device here exports only {:?}", platform::KIND)));
    }
    let exporter = device
        .exporter()
        .ok_or_else(|| Error::unsupported(format!("the EGL display can't render into {:?}", platform::KIND)))?;
    exporter.source.layout(ring).map(|_| platform::KIND).ok_or_else(|| {
        Error::unsupported(format!("the EGL display can render into no {:?} of the ring", platform::KIND))
    })
}

/// The kinds of sync with which the host waits for a frame (the
/// producer's), and the target for the host's reads (the consumer's): a
/// finished frame, since the host reads outside the share group, and on
/// Android a sync file, where the display has them.
#[must_use]
pub fn sync_kinds(device: &Device) -> (SyncKinds, SyncKinds) {
    let kinds = SyncKinds::of(&[SyncKind::CpuComplete]);
    #[cfg(target_os = "android")]
    let kinds = match device.exporter() {
        Some(exporter) if exporter.fences.is_some() => kinds.with(SyncKind::SyncFd),
        _ => kinds,
    };
    #[cfg(not(target_os = "android"))]
    let _ = device;
    (kinds, kinds)
}

/// A slot's buffer, its image, and the texture over it with the
/// framebuffer that a frame's blit draws into.
struct SlotTexture {
    memory: Memory,
    image: *mut c_void,
    texture: TextureID,
    framebuffer: FramebufferID,
    /// The texture that the blit copies the frame's depth and stencil into,
    /// when the ring keeps depth, which the framebuffer attaches.
    depth: Option<TextureID>,
    /// The sync file of the slot's last frame, which the host may wait on
    /// until it gives the slot back.
    #[cfg(target_os = "android")]
    producer: RefCell<Option<std::os::fd::OwnedFd>>,
}

/// A generation's GL objects and images, which go with it.
struct Objects {
    context: std::rc::Weak<Context>,
    exporter: std::sync::Arc<Exporter>,
    slots: Vec<SlotTexture>,
    /// The texture that frames render into, and the framebuffer that their
    /// blits read.
    canvas: Option<(TextureID, FramebufferID)>,
    /// The canvas's depth texture, when the ring keeps depth, which frames
    /// draw their depth into and blits read beside the canvas.
    canvas_depth: Option<TextureID>,
}

impl Drop for Objects {
    fn drop(&mut self) {
        let context = self.context.upgrade();
        if let Some((texture, framebuffer)) = self.canvas.take()
            && let Some(context) = &context
        {
            context.abandon_framebuffer(framebuffer);
            context.abandon_texture(texture);
        }
        if let Some(texture) = self.canvas_depth.take()
            && let Some(context) = &context
        {
            context.abandon_texture(texture);
        }
        for slot in self.slots.drain(..) {
            if let Some(context) = &context {
                context.abandon_framebuffer(slot.framebuffer);
                context.abandon_texture(slot.texture);
                if let Some(depth) = slot.depth {
                    context.abandon_texture(depth);
                }
            }
            // The image is the generation's, of its display; a texture over
            // it keeps its storage.
            self.exporter.images.destroy(slot.image);
        }
    }
}

/// One generation of a ring's buffers, of one size. Frames render into a
/// texture of the generation's, through a borrowed ring target of that one
/// texture, and each frame's end blits it into the slot's buffer upside
/// down: GL's rows start at the bottom of the image, and the readers of a
/// dmabuf or hardware buffer take its first row as the top.
struct Generation {
    // Before the objects, whose canvas its framebuffer attaches.
    target: BorrowedTarget,
    objects: Objects,
    size: Size,
    layout: Layout,
}

impl Generation {
    /// Call it with the backend's context current.
    fn new(
        backend: &Rc<RendererBackend>,
        exporter: &std::sync::Arc<Exporter>,
        layout: &Layout,
        depth: usize,
        size: Size,
        keep_depth: bool,
    ) -> Result<Generation, Error> {
        let context = backend.gl_context().ok_or_else(|| Error::native("the OpenGL backend has no context"))?;
        let mut objects = Objects {
            context: context.weak(),
            exporter: exporter.clone(),
            slots: Vec::new(),
            canvas: None,
            canvas_depth: None,
        };
        let gl = context.gl();
        let state = context.state();
        let framebuffer_of = |texture: TextureID| -> Result<FramebufferID, Error> {
            // SAFETY: the context is current (the caller's), and the
            // framebuffer is new; the state's cache sees each binding.
            unsafe {
                let framebuffer = gl.create_framebuffer().map_err(Error::native)?;
                state.bind_framebuffer(gl, Some(framebuffer));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                state.bind_framebuffer(gl, None);
                if status != glow::FRAMEBUFFER_COMPLETE {
                    context.abandon_framebuffer(framebuffer);
                    return Err(Error::native(format!(
                        "a {:?} makes no complete framebuffer: status {status:#x}",
                        platform::KIND
                    )));
                }
                Ok(framebuffer)
            }
        };
        let images = &exporter.images;
        for _ in 0..depth {
            let (memory, image) = exporter.source.allocate(images, size, layout)?;
            // SAFETY: as above, and the image is of the context's display.
            let texture = match unsafe { gl.create_texture() } {
                Ok(texture) => texture,
                Err(error) => {
                    images.destroy(image);
                    return Err(Error::native(error));
                }
            };
            state.bind_texture(gl, 0, Some(texture));
            // SAFETY: as above, with the texture bound.
            unsafe {
                (images.image_target_texture)(glow::TEXTURE_2D, image);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            }
            state.bind_texture(gl, 0, None);
            let framebuffer = match framebuffer_of(texture) {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    context.abandon_texture(texture);
                    images.destroy(image);
                    return Err(error);
                }
            };
            // Pushed first, so that the slot's objects go with the generation
            // when its depth texture fails.
            objects.slots.push(SlotTexture {
                memory,
                image,
                texture,
                framebuffer,
                depth: None,
                #[cfg(target_os = "android")]
                producer: RefCell::new(None),
            });
            if keep_depth {
                let depth = depth_texture(&context, framebuffer, size)?;
                if let Some(slot) = objects.slots.last_mut() {
                    slot.depth = Some(depth);
                }
            }
        }
        // SAFETY: as above.
        let canvas = unsafe { gl.create_texture() }.map_err(Error::native)?;
        state.bind_texture(gl, 0, Some(canvas));
        // SAFETY: as above, with the canvas bound.
        unsafe {
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, size.width as i32, size.height as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        }
        state.bind_texture(gl, 0, None);
        let read = match framebuffer_of(canvas) {
            Ok(read) => read,
            Err(error) => {
                context.abandon_texture(canvas);
                return Err(error);
            }
        };
        objects.canvas = Some((canvas, read));
        if keep_depth {
            objects.canvas_depth = Some(depth_texture(&context, read, size)?);
        }
        let textures = BorrowedTextures {
            textures: vec![canvas.0.get()],
            size,
            depth_textures: objects.canvas_depth.iter().map(|texture| texture.0.get()).collect(),
        };
        // The ring hands its frames over itself, so the canvas's frames end
        // with none but a finished one.
        let target = BorrowedTarget::new(backend.clone(), textures, SyncKind::CpuComplete)?;
        Ok(Generation { target, objects, size, layout: layout.clone() })
    }

    /// Copies the canvas into the buffer of `slot`, upside down, with its
    /// depth and stencil when the slot keeps them. Call it with the context
    /// current.
    fn blit(&self, context: &Context, slot: usize) -> bool {
        let (Some((_, read)), Some(slot)) = (self.objects.canvas, self.objects.slots.get(slot)) else { return false };
        let gl = context.gl();
        let (width, height) = (self.size.width as i32, self.size.height as i32);
        context.state().bind_framebuffer(gl, Some(slot.framebuffer));
        context.state().set_scissor(gl, None);
        // SAFETY: the context is current, and both framebuffers are the
        // generation's, of its size. The read binding goes back to the one
        // that the state's cache holds.
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(read));
            let mask = if slot.depth.is_some() {
                glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT
            } else {
                glow::COLOR_BUFFER_BIT
            };
            gl.blit_framebuffer(0, 0, width, height, 0, height, width, 0, mask, glow::NEAREST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(slot.framebuffer));
        }
        true
    }

    /// The host gave the frame at `slot` back: the sync file it waited on
    /// closes.
    #[cfg(target_os = "android")]
    fn given_back(&self, slot: usize) {
        if let Some(slot) = self.objects.slots.get(slot) {
            slot.producer.borrow_mut().take();
        }
    }
}

/// Frames drawn into buffers of the target's own, and queued for the host to
/// acquire.
pub struct ExportedTarget {
    backend: Rc<RendererBackend>,
    exporter: std::sync::Arc<Exporter>,
    layout: Layout,
    depth: usize,
    keep_depth: bool,
    /// The sync that hands a frame over, which only differs on Android.
    #[cfg(target_os = "android")]
    producer: SyncKind,
    consumers: SyncKinds,
    generations: RefCell<Generations<Generation>>,
    /// The slot that the next frame goes to.
    selected: Cell<usize>,
}

impl ExportedTarget {
    /// A target on `backend`, a renderer of `device`, that draws into
    /// `ring.depth` buffers of `size` pixels, and hands each frame over with
    /// sync of the kind `producer`, one of [`sync_kinds`]'s. Call it with the
    /// backend's context current.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device can't export `kind`, a ring's layout or `producer`, and with
    /// [`ErrorKind::Native`](mln_util::ErrorKind) when a buffer can't be
    /// made.
    pub fn new(
        backend: Rc<RendererBackend>,
        device: &Device,
        ring: &ExportedRing,
        kind: ExportKind,
        size: Size,
        producer: SyncKind,
    ) -> Result<ExportedTarget, Error> {
        if kind != platform::KIND {
            return Err(Error::unsupported(format!("an OpenGL device here exports no {kind:?}")));
        }
        let (producers, consumers) = sync_kinds(device);
        if !producers.contains(producer) {
            return Err(Error::unsupported(format!("an exported {kind:?} hands no frame over with {producer:?}")));
        }
        let exporter = device
            .exporter()
            .ok_or_else(|| Error::unsupported(format!("the EGL display can't render into {kind:?}")))?;
        let layout = exporter
            .source
            .layout(ring)
            .ok_or_else(|| Error::unsupported(format!("the EGL display can render into no {kind:?} of the ring")))?;
        let current = Generation::new(&backend, &exporter, &layout, ring.depth, size, ring.keep_depth)?;
        Ok(ExportedTarget {
            backend,
            exporter,
            layout,
            depth: ring.depth,
            keep_depth: ring.keep_depth,
            #[cfg(target_os = "android")]
            producer,
            consumers,
            generations: RefCell::new(Generations::new(current)),
            selected: Cell::new(0),
        })
    }

    /// Has the next frame draw into the buffer at `slot`.
    pub fn select(&self, slot: usize) {
        self.selected.set(slot);
    }

    /// The host gave the frame at `slot` back, once its reads reach
    /// `consumer`: a finished frame, or on Android a sync file, which the
    /// context's later commands wait for. Call it with the backend's context
    /// current.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another kind of sync, and with [`ErrorKind::Native`](mln_util::ErrorKind)
    /// when the context can't wait for the sync file.
    pub fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        let kind = consumer.sync.kind;
        if !self.consumers.contains(kind) {
            return Err(Error::unsupported(format!("an exported ring waits for no {kind:?}")));
        }
        let mut generations = self.generations.borrow_mut();
        // An older generation's buffer is never drawn into again.
        if generations.give_back(slot) {
            return Ok(());
        }
        #[cfg(target_os = "android")]
        {
            generations.current().given_back(slot);
            if let Some(file) = consumer.sync_file.filter(|_| kind == SyncKind::SyncFd)
                && let Some(fences) = &self.exporter.fences
            {
                return fences.wait(file);
            }
        }
        #[cfg(not(target_os = "android"))]
        drop(consumer);
        Ok(())
    }

    /// The host gave the frame at `slot` up.
    pub fn discard(&self, slot: usize) {
        let mut generations = self.generations.borrow_mut();
        #[cfg(target_os = "android")]
        if !generations.give_back(slot) {
            generations.current().given_back(slot);
        }
        #[cfg(not(target_os = "android"))]
        generations.give_back(slot);
    }

    /// Draws into buffers of `size` pixels from now on. The frames that the
    /// host holds, at `held` slots, keep their buffers. Returns whether the
    /// ring has new buffers, which it doesn't when they have `size` already.
    /// Call it with the backend's context current.
    ///
    /// Fails with [`ErrorKind::Native`](mln_util::ErrorKind) when a buffer
    /// can't be made, and the target then keeps its own.
    pub fn resize(&self, size: Size, held: &[usize]) -> Result<bool, Error> {
        if self.size() == size {
            return Ok(false);
        }
        let next = Generation::new(&self.backend, &self.exporter, &self.layout, self.depth, size, self.keep_depth)?;
        self.generations.borrow_mut().replace(next, held);
        Ok(true)
    }

    /// The size of the buffers that frames draw into.
    pub fn size(&self) -> Size {
        self.generations.borrow().current().size
    }

    /// The buffer of the frame at `slot`, its texture over it, and its depth
    /// texture when the ring keeps depth: upside down from OpenGL's rows, as
    /// the buffer's readers take them. The depth stays on the device: a
    /// dmabuf or a hardware buffer holds no depth format.
    pub fn image(&self, slot: usize) -> Option<ExportedFrame> {
        let generations = self.generations.borrow();
        let generation = generations.retired(slot).unwrap_or(generations.current());
        generation.objects.slots.get(slot).map(|texture| ExportedFrame {
            size: generation.size,
            image: texture.memory.exported(&generation.layout),
            texture: NativeTexture::OpenGl {
                texture: texture.texture.0.get(),
                target: glow::TEXTURE_2D,
                internal_format: 0,
            },
            depth: texture.depth.map(|depth| FrameDepth {
                texture: NativeTexture::OpenGl {
                    texture: depth.0.get(),
                    target: glow::TEXTURE_2D,
                    internal_format: glow::DEPTH24_STENCIL8,
                },
                export: None,
            }),
        })
    }

    /// Hands the blitted frame at `slot` over with a sync file of its own,
    /// which the slot keeps until the host gives it back.
    #[cfg(target_os = "android")]
    fn hand_over_with_sync_file(&self, generation: &Generation, slot: usize) -> Handoff {
        use std::os::fd::AsRawFd as _;
        let (Some(fences), Some(texture)) = (&self.exporter.fences, generation.objects.slots.get(slot)) else {
            return Handoff::Dropped;
        };
        match fences.signal(self.backend.gl()) {
            Ok(file) => {
                let handle = file.as_raw_fd() as u64;
                *texture.producer.borrow_mut() = Some(file);
                Handoff::Queued { slot, sync: mln_gfx::GpuSync { kind: SyncKind::SyncFd, handle, value: 0 } }
            }
            Err(error) => {
                Log::error(Event::Render, format!("Handing an exported frame over failed: {}", error.message));
                Handoff::Dropped
            }
        }
    }
}

impl FrameTarget for ExportedTarget {
    fn begin_frame(&self) -> Option<Rc<dyn frame::DrawTarget>> {
        self.generations.borrow().current().target.begin_frame()
    }

    fn end_frame(&self, encoder: &dyn CommandEncoder) -> Handoff {
        let slot = self.selected.get();
        let generations = self.generations.borrow();
        let generation = generations.current();
        let Some(context) = self.backend.gl_context() else { return Handoff::Dropped };
        if !generation.blit(&context, slot) {
            return Handoff::Dropped;
        }
        // A sync file signals once the blit is done, without the canvas's
        // frame waiting for it.
        #[cfg(target_os = "android")]
        if self.producer == SyncKind::SyncFd {
            return self.hand_over_with_sync_file(generation, slot);
        }
        // The canvas's frame ends, and with it the blit.
        match generation.target.end_frame(encoder) {
            Handoff::GivenBack { sync, .. } => Handoff::Queued { slot, sync },
            other => other,
        }
    }
}

impl Drop for ExportedTarget {
    fn drop(&mut self) {
        self.generations.get_mut().clear_retired();
    }
}

/// A `GL_DEPTH24_STENCIL8` texture of `size`, attached to `framebuffer`: a
/// canvas's, which frames draw into and blits read, or a slot's, which blits
/// copy into. Call it with the context current.
fn depth_texture(context: &Context, framebuffer: FramebufferID, size: Size) -> Result<TextureID, Error> {
    let gl = context.gl();
    let state = context.state();
    // SAFETY: the context is current (the caller's), and the texture is new;
    // the state's cache sees each binding.
    unsafe {
        let texture = gl.create_texture().map_err(Error::native)?;
        state.bind_texture(gl, 0, Some(texture));
        gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::DEPTH24_STENCIL8, size.width as i32, size.height as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        state.bind_texture(gl, 0, None);
        state.bind_framebuffer(gl, Some(framebuffer));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::DEPTH_STENCIL_ATTACHMENT,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        state.bind_framebuffer(gl, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            context.abandon_texture(texture);
            return Err(Error::native(format!("a depth texture makes no complete framebuffer: status {status:#x}")));
        }
        Ok(texture)
    }
}
