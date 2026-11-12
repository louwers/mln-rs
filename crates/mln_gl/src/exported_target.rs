// A ring of dmabufs that the target allocates and renders into through
// EGLImages, for the host to import elsewhere, such as in a compositor.
//
// GL renders into memory that it imports rather than exports: the target
// allocates each buffer on the display's GPU through GBM, or, for a renderer
// on the CPU, which has no GPU to allocate on, in shared memory through the
// kernel's udmabuf, then imports it with EGL_EXT_image_dma_buf_import_modifiers
// and draws into a texture over the image (GL_OES_EGL_image). A resize makes a
// new generation of buffers; the frames that the host holds keep the old
// generation's buffers until they are given back.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_char, c_void};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::rc::Rc;

use glow::HasContext;
use khronos_egl as egl;
use mln_gfx::command_encoder::CommandEncoder;
use mln_gfx::export::{DMA_BUF_MAX_PLANES, DRM_FORMAT_ABGR8888, DRM_FORMAT_MOD_LINEAR, DmaBufChoice, Generations};
use mln_gfx::frame::{self, FrameTarget, Handoff};
use mln_gfx::{
    ConsumerSync, DmaBufFormat, DmaBufPlane, ExportKind, ExportKinds, ExportedImage, ExportedRing, SyncKind, SyncKinds,
};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::borrowed_target::{BorrowedTarget, BorrowedTextures};
use crate::context::Context;
use crate::device::Device;
use crate::egl::Egl;
use crate::renderer_backend::RendererBackend;
use crate::types::{FramebufferID, TextureID};

// EGL_EXT_image_dma_buf_import(_modifiers) and EGL_EXT_device_query, spelled
// out since EGL's headers differ in which extensions they declare.
const LINUX_DMA_BUF: egl::Enum = 0x3270;
const LINUX_DRM_FOURCC: egl::Int = 0x3271;
const PLANE_ATTRIBUTES: [[egl::Int; 5]; DMA_BUF_MAX_PLANES] = [
    // fd, offset, pitch, modifier low, modifier high.
    [0x3272, 0x3273, 0x3274, 0x3443, 0x3444],
    [0x3275, 0x3276, 0x3277, 0x3445, 0x3446],
    [0x3278, 0x3279, 0x327A, 0x3447, 0x3448],
    [0x3440, 0x3441, 0x3442, 0x3449, 0x344A],
];
const EGL_DEVICE: egl::Int = 0x322C;
const DRM_RENDER_NODE_FILE: egl::Int = 0x3377;

type CreateImage =
    unsafe extern "system" fn(*mut c_void, *mut c_void, egl::Enum, *mut c_void, *const egl::Int) -> *mut c_void;
type DestroyImage = unsafe extern "system" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type QueryModifiers = unsafe extern "system" fn(
    *mut c_void,
    egl::Int,
    egl::Int,
    *mut u64,
    *mut egl::Boolean,
    *mut egl::Int,
) -> egl::Boolean;
type ImageTargetTexture = unsafe extern "system" fn(u32, *mut c_void);
type QueryDisplayAttrib = unsafe extern "system" fn(*mut c_void, egl::Int, *mut isize) -> egl::Boolean;
type QueryDeviceString = unsafe extern "system" fn(*mut c_void, egl::Int) -> *const c_char;

/// libgbm's functions, loaded at run time so that the library runs without
/// GBM.
struct Gbm {
    _library: libloading::Library,
    create_device: unsafe extern "C" fn(i32) -> *mut c_void,
    device_destroy: unsafe extern "C" fn(*mut c_void),
    modifier_plane_count: unsafe extern "C" fn(*mut c_void, u32, u64) -> i32,
    create_with_modifiers: unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const u64, u32) -> *mut c_void,
    plane_count: unsafe extern "C" fn(*mut c_void) -> i32,
    fd_for_plane: unsafe extern "C" fn(*mut c_void, i32) -> i32,
    modifier: unsafe extern "C" fn(*mut c_void) -> u64,
    offset: unsafe extern "C" fn(*mut c_void, i32) -> u32,
    stride: unsafe extern "C" fn(*mut c_void, i32) -> u32,
    destroy: unsafe extern "C" fn(*mut c_void),
}

