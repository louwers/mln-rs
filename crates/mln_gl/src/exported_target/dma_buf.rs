// The dmabufs of an exported ring on Linux and FreeBSD. GL renders into
// memory that it imports rather than exports: the ring allocates each buffer
// on the display's GPU through GBM, or, for a renderer on the CPU, which has
// no GPU to allocate on, in shared memory through the kernel's udmabuf, then
// imports it with EGL_EXT_image_dma_buf_import_modifiers.

use std::ffi::{CStr, c_char, c_void};
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};

use khronos_egl as egl;
use mln_gfx::export::{DMA_BUF_MAX_PLANES, DRM_FORMAT_ABGR8888, DRM_FORMAT_MOD_LINEAR, DmaBufChoice};
use mln_gfx::{DmaBufFormat, DmaBufPlane, ExportKind, ExportedImage, ExportedRing};
use mln_util::error::Error;
use mln_util::size::Size;

use super::Images;
use crate::egl::Egl;

/// The kind of image that the ring shares.
pub(super) const KIND: ExportKind = ExportKind::DmaBuf;

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

type QueryModifiers = unsafe extern "system" fn(
    *mut c_void,
    egl::Int,
    egl::Int,
    *mut u64,
    *mut egl::Boolean,
    *mut egl::Int,
) -> egl::Boolean;
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
pub(super) struct Memory {
    modifier: u64,
    planes: Vec<DmaBufPlane>,
    _fds: Vec<OwnedFd>,
}

impl Memory {
    /// The dmabuf, as the host imports it.
    pub(super) fn exported(&self, layout: &Layout) -> ExportedImage {
        ExportedImage::DmaBuf { fourcc: layout.fourcc, modifier: self.modifier, planes: self.planes.clone() }
    }
}

/// The fourcc and the modifiers that a ring's dmabufs have.
pub(super) type Layout = DmaBufChoice;

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

    fn allocate(&self, size: Size, choice: &DmaBufChoice) -> Result<Memory, Error> {
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
                    Ok(Memory { modifier, planes, _fds: fds })
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
fn udmabuf_allocate(device: &OwnedFd, size: Size) -> Result<Memory, Error> {
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
    #[cfg_attr(
        target_env = "gnu",
        expect(
            trivial_numeric_casts,
            reason = "glibc's ioctl takes a `c_ulong` request, and other C libraries' a `c_int`"
        )
    )]
    let fd = unsafe { libc::ioctl(device.as_raw_fd(), UDMABUF_CREATE as _, &create) };
    if fd < 0 {
        return Err(Error::native("udmabuf can't share the dmabuf's memory"));
    }
    // SAFETY: the ioctl made the descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let plane = DmaBufPlane { fd: fd.as_raw_fd(), offset: 0, stride };
    Ok(Memory { modifier: DRM_FORMAT_MOD_LINEAR, planes: vec![plane], _fds: vec![fd] })
}

/// What makes a display's dmabufs: the query of the modifiers that it
/// imports, and the allocator.
pub(super) struct Source {
    display: *mut c_void,
    query_modifiers: QueryModifiers,
    allocator: Allocator,
}

impl Source {
    /// The display's source, where it imports dmabufs and the system
    /// allocates them, or `None`.
    pub(super) fn load(egl: &'static Egl, display: egl::Display, has: impl Fn(&str) -> bool) -> Option<Source> {
        if !has("EGL_EXT_image_dma_buf_import_modifiers") {
            return None;
        }
        let function = egl.get_proc_address("eglQueryDmaBufModifiersEXT")? as *const c_void;
        // SAFETY: the function has its extension's type.
        let query_modifiers = unsafe { std::mem::transmute::<*const c_void, QueryModifiers>(function) };
        let allocator = match render_node(egl, display) {
            Some(node) => Allocator::gbm(&node)?,
            #[cfg(target_os = "linux")]
            None => Allocator::udmabuf()?,
            #[cfg(not(target_os = "linux"))]
            None => return None,
        };
        Some(Source { display: display.as_ptr(), query_modifiers, allocator })
    }

    /// A layout to probe the display with: the first modifier of ABGR8888
    /// that the display imports and the allocator makes.
    pub(super) fn probe_layout(&self) -> Option<Layout> {
        let modifiers = self.display_modifiers(DRM_FORMAT_ABGR8888);
        let formats: Vec<DmaBufFormat> =
            modifiers.into_iter().map(|modifier| DmaBufFormat { fourcc: DRM_FORMAT_ABGR8888, modifier }).collect();
        self.choice(&formats)
    }

    /// The layout of `ring`'s dmabufs: of the host's formats, those that the
    /// display imports and the allocator makes.
    pub(super) fn layout(&self, ring: &ExportedRing) -> Option<Layout> {
        self.choice(&ring.dma_buf_formats)
    }

    /// A dmabuf of `size` pixels in `layout`, and an `EGLImage` over every
    /// plane of it.
    pub(super) fn allocate(
        &self,
        images: &Images,
        size: Size,
        layout: &Layout,
    ) -> Result<(Memory, *mut c_void), Error> {
        let buffer = self.allocator.allocate(size, layout)?;
        let mut attributes = vec![
            egl::WIDTH,
            size.width as egl::Int,
            egl::HEIGHT,
            size.height as egl::Int,
            LINUX_DRM_FOURCC,
            layout.fourcc as egl::Int,
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
        let image = unsafe { images.create(LINUX_DMA_BUF, std::ptr::null_mut(), &attributes) }
            .ok_or_else(|| Error::native("EGL can't make an image of the dmabuf"))?;
        Ok((buffer, image))
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