impl Gbm {
    fn load() -> Option<Gbm> {
        // SAFETY: libgbm's initializers have no requirements, and each symbol
        // has the type of its declaration in gbm.h.
        unsafe {
            let library = libloading::Library::new("libgbm.so.1").ok()?;
            macro_rules! load {
                ($name:literal) => {
                    *library.get($name).ok()?
                };
            }
            Some(Gbm {
                create_device: load!(b"gbm_create_device\0"),
                device_destroy: load!(b"gbm_device_destroy\0"),
                modifier_plane_count: load!(b"gbm_device_get_format_modifier_plane_count\0"),
                create_with_modifiers: load!(b"gbm_bo_create_with_modifiers\0"),
                plane_count: load!(b"gbm_bo_get_plane_count\0"),
                fd_for_plane: load!(b"gbm_bo_get_fd_for_plane\0"),
                modifier: load!(b"gbm_bo_get_modifier\0"),
                offset: load!(b"gbm_bo_get_offset\0"),
                stride: load!(b"gbm_bo_get_stride_for_plane\0"),
                destroy: load!(b"gbm_bo_destroy\0"),
                _library: library,
            })
        }
    }
}

/// A dmabuf that an allocator made: the descriptors it owns, and where each
/// of its planes lies.
struct Buffer {
    modifier: u64,
    planes: Vec<DmaBufPlane>,
    _fds: Vec<OwnedFd>,
}

/// What allocates a ring's dmabufs.
enum Allocator {
    /// GBM on the display's GPU, through its render node.
    Gbm { gbm: Gbm, device: *mut c_void, _node: OwnedFd },
    /// Linear buffers in shared memory, for a renderer on the CPU.
    #[cfg(target_os = "linux")]
    Udmabuf { device: OwnedFd },
}

impl Allocator {
    fn gbm(node: &CStr) -> Option<Allocator> {
        let gbm = Gbm::load()?;
        // SAFETY: a path, opened for reading and writing.
        let fd = unsafe { libc::open(node.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        // SAFETY: the descriptor is new, and the allocator's.
        let node = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: a render node's descriptor, which outlives the device.
        let device = unsafe { (gbm.create_device)(node.as_raw_fd()) };
        (!device.is_null()).then_some(Allocator::Gbm { gbm, device, _node: node })
    }

    #[cfg(target_os = "linux")]
    fn udmabuf() -> Option<Allocator> {
        // SAFETY: a path, opened for reading and writing.
        let fd = unsafe { libc::open(c"/dev/udmabuf".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        // SAFETY: the descriptor is new, and the allocator's.
        (fd >= 0).then(|| Allocator::Udmabuf { device: unsafe { OwnedFd::from_raw_fd(fd) } })
    }

    /// Whether the allocator makes and shares a buffer of `fourcc` with
    /// `modifier`, in every plane that the modifier gives it.
    fn allocates(&self, fourcc: u32, modifier: u64) -> bool {
        match self {
            Allocator::Gbm { gbm, device, .. } => {
                // SAFETY: a plain query of the allocator's device.
                let planes = unsafe { (gbm.modifier_plane_count)(*device, fourcc, modifier) };
                (1..=DMA_BUF_MAX_PLANES as i32).contains(&planes)
            }
            #[cfg(target_os = "linux")]
            Allocator::Udmabuf { .. } => modifier == DRM_FORMAT_MOD_LINEAR,
        }
    }

    fn allocate(&self, size: Size, choice: &DmaBufChoice) -> Result<Buffer, Error> {
        match self {
            Allocator::Gbm { gbm, device, .. } => {
                // SAFETY: the modifiers are the allocator's to choose from.
                let object = unsafe {
                    (gbm.create_with_modifiers)(
                        *device,
                        size.width,
                        size.height,
                        choice.fourcc,
                        choice.modifiers.as_ptr(),
                        choice.modifiers.len() as u32,
                    )
                };
                if object.is_null() {
                    return Err(Error::native("GBM can't allocate the dmabuf"));
                }
                // The descriptors keep the memory once the buffer object goes.
                let result = (|| {
                    // SAFETY: plain queries of the new buffer object.
                    let (modifier, count) = unsafe { ((gbm.modifier)(object), (gbm.plane_count)(object)) };
                    if !choice.modifiers.contains(&modifier) {
                        return Err(Error::native(format!(
                            "GBM chose the modifier {modifier:#x}, which the host doesn't import"
                        )));
                    }
                    if !(1..=DMA_BUF_MAX_PLANES as i32).contains(&count) {
                        return Err(Error::native("GBM made a dmabuf of too many planes"));
                    }
                    let mut fds: Vec<OwnedFd> = Vec::new();
                    let mut planes = Vec::new();
                    for plane in 0..count {
                        // SAFETY: as above, of one of its planes.
                        let (fd, offset, stride) = unsafe {
                            (
                                (gbm.fd_for_plane)(object, plane),
                                (gbm.offset)(object, plane),
                                (gbm.stride)(object, plane),
                            )
                        };
                        if fd < 0 {
                            return Err(Error::native("GBM can't export the dmabuf"));
                        }
                        // SAFETY: the export made the descriptor.
                        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                        // GBM exports each plane as a descriptor of its own,
                        // even of one dmabuf: such planes share the first.
                        let shared =
                            fds.iter().find(|earlier| same_file(earlier, &fd)).map(std::os::fd::AsRawFd::as_raw_fd);
                        let raw = if let Some(raw) = shared {
                            raw
                        } else {
                            let raw = fd.as_raw_fd();
                            fds.push(fd);
                            raw
                        };
                        planes.push(DmaBufPlane { fd: raw, offset, stride });
                    }
                    Ok(Buffer { modifier, planes, _fds: fds })
                })();
                // SAFETY: the buffer object is the allocator's.
                unsafe { (gbm.destroy)(object) };
                result
            }
            #[cfg(target_os = "linux")]
            Allocator::Udmabuf { device } => udmabuf_allocate(device, size),
        }
    }
}

impl Drop for Allocator {
    fn drop(&mut self) {
        if let Allocator::Gbm { gbm, device, .. } = self {
            // SAFETY: the device is the allocator's, and its buffers are gone.
            unsafe { (gbm.device_destroy)(*device) };
        }
    }
}

fn same_file(a: &OwnedFd, b: &OwnedFd) -> bool {
    // SAFETY: plain queries of two open descriptors.
    unsafe {
        let mut first: libc::stat = std::mem::zeroed();
        let mut second: libc::stat = std::mem::zeroed();
        libc::fstat(a.as_raw_fd(), &raw mut first) == 0
            && libc::fstat(b.as_raw_fd(), &raw mut second) == 0
            && first.st_dev == second.st_dev
            && first.st_ino == second.st_ino
    }
}

/// A linear buffer of `size` pixels in a sealed memfd that udmabuf shares as
/// a dmabuf, with rows of a multiple of 256 bytes, which GPUs that import
/// linear buffers take.
#[cfg(target_os = "linux")]
fn udmabuf_allocate(device: &OwnedFd, size: Size) -> Result<Buffer, Error> {
    #[repr(C)]
    struct Create {
        memfd: u32,
        flags: u32,
        offset: u64,
        size: u64,
    }
    // _IOW('u', 0x42, struct udmabuf_create).
    const UDMABUF_CREATE: libc::c_ulong = 0x4018_7542;
    const UDMABUF_FLAGS_CLOEXEC: u32 = 1;
    let stride = (size.width * 4).div_ceil(256) * 256;
    // SAFETY: a plain query.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    let bytes = (u64::from(stride) * u64::from(size.height)).div_ceil(page) * page;
    // SAFETY: a new memfd, which the buffer owns.
    let memory = unsafe { libc::memfd_create(c"mln-dma-buf".as_ptr(), libc::MFD_ALLOW_SEALING | libc::MFD_CLOEXEC) };
    if memory < 0 {
        return Err(Error::native("allocating the dmabuf's memory failed"));
    }
    // SAFETY: the descriptor is new.
    let memory = unsafe { OwnedFd::from_raw_fd(memory) };
    // SAFETY: the memfd is the buffer's; udmabuf takes memory that can't
    // shrink.
    let sized = unsafe {
        libc::ftruncate(memory.as_raw_fd(), bytes as libc::off_t) == 0
            && libc::fcntl(memory.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_SHRINK) == 0
    };
    if !sized {
        return Err(Error::native("sizing the dmabuf's memory failed"));
    }
    let create = Create { memfd: memory.as_raw_fd() as u32, flags: UDMABUF_FLAGS_CLOEXEC, offset: 0, size: bytes };
    // SAFETY: the ioctl reads a udmabuf_create, and returns a new descriptor.
    let fd = unsafe { libc::ioctl(device.as_raw_fd(), UDMABUF_CREATE as _, &create) };
    if fd < 0 {
        return Err(Error::native("udmabuf can't share the dmabuf's memory"));
    }
    // SAFETY: the ioctl made the descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let plane = DmaBufPlane { fd: fd.as_raw_fd(), offset: 0, stride };
    Ok(Buffer { modifier: DRM_FORMAT_MOD_LINEAR, planes: vec![plane], _fds: vec![fd] })
}

/// What renders into dmabufs on a share group's display: the EGL and GL
/// functions that import them, and the allocator that makes them.
pub(crate) struct Exporter {
    display: *mut c_void,
    create_image: CreateImage,
    destroy_image: DestroyImage,
    query_modifiers: QueryModifiers,
    image_target_texture: ImageTargetTexture,
    allocator: Allocator,
}

// SAFETY: the display, the allocator's GBM device and the functions may be
// used from any thread; the target calls the GL function only with its
// context current.
unsafe impl Send for Exporter {}
// SAFETY: as above.
unsafe impl Sync for Exporter {}

impl Exporter {
    /// The exporter of `device`'s display, once a buffer of it imports, or
    /// `None` where the display or the system can't.
    pub(crate) fn probe(device: &Device) -> Option<Exporter> {
        let exporter = Self::load(device)?;
        let modifiers = exporter.display_modifiers(DRM_FORMAT_ABGR8888);
        let formats: Vec<DmaBufFormat> =
            modifiers.into_iter().map(|modifier| DmaBufFormat { fourcc: DRM_FORMAT_ABGR8888, modifier }).collect();
        let choice = exporter.choice(&formats)?;
        match exporter.import(
            &exporter.allocator.allocate(Size::new(1, 1), &choice).ok()?,
            choice.fourcc,
            Size::new(1, 1),
        ) {
            Ok(image) => {
                // SAFETY: the image is the probe's.
                unsafe { (exporter.destroy_image)(exporter.display, image) };
                Some(exporter)
            }
            Err(error) => {
                Log::info(Event::GraphicsBackend, format!("The EGL display imports no dmabuf: {}", error.message));
                None
            }
        }
    }

    fn load(device: &Device) -> Option<Exporter> {
        let (egl, display) = device.display();
        let extensions = egl.query_string(Some(display), egl::EXTENSIONS).ok()?.to_string_lossy().into_owned();
        let has = |name: &str| extensions.split(' ').any(|extension| extension == name);
        if !has("EGL_KHR_image_base") || !has("EGL_EXT_image_dma_buf_import_modifiers") {
            return None;
        }
        let lookup = |name: &str| egl.get_proc_address(name).map(|function| function as *const c_void);
        // SAFETY: each function has its extension's type.
        let (create_image, destroy_image, query_modifiers, image_target_texture) = unsafe {
            (
                std::mem::transmute::<*const c_void, CreateImage>(lookup("eglCreateImageKHR")?),
                std::mem::transmute::<*const c_void, DestroyImage>(lookup("eglDestroyImageKHR")?),
                std::mem::transmute::<*const c_void, QueryModifiers>(lookup("eglQueryDmaBufModifiersEXT")?),
                std::mem::transmute::<*const c_void, ImageTargetTexture>(lookup("glEGLImageTargetTexture2DOES")?),
            )
        };
        let allocator = match render_node(egl, display) {
            Some(node) => Allocator::gbm(&node)?,
            #[cfg(target_os = "linux")]
            None => Allocator::udmabuf()?,
            #[cfg(not(target_os = "linux"))]
            None => return None,
        };
        Some(Exporter {
            display: display.as_ptr(),
            create_image,
            destroy_image,
            query_modifiers,
            image_target_texture,
            allocator,
        })
    }

    /// The modifiers of `fourcc` that the display imports as textures to
    /// render into: those that aren't external only.
    fn display_modifiers(&self, fourcc: u32) -> Vec<u64> {
        let mut count = 0;
        // SAFETY: a count query of the display.
        let counted = unsafe {
            (self.query_modifiers)(
                self.display,
                fourcc as egl::Int,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut count,
            )
        };
        if counted == egl::FALSE || count <= 0 {
            return Vec::new();
        }
        let mut modifiers = vec![0u64; count as usize];
        let mut external = vec![egl::FALSE; count as usize];
        // SAFETY: arrays of the count that the display reported.
        let listed = unsafe {
            (self.query_modifiers)(
                self.display,
                fourcc as egl::Int,
                count,
                modifiers.as_mut_ptr(),
                external.as_mut_ptr(),
                &raw mut count,
            )
        };
        if listed == egl::FALSE {
            return Vec::new();
        }
        modifiers.truncate(count.max(0) as usize);
        modifiers.into_iter().zip(external).filter(|(_, external)| *external == egl::FALSE).map(|(m, _)| m).collect()
    }

    /// The host's formats of dmabufs that the display imports and the
    /// allocator makes.
    fn choice(&self, formats: &[DmaBufFormat]) -> Option<DmaBufChoice> {
        mln_gfx::export::choose_dma_buf_format(formats, |format| {
            self.allocator.allocates(format.fourcc, format.modifier)
                && self.display_modifiers(format.fourcc).contains(&format.modifier)
        })
    }

    /// An `EGLImage` over every plane of `buffer`.
    fn import(&self, buffer: &Buffer, fourcc: u32, size: Size) -> Result<*mut c_void, Error> {
        let mut attributes = vec![
            egl::WIDTH,
            size.width as egl::Int,
            egl::HEIGHT,
            size.height as egl::Int,
            LINUX_DRM_FOURCC,
            fourcc as egl::Int,
        ];
        for (plane, names) in buffer.planes.iter().zip(PLANE_ATTRIBUTES) {
            attributes.extend([
                names[0],
                plane.fd,
                names[1],
                plane.offset as egl::Int,
                names[2],
                plane.stride as egl::Int,
                names[3],
                (buffer.modifier & 0xFFFF_FFFF) as u32 as egl::Int,
                names[4],
                (buffer.modifier >> 32) as u32 as egl::Int,
            ]);
        }
        attributes.push(egl::NONE);
        // SAFETY: the attributes name the buffer's live descriptors.
        let image = unsafe {
            (self.create_image)(
                self.display,
                std::ptr::null_mut(),
                LINUX_DMA_BUF,
                std::ptr::null_mut(),
                attributes.as_ptr(),
            )
        };
        if image.is_null() {
            return Err(Error::native("EGL can't make an image of the dmabuf"));
        }
        Ok(image)
    }
}

/// The DRM render node of the display's GPU, or `None` for a display
/// without one, such as a renderer on the CPU.
fn render_node(egl: &'static Egl, display: egl::Display) -> Option<std::ffi::CString> {
    let lookup = |name: &str| egl.get_proc_address(name).map(|function| function as *const c_void);
    // SAFETY: each function has its extension's type.
    let (query_display, query_device) = unsafe {
        (
            std::mem::transmute::<*const c_void, QueryDisplayAttrib>(lookup("eglQueryDisplayAttribEXT")?),
            std::mem::transmute::<*const c_void, QueryDeviceString>(lookup("eglQueryDeviceStringEXT")?),
        )
    };
    let mut device = 0isize;
    // SAFETY: plain queries of the display and its device.
    unsafe {
        if query_display(display.as_ptr(), EGL_DEVICE, &raw mut device) == egl::FALSE || device == 0 {
            return None;
        }
        let node = query_device(device as *mut c_void, DRM_RENDER_NODE_FILE);
        (!node.is_null()).then(|| CStr::from_ptr(node).to_owned())
    }
}

/// The kinds of exported ring that sessions on `device` share their images
/// in: dmabufs, where the display imports one that the target allocated.
#[must_use]
pub fn export_kinds(device: &Device) -> ExportKinds {
    if device.exporter().is_some() { ExportKinds::of(&[ExportKind::DmaBuf]) } else { ExportKinds::NONE }
}

/// The kind of `ring` that a session on `device` shares its images in.
///
/// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
/// host takes no dmabuf, or none of its formats that the device renders.
pub fn plan(device: &Device, ring: &ExportedRing) -> Result<ExportKind, Error> {
    if !ring.kinds.contains(ExportKind::DmaBuf) {
        return Err(Error::unsupported("an OpenGL device exports only dmabufs"));
    }
    let exporter = device.exporter().ok_or_else(|| Error::unsupported("the EGL display can't render into dmabufs"))?;
    exporter
        .choice(&ring.dma_buf_formats)
        .map(|_| ExportKind::DmaBuf)
        .ok_or_else(|| Error::unsupported("the EGL display can render into a dmabuf of none of the formats"))
}

/// The kinds of sync with which the host waits for a frame (the
/// producer's), and the target for the host's reads (the consumer's): a
/// finished frame, since the host reads outside the share group.
#[must_use]
pub fn sync_kinds() -> (SyncKinds, SyncKinds) {
    let kinds = SyncKinds::of(&[SyncKind::CpuComplete]);
    (kinds, kinds)
}

/// A slot's dmabuf, its image, and the texture over it with the
/// framebuffer that a frame's blit draws into.
struct SlotTexture {
    buffer: Buffer,
    image: *mut c_void,
    texture: TextureID,
    framebuffer: FramebufferID,
}

/// A generation's GL objects and images, which go with it.
struct Objects {
    context: std::rc::Weak<Context>,
    exporter: std::sync::Arc<Exporter>,
    slots: Vec<SlotTexture>,
    /// The texture that frames render into, and the framebuffer that their
    /// blits read.
    canvas: Option<(TextureID, FramebufferID)>,
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
        for slot in self.slots.drain(..) {
            if let Some(context) = &context {
                context.abandon_framebuffer(slot.framebuffer);
                context.abandon_texture(slot.texture);
            }
            // SAFETY: the image is the generation's, of its display; a
            // texture over it keeps its storage.
            unsafe { (self.exporter.destroy_image)(self.exporter.display, slot.image) };
        }
    }
}

/// One generation of a ring's buffers, of one size. Frames render into a
/// texture of the generation's, through a borrowed ring target of that one
/// texture, and each frame's end blits it into the slot's dmabuf upside
/// down: GL's rows start at the bottom of the image, and a dmabuf's readers
/// take its first row as the top.
struct Generation {
    // Before the objects, whose canvas its framebuffer attaches.
    target: BorrowedTarget,
    objects: Objects,
    size: Size,
    fourcc: u32,
}

impl Generation {
    /// Call it with the backend's context current.
    fn new(
        backend: &Rc<RendererBackend>,
        exporter: &std::sync::Arc<Exporter>,
        choice: &DmaBufChoice,
        depth: usize,
        size: Size,
        producer: SyncKind,
    ) -> Result<Generation, Error> {
        let context = backend.gl_context().ok_or_else(|| Error::native("the OpenGL backend has no context"))?;
        let mut objects =
            Objects { context: context.weak(), exporter: exporter.clone(), slots: Vec::new(), canvas: None };
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
                    return Err(Error::native(format!("a dmabuf makes no complete framebuffer: status {status:#x}")));
                }
                Ok(framebuffer)
            }
        };
        for _ in 0..depth {
            let buffer = exporter.allocator.allocate(size, choice)?;
            let image = exporter.import(&buffer, choice.fourcc, size)?;
            // SAFETY: as above, and the image is of the context's display.
            let texture = match unsafe { gl.create_texture() } {
                Ok(texture) => texture,
                Err(error) => {
                    // SAFETY: the image is new, and nothing uses it.
                    unsafe { (exporter.destroy_image)(exporter.display, image) };
                    return Err(Error::native(error));
                }
            };
            state.bind_texture(gl, 0, Some(texture));
            // SAFETY: as above, with the texture bound.
            unsafe {
                (exporter.image_target_texture)(glow::TEXTURE_2D, image);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            }
            state.bind_texture(gl, 0, None);
            let framebuffer = match framebuffer_of(texture) {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    context.abandon_texture(texture);
                    // SAFETY: as above.
                    unsafe { (exporter.destroy_image)(exporter.display, image) };
                    return Err(error);
                }
            };
            objects.slots.push(SlotTexture { buffer, image, texture, framebuffer });
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
        let textures = BorrowedTextures { textures: vec![canvas.0.get()], size };
        let target = BorrowedTarget::new(backend.clone(), textures, producer)?;
        Ok(Generation { target, objects, size, fourcc: choice.fourcc })
    }

    /// Copies the canvas into the dmabuf of `slot`, upside down. Call it with
    /// the context current.
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
            gl.blit_framebuffer(0, 0, width, height, 0, height, width, 0, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(slot.framebuffer));
        }
        true
    }
}

/// Frames drawn into dmabufs of the target's own, and queued for the host to
/// acquire.
pub struct ExportedTarget {
    backend: Rc<RendererBackend>,
    exporter: std::sync::Arc<Exporter>,
    choice: DmaBufChoice,
    depth: usize,
    producer: SyncKind,
    generations: RefCell<Generations<Generation>>,
    /// The slot that the next frame goes to.
    selected: Cell<usize>,
}

impl ExportedTarget {
    /// A target on `backend`, a renderer of `device`, that draws into
    /// `ring.depth` dmabufs of `size` pixels, and hands each frame over with
    /// sync of the kind `producer`, one of [`sync_kinds`]'s. Call it with the
    /// backend's context current.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) when the
    /// device can't export `kind`, a ring's formats or `producer`, and with
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
        if kind != ExportKind::DmaBuf {
            return Err(Error::unsupported(format!("an OpenGL device exports no {kind:?}")));
        }
        if !sync_kinds().0.contains(producer) {
            return Err(Error::unsupported(format!("an exported dmabuf hands no frame over with {producer:?}")));
        }
        let exporter =
            device.exporter().ok_or_else(|| Error::unsupported("the EGL display can't render into dmabufs"))?;
        let choice = exporter
            .choice(&ring.dma_buf_formats)
            .ok_or_else(|| Error::unsupported("the EGL display can render into a dmabuf of none of the formats"))?;
        let current = Generation::new(&backend, &exporter, &choice, ring.depth, size, producer)?;
        Ok(ExportedTarget {
            backend,
            exporter,
            choice,
            depth: ring.depth,
            producer,
            generations: RefCell::new(Generations::new(current)),
            selected: Cell::new(0),
        })
    }

    /// Has the next frame draw into the buffer at `slot`.
    pub fn select(&self, slot: usize) {
        self.selected.set(slot);
    }

    /// The host gave the frame at `slot` back, once its reads reach
    /// `consumer`, a finished frame. Call it with the backend's context
    /// current.
    ///
    /// Fails with [`ErrorKind::Unsupported`](mln_util::ErrorKind) for
    /// another kind of sync.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the release takes the host's sync, as every backend's does, and drops it"
    )]
    pub fn release(&self, slot: usize, consumer: ConsumerSync) -> Result<(), Error> {
        if !sync_kinds().1.contains(consumer.sync.kind) {
            return Err(Error::unsupported(format!("an exported dmabuf waits for no {:?}", consumer.sync.kind)));
        }
        self.generations.borrow_mut().give_back(slot);
        Ok(())
    }

    /// The host gave the frame at `slot` up.
    pub fn discard(&self, slot: usize) {
        self.generations.borrow_mut().give_back(slot);
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
        let next = Generation::new(&self.backend, &self.exporter, &self.choice, self.depth, size, self.producer)?;
        self.generations.borrow_mut().replace(next, held);
        Ok(true)
    }

    /// The size of the buffers that frames draw into.
    pub fn size(&self) -> Size {
        self.generations.borrow().current().size
    }

    /// The dmabuf of the frame at `slot`, and its size.
    pub fn image(&self, slot: usize) -> Option<(Size, ExportedImage)> {
        let generations = self.generations.borrow();
        let generation = generations.retired(slot).unwrap_or(generations.current());
        generation.objects.slots.get(slot).map(|texture| {
            let image = ExportedImage::DmaBuf {
                fourcc: generation.fourcc,
                modifier: texture.buffer.modifier,
                planes: texture.buffer.planes.clone(),
            };
            (generation.size, image)
        })
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
